/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use common::app_config::{AppConfig, StakePolicy};
use std::{
    borrow::Cow,
    io::{self, Read},
    path::Path,
};

#[derive(clap::Args, Clone)]
#[command(about = "Node control service REST API")]
pub struct ApiCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    // URL to the service API
    #[arg(short = 'u', long = "url", value_hint = clap::ValueHint::Url, help = "URL to the node control service API (default: http://127.0.0.1:8080)")]
    pub url: Option<String>,

    // JWT token to authenticate with the service API
    #[arg(
        long = "token",
        env = "NODECTL_API_TOKEN",
        value_name = "TOKEN",
        help = "JWT token to authenticate with the service API (or NODECTL_API_TOKEN)"
    )]
    pub token: Option<String>,

    #[command(subcommand)]
    action: ServiceAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum ServiceAction {
    Health,
    Elections(ElectionsCmd),
    Validators,
    Task(TaskCmd),
    StakePolicy(StakePolicyCmd),
    /// Authenticate and print the JWT token
    Login(LoginCmd),
}

#[derive(clap::Args, Clone)]
pub struct LoginCmd {
    #[arg(required = true, help = "Username to authenticate with")]
    pub username: String,
    #[arg(
        long = "password-stdin",
        help = "Read password from stdin instead of interactive prompt"
    )]
    pub password_stdin: bool,
}

#[derive(clap::Args, Clone)]
pub struct TaskCmd {
    #[arg(value_parser = ["elections", "voting"])]
    name: String,
    #[arg(value_enum)]
    action: TaskAction,
}

#[derive(clap::ValueEnum, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskAction {
    Enable,
    Disable,
    Restart,
}

#[derive(clap::Args, Clone)]
pub struct ElectionsCmd {
    #[arg(
        long = "exclude",
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        value_parser = parse_node_name,
        help = "List of controlled nodes to be excluded from elections (repeatable, or comma-separated)"
    )]
    pub exclude: Vec<String>,
    #[arg(
        long = "include",
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        value_parser = parse_node_name,
        help = "List of controlled nodes to be included in elections (repeatable, or comma-separated)"
    )]
    pub include: Vec<String>,
}

#[derive(clap::Args, Clone)]
pub struct StakePolicyCmd {
    #[arg(long = "fixed")]
    fixed: Option<u64>,
    #[arg(long = "split50")]
    split50: bool,
    #[arg(long = "minimum")]
    minimum: bool,
    #[arg(
        short = 'n',
        long = "node",
        help = "Apply policy only to this node (override). Omit to set the default policy."
    )]
    node: Option<String>,
}

impl ApiCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let base_url = if let Some(url) = self.url.as_deref() {
            normalize_base_url(Cow::Borrowed(url))
        } else {
            let app_cfg = AppConfig::load(Path::new(&self.config))?;
            normalize_base_url(Cow::Owned(app_cfg.http.bind.clone()))
        };
        let client = reqwest::Client::new();
        let token = self.token.as_deref();

        match &self.action {
            ServiceAction::Health => {
                let url = join_url(&base_url, "/health");
                send_get(&client, &url, token).await?;
            }
            ServiceAction::Elections(cmd) => {
                if !cmd.exclude.is_empty() {
                    let url = join_url(&base_url, "/v1/elections/exclude");
                    send_post(
                        &client,
                        &url,
                        &NodeListPayload { nodes: cmd.exclude.clone() },
                        token,
                    )
                    .await?;
                }
                if !cmd.include.is_empty() {
                    let url = join_url(&base_url, "/v1/elections/include");
                    send_post(
                        &client,
                        &url,
                        &NodeListPayload { nodes: cmd.include.clone() },
                        token,
                    )
                    .await?;
                }
                if cmd.exclude.is_empty() && cmd.include.is_empty() {
                    let url = join_url(&base_url, "/v1/elections");
                    send_get(&client, &url, token).await?;
                }
            }
            ServiceAction::Validators => {
                let url = join_url(&base_url, "/v1/validators");
                send_get(&client, &url, token).await?;
            }
            ServiceAction::Task(cmd) => {
                let url = join_url(&base_url, &format!("/v1/task/{}", cmd.name));
                let payload = ElectionsTaskControlRequest { action: cmd.action.clone() };
                send_post(&client, &url, &payload, token).await?;
            }
            ServiceAction::StakePolicy(cmd) => {
                let url = join_url(&base_url, "/v1/stake_strategy");
                let Some(policy) = cmd.to_policy() else {
                    anyhow::bail!("no policy specified");
                };
                let request = StakePolicyRequest { policy, node: cmd.node.clone() };
                send_post(&client, &url, &request, token).await?;
            }
            ServiceAction::Login(cmd) => {
                let url = join_url(&base_url, "/auth/login");
                let password = cmd.read_password()?;
                let payload = serde_json::json!({
                    "username": cmd.username,
                    "password": password,
                });
                send_post(&client, &url, &payload, None).await?;
            }
        }

        Ok(())
    }
}

impl LoginCmd {
    fn read_password(&self) -> anyhow::Result<String> {
        let password = if self.password_stdin {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input).context("failed to read password from stdin")?;
            input.trim_end_matches(['\n', '\r']).to_owned()
        } else {
            rpassword::prompt_password("Password: ").context("failed to read password")?
        };

        if password.is_empty() {
            anyhow::bail!("password cannot be empty");
        }

        Ok(password)
    }
}

#[derive(Clone, serde::Serialize)]
struct StakePolicyRequest {
    policy: StakePolicy,
    /// If set, the policy is applied as a per-node override.
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
}

impl StakePolicyCmd {
    fn to_policy(&self) -> Option<StakePolicy> {
        if let Some(v) = self.fixed {
            return Some(StakePolicy::Fixed(v));
        }
        if self.split50 {
            return Some(StakePolicy::Split50);
        }
        if self.minimum {
            return Some(StakePolicy::Minimum);
        }
        None
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
struct ElectionsTaskControlRequest {
    action: TaskAction,
}

#[derive(Clone, serde::Serialize)]
struct NodeListPayload {
    nodes: Vec<String>,
}

fn normalize_base_url(url: Cow<'_, str>) -> String {
    let mut base = url.into_owned();
    if base.starts_with("0.0.0.0") {
        base = base.replacen("0.0.0.0", "127.0.0.1", 1);
    }
    if !base.starts_with("http://") && !base.starts_with("https://") {
        base = format!("http://{}", base);
    }
    base
}

fn join_url(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

async fn send_get(client: &reqwest::Client, url: &str, token: Option<&str>) -> anyhow::Result<()> {
    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let response = req.send().await?;
    let status = response.status();
    let body = response.text().await?;
    handle_response(status, &body)
}

async fn send_post<T: serde::Serialize>(
    client: &reqwest::Client,
    url: &str,
    payload: &T,
    token: Option<&str>,
) -> anyhow::Result<()> {
    let mut req = client.post(url).json(payload);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let response = req.send().await?;
    let status = response.status();
    let body = response.text().await?;
    handle_response(status, &body)
}

fn handle_response(status: reqwest::StatusCode, body: &str) -> anyhow::Result<()> {
    if !status.is_success() {
        anyhow::bail!("request failed: status={}, body={}", status, body);
    }
    print_json(body);
    Ok(())
}

fn print_json(body: &str) {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => {
            if let Ok(pretty) = serde_json::to_string_pretty(&value) {
                println!("{}", pretty);
            } else {
                println!("{}", body);
            }
        }
        Err(_) => println!("{}", body),
    }
}

fn parse_node_name(s: &str) -> Result<String, String> {
    let v = s.trim();
    if v.is_empty() {
        return Err("node name cannot be empty".to_string());
    }
    Ok(v.to_string())
}
