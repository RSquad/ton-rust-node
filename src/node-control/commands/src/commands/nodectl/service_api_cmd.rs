/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use colored::Colorize;
use common::{
    app_config::{AppConfig, StakePolicy},
    ton_utils::display_tons_from_str,
};
use std::{
    borrow::Cow,
    collections::HashSet,
    io::{self, Read},
    path::Path,
};

#[derive(clap::Args, Clone)]
#[command(about = "Node control service REST API")]
pub struct ApiCmd {
    // URL to the service API
    #[arg(
        short = 'u',
        long = "url",
        value_hint = clap::ValueHint::Url,
        help = "URL to the node control service API (takes precedence over --config)",
        global = true,
    )]
    pub url: Option<String>,

    // JWT token to authenticate with the service API
    #[arg(
        long = "token",
        env = "NODECTL_API_TOKEN",
        value_name = "TOKEN",
        help = "JWT token to authenticate with the service API (or NODECTL_API_TOKEN)",
        global = true
    )]
    pub token: Option<String>,

    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file (default: nodectl-config.json)",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    #[command(subcommand)]
    action: ServiceAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum ServiceAction {
    Health,
    Elections(ElectionsCmd),
    Validators(ValidatorsCmd),
    Task(TaskCmd),
    StakePolicy(StakePolicyCmd),
    /// Authenticate and print the JWT token
    Login(LoginCmd),
}

#[derive(clap::Args, Clone)]
pub struct ValidatorsCmd {
    #[arg(
        long = "format",
        default_value = "table",
        help = "Output format for validators view: table or json"
    )]
    pub format: ValidatorsOutputFormat,
    #[arg(
        short = 'f',
        long = "filter",
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        value_parser = parse_node_name,
        help = "Filter output by controlled node name (repeatable, or comma-separated)"
    )]
    pub filter: Vec<String>,
}

#[derive(clap::ValueEnum, Clone, Default, PartialEq, Eq)]
pub enum ValidatorsOutputFormat {
    #[default]
    Table,
    Json,
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
    #[arg(
        long = "all-participants",
        help = "Include full elections participants list (disabled by default)"
    )]
    pub all_participants: bool,
    #[arg(
        long = "format",
        default_value = "table",
        help = "Output format for elections view: table or json"
    )]
    pub format: ElectionsOutputFormat,
    #[arg(
        short = 'f',
        long = "filter",
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        value_parser = parse_node_name,
        help = "Filter output by controlled node name (repeatable, or comma-separated)"
    )]
    pub filter: Vec<String>,
}

#[derive(clap::ValueEnum, Clone, Default, PartialEq, Eq)]
pub enum ElectionsOutputFormat {
    #[default]
    Table,
    Json,
}

#[derive(clap::Args, Clone)]
pub struct StakePolicyCmd {
    #[arg(long = "fixed")]
    fixed: Option<u64>,
    #[arg(long = "split50")]
    split50: bool,
    #[arg(long = "minimum")]
    minimum: bool,
    #[arg(long = "adaptive-split50")]
    adaptive_split50: bool,
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
                    let mut url = join_url(&base_url, "/v1/elections");
                    if cmd.all_participants {
                        url.push_str("?include_participants=true");
                    }
                    let body = send_get_raw(&client, &url, token).await?;
                    let body =
                        filter_response_by_nodes(&body, &cmd.filter, NodeFilterTarget::Elections)?;
                    if cmd.format == ElectionsOutputFormat::Json {
                        print_json(&body);
                    } else {
                        print_elections_table(&body)?;
                    }
                }
            }
            ServiceAction::Validators(cmd) => {
                let url = join_url(&base_url, "/v1/validators");
                let body = send_get_raw(&client, &url, token).await?;
                let body =
                    filter_response_by_nodes(&body, &cmd.filter, NodeFilterTarget::Validators)?;
                if cmd.format == ValidatorsOutputFormat::Json {
                    print_json(&body);
                } else {
                    print_validators_table(&body)?;
                }
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

#[derive(Clone, Copy)]
enum NodeFilterTarget {
    Elections,
    Validators,
}

fn filter_response_by_nodes(
    body: &str,
    filters: &[String],
    target: NodeFilterTarget,
) -> anyhow::Result<String> {
    if filters.is_empty() {
        return Ok(body.to_owned());
    }

    let wanted: HashSet<&str> = filters.iter().map(String::as_str).collect();
    let mut value: serde_json::Value = serde_json::from_str(body).with_context(|| {
        let what = match target {
            NodeFilterTarget::Elections => "elections",
            NodeFilterTarget::Validators => "validators",
        };
        format!("failed to parse {what} response JSON while applying --filter")
    })?;

    match target {
        NodeFilterTarget::Elections => {
            if let Some(items) =
                value.get_mut("our_participants").and_then(serde_json::Value::as_array_mut)
            {
                items.retain(|item| {
                    item.get("node_id")
                        .and_then(serde_json::Value::as_str)
                        .map(|node_id| wanted.contains(node_id))
                        .unwrap_or(false)
                });
            }
        }
        NodeFilterTarget::Validators => {
            if let Some(items) = value
                .get_mut("result")
                .and_then(|v| v.get_mut("controlled_nodes"))
                .and_then(serde_json::Value::as_array_mut)
            {
                items.retain(|item| {
                    item.get("node_id")
                        .and_then(serde_json::Value::as_str)
                        .map(|node_id| wanted.contains(node_id))
                        .unwrap_or(false)
                });
            }
        }
    }

    Ok(serde_json::to_string(&value).context("failed to re-serialize filtered response to JSON")?)
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
        if self.adaptive_split50 {
            return Some(StakePolicy::AdaptiveSplit50);
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
    let body = send_get_raw(client, url, token).await?;
    print_json(&body);
    Ok(())
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
    ensure_success(status, &body)?;
    print_json(&body);
    Ok(())
}

async fn send_get_raw(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
) -> anyhow::Result<String> {
    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {t}"));
    }
    let response = req.send().await?;
    let status = response.status();
    let body = response.text().await?;
    ensure_success(status, &body)?;
    Ok(body)
}

fn ensure_success(status: reqwest::StatusCode, body: &str) -> anyhow::Result<()> {
    if !status.is_success() {
        anyhow::bail!("request failed: status={}, body={}", status, body);
    }
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

fn print_validators_table(body: &str) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    let ok = value.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let result = value.get("result");

    let nodes =
        result.and_then(|r| r.get("controlled_nodes")).and_then(serde_json::Value::as_array);

    let default_policy = result
        .and_then(|r| r.get("default_stake_policy"))
        .map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(obj) => {
                obj.iter()
                    .next()
                    .map(|(k, v)| {
                        if let Some(n) = v.as_u64() {
                            format!("{}({})", k, n)
                        } else {
                            k.to_string()
                        }
                    })
                    .unwrap_or_else(|| "-".to_string())
            }
            _ => "-".to_string(),
        })
        .unwrap_or_else(|| "-".to_string());

    let election_id = nodes
        .and_then(|arr| {
            arr.iter().filter_map(|n| n.get("key_election_id").and_then(|v| v.as_u64())).next()
        })
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());

    let validation_range = result.and_then(|r| r.get("validation_range"));
    let validation_start = validation_range
        .and_then(|r| r.get("start_utc"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let validation_end = validation_range
        .and_then(|r| r.get("end_utc"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");

    println!(
        "\n{} Validators (default policy: {})\n",
        if ok { "OK".green().bold() } else { "ERR".red().bold() },
        default_policy
    );
    println!("  {} {}", format!("{:<26}", "Election ID:").bold(), election_id);
    println!("  {} {}", format!("{:<26}", "Validation start:").bold(), validation_start);
    println!("  {} {}", format!("{:<26}", "Validation end:").bold(), validation_end);

    let Some(nodes) = nodes else {
        println!("  {}\n", "No nodes in response".yellow());
        return Ok(());
    };

    if nodes.is_empty() {
        println!("  {}\n", "No controlled nodes".yellow());
        return Ok(());
    }

    let total_weight: u64 =
        nodes.iter().filter_map(|n| n.get("weight").and_then(|v| v.as_u64())).sum();

    println!(
        "\n  {} {} {} {} {} {} {} {}",
        format!("{:<14}", "Node").cyan().bold(),
        format!("{:<13}", "Status").cyan().bold(),
        format!("{:<6}", "Index").cyan().bold(),
        format!("{:<10}", "Weight %").cyan().bold(),
        format!("{:<15}", "Stake TON").cyan().bold(),
        format!("{:<10}", "Key").cyan().bold(),
        format!("{:<44}", "Pubkey").cyan().bold(),
        "ADNL".cyan().bold(),
    );
    println!("  {}", "-".repeat(125).dimmed());

    for node in nodes {
        let node_id = binding_str(node, "node_id");

        let binding_status = node.get("binding_status").and_then(|v| v.as_str()).unwrap_or("idle");
        let status = match binding_status {
            "validating" => format!("{:<13}", "validating").green().bold().to_string(),
            "participating" => format!("{:<13}", "participating").blue().to_string(),
            "draining" => format!("{:<13}", "draining").yellow().to_string(),
            "idle" => format!("{:<13}", "idle").dimmed().to_string(),
            other => format!("{:<13}", other),
        };

        let validator_index = node
            .get("validator_index")
            .and_then(|v| v.as_u64())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());

        let weight_pct = node
            .get("weight")
            .and_then(|v| v.as_u64())
            .filter(|_| total_weight > 0)
            .map(|w| format!("{:.2}%", (w as f64 / total_weight as f64) * 100.0))
            .unwrap_or_else(|| "-".to_string());

        let stake = node.get("stake").and_then(|v| v.as_str()).unwrap_or("-");
        let stake_display = display_tons_from_str(stake);

        let is_key_active = node.get("is_key_active").and_then(|v| v.as_bool());
        let key_status = match is_key_active {
            Some(true) => format!("{:<10}", "active").green().to_string(),
            Some(false) => format!("{:<10}", "expired").yellow().to_string(),
            None => format!("{:<10}", "-"),
        };

        let pubkey = binding_str(node, "pubkey");
        let adnl = binding_str(node, "adnl");

        println!(
            "  {:<14} {} {:<6} {:<10} {:<15} {} {:<44} {}",
            node_id, status, validator_index, weight_pct, stake_display, key_status, pubkey, adnl,
        );

        if let Some(err) = node.get("last_error").and_then(|v| v.as_str()) {
            if !err.is_empty() {
                println!("  {} {}: {}", " ".repeat(14), "Error".red().bold(), err);
            }
        }
    }
    println!();
    Ok(())
}

fn print_elections_table(body: &str) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    let ok = value.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let status = value.get("status").and_then(serde_json::Value::as_str).unwrap_or("-").to_string();
    let result = value.get("result");

    let participants_count = result
        .and_then(|r| r.get("participants_count"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    let election_id = result
        .and_then(|r| r.get("election_id"))
        .and_then(serde_json::Value::as_u64)
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string());

    // Elections time range
    let elections_range = result.and_then(|r| r.get("elections_range"));
    let elections_start = elections_range
        .and_then(|r| r.get("start_utc"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let elections_end = elections_range
        .and_then(|r| r.get("end_utc"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");

    let min_stake =
        result.and_then(|r| r.get("min_stake")).and_then(serde_json::Value::as_str).unwrap_or("-");
    let participant_min_stake = result
        .and_then(|r| r.get("participant_min_stake"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let participant_max_stake = result
        .and_then(|r| r.get("participant_max_stake"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");
    let total_stake = result
        .and_then(|r| r.get("total_stake"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("-");

    println!("\n{} Elections\n", if ok { "OK".green().bold() } else { "ERR".red().bold() });
    println!("  {} {}", format!("{:<26}", "Status:").bold(), status);
    println!("  {} {}", format!("{:<26}", "Election ID:").bold(), election_id);
    println!("  {} {}", format!("{:<26}", "Elections start:").bold(), elections_start);
    println!("  {} {}", format!("{:<26}", "Elections end:").bold(), elections_end);
    println!("  {} {}", format!("{:<26}", "Participants count:").bold(), participants_count);
    println!("  {} {}", format!("{:<26}", "Min stake:").bold(), display_tons_from_str(min_stake));
    println!(
        "  {} {}",
        format!("{:<26}", "Participant min stake:").bold(),
        display_tons_from_str(participant_min_stake)
    );
    println!(
        "  {} {}",
        format!("{:<26}", "Participant max stake:").bold(),
        display_tons_from_str(participant_max_stake)
    );
    println!(
        "  {} {}",
        format!("{:<26}", "Total stake:").bold(),
        display_tons_from_str(total_stake)
    );

    let Some(participants) = value.get("our_participants").and_then(serde_json::Value::as_array)
    else {
        println!("\n  {}\n", "No participants in response".yellow());
        return Ok(());
    };

    if participants.is_empty() {
        println!("\n  {}\n", "No controlled participants".yellow());
        return Ok(());
    }

    println!("\n  {} ({})\n", "Our Participants".cyan().bold(), participants.len());
    println!(
        "  {} {} {} {} {} {} {} {} {}",
        format!("{:<14}", "Node").cyan().bold(),
        format!("{:<13}", "Status").cyan().bold(),
        format!("{:<5}", "Pos").cyan().bold(),
        format!("{:<15}", "Submitted TON").cyan().bold(),
        format!("{:<15}", "Accepted TON").cyan().bold(),
        format!("{:<24}", "Submitted At").cyan().bold(),
        format!("{:<6}", "MaxF").cyan().bold(),
        format!("{:<44}", "Pubkey").cyan().bold(),
        "ADNL".cyan().bold(),
    );
    println!("  {}", "-".repeat(148).dimmed());

    for p in participants {
        let node = binding_str(p, "node_id");

        // Get status with color coding (pad BEFORE coloring to fix alignment)
        // Flow: Idle → Participating → Submitted → Accepted → Elected → Validating
        let status_raw = p.get("status").and_then(serde_json::Value::as_str).unwrap_or("idle");
        let status = match status_raw {
            "validating" => format!("{:<13}", "validating").green().bold().to_string(),
            "elected" => format!("{:<13}", "elected").green().to_string(),
            "accepted" => format!("{:<13}", "accepted").cyan().to_string(),
            "submitted" => format!("{:<13}", "submitted").yellow().to_string(),
            "participating" => format!("{:<13}", "participating").blue().to_string(),
            _ => format!("{:<13}", "idle"),
        };

        // Get position
        let position = p
            .get("position")
            .and_then(serde_json::Value::as_u64)
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string());

        // Get stake submissions info
        let submissions = p.get("stake_submissions").and_then(serde_json::Value::as_array);

        // Sum all submitted stakes
        let total_submitted: u64 = submissions
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.get("stake").and_then(serde_json::Value::as_str))
                    .filter_map(|s| s.parse::<u64>().ok())
                    .sum()
            })
            .unwrap_or(0);
        let submitted_stake =
            if total_submitted > 0 { total_submitted.to_string() } else { "-".to_string() };

        // Get last submission time and max_factor
        let last_submission = submissions.and_then(|arr| arr.last());
        let submitted_at = last_submission
            .and_then(|s| s.get("submission_time_utc"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("-");
        let max_factor = last_submission
            .and_then(|s| s.get("max_factor"))
            .and_then(serde_json::Value::as_f64)
            .map(|v| format!("{:.1}", v))
            .unwrap_or_else(|| "-".to_string());

        let accepted_stake = binding_str(p, "accepted_stake");
        let pubkey = binding_str(p, "pubkey");
        let adnl = binding_str(p, "adnl");

        println!(
            "  {:<14} {} {:<5} {:<15} {:<15} {:<24} {:<6} {:<44} {}",
            node,
            status,
            position,
            display_tons_from_str(&submitted_stake),
            display_tons_from_str(&accepted_stake),
            submitted_at,
            max_factor,
            pubkey,
            adnl,
        );
    }
    println!();
    Ok(())
}

fn binding_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| "-".to_string())
}

fn parse_node_name(s: &str) -> Result<String, String> {
    let v = s.trim();
    if v.is_empty() {
        return Err("node name cannot be empty".to_string());
    }
    Ok(v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_elections_table_formatting() {
        let mock_json = r#"{
            "ok": true,
            "status": "active",
            "result": {
                "election_id": 1742300000,
                "elections_range": {
                    "start": 1742300000,
                    "start_utc": "2026-03-18 08:00:00",
                    "end": 1742386400,
                    "end_utc": "2026-03-19 08:00:00"
                },
                "participants_count": 5,
                "min_stake": "10000000000000",
                "participant_min_stake": "10000000000000",
                "participant_max_stake": "50000000000000",
                "total_stake": "100000000000000"
            },
            "our_participants": [
                {
                    "node_id": "node1",
                    "status": "validating",
                    "elected": true,
                    "position": 1,
                    "stake_accepted": true,
                    "accepted_stake": "25000000000000",
                    "pubkey": "obss1OX2obss1OX2obss1OX2obss1OX2obss1OX2obs=",
                    "adnl": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    "stake_submissions": [
                        {"stake": "15000000000000", "max_factor": 3.0, "submission_time_utc": "2026-03-18 10:00:00"},
                        {"stake": "10000000000000", "max_factor": 3.0, "submission_time_utc": "2026-03-18 12:30:00"}
                    ]
                },
                {
                    "node_id": "node2",
                    "status": "elected",
                    "elected": true,
                    "position": 2,
                    "stake_accepted": true,
                    "accepted_stake": "20000000000000",
                    "pubkey": "ssPE5fahr7LD5OX2oa+yw+Tl9qGvssPk5fahrssPk5c=",
                    "adnl": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=",
                    "stake_submissions": [
                        {"stake": "20000000000000", "max_factor": 2.5, "submission_time_utc": "2026-03-18 11:00:00"}
                    ]
                },
                {
                    "node_id": "node3",
                    "status": "submitted",
                    "elected": false,
                    "position": 5,
                    "stake_accepted": false,
                    "accepted_stake": null,
                    "pubkey": "w9Tl9qGvssLk5fahrsPU5fahtcPU5fahrsPS5fahr8M=",
                    "adnl": "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=",
                    "stake_submissions": [
                        {"stake": "10000000000000", "max_factor": 2.0, "submission_time_utc": "2026-03-18 12:00:00"}
                    ]
                },
                {
                    "node_id": "node4",
                    "status": "accepted",
                    "elected": false,
                    "position": 10,
                    "stake_accepted": true,
                    "accepted_stake": "12000000000000",
                    "pubkey": "1OX2oa+yw+Tl9qGuw9Tl9qG1w9Tl9qGuw9Ll9qGvw9Q=",
                    "adnl": "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD=",
                    "stake_submissions": [
                        {"stake": "12000000000000", "max_factor": 2.2, "submission_time_utc": "2026-03-18 13:00:00"}
                    ]
                },
                {
                    "node_id": "node5",
                    "status": "participating",
                    "elected": false,
                    "position": null,
                    "stake_accepted": false,
                    "accepted_stake": null,
                    "pubkey": "5fahrsPU5fahrsPS5fahrsPU5fahrcPU5fahrsPU5fY=",
                    "adnl": "EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE=",
                    "stake_submissions": []
                },
                {
                    "node_id": "node6",
                    "status": "idle",
                    "elected": false,
                    "position": null,
                    "stake_accepted": false,
                    "accepted_stake": null,
                    "pubkey": "",
                    "adnl": "",
                    "stake_submissions": []
                }
            ]
        }"#;

        // This test just verifies the function runs without panic
        // Run with `cargo test -p commands test_print_elections -- --nocapture` to see output
        let result = print_elections_table(mock_json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_print_validators_table_formatting() {
        let mock_json = r#"{
            "ok": true,
            "result": {
                "default_stake_policy": "split50",
                "validation_range": {
                    "start": 1742300000,
                    "start_utc": "2026-03-18 08:00:00",
                    "end": 1742386400,
                    "end_utc": "2026-03-19 08:00:00"
                },
                "controlled_nodes": [
                    {
                        "node_id": "node1",
                        "is_validator": true,
                        "validator_index": 0,
                        "weight": 10000,
                        "wallet_addr": "-1:aabbccdd",
                        "stake": "25000000000000",
                        "stake_accepted": true,
                        "key_election_id": 1742300000,
                        "key_expires_at_utc": "2026-03-19 08:00:00",
                        "is_key_active": true,
                        "pubkey": "obss1OX2obss1OX2obss1OX2obss1OX2obss1OX2obs=",
                        "adnl": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                        "binding_status": "validating"
                    },
                    {
                        "node_id": "node2",
                        "is_validator": false,
                        "wallet_addr": "-1:ddeeff00",
                        "stake": "15000000000000",
                        "stake_accepted": false,
                        "key_election_id": 1742300000,
                        "key_expires_at_utc": "2026-03-19 08:00:00",
                        "is_key_active": true,
                        "pubkey": "ssPE5fahr7LD5OX2oa+yw+Tl9qGvssPk5fahrssPk5c=",
                        "adnl": "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=",
                        "binding_status": "participating"
                    },
                    {
                        "node_id": "node3",
                        "is_validator": false,
                        "wallet_addr": "-1:11223344",
                        "stake_accepted": false,
                        "binding_status": "idle"
                    }
                ]
            }
        }"#;

        let result = print_validators_table(mock_json);
        assert!(result.is_ok());
    }

    #[test]
    fn test_filter_response_by_nodes_for_elections() {
        let body = r#"{
            "ok": true,
            "status": "active",
            "result": {"election_id": 1},
            "our_participants": [
                {"node_id": "node1", "status": "idle"},
                {"node_id": "node2", "status": "submitted"}
            ]
        }"#;

        let filtered =
            filter_response_by_nodes(body, &["node2".to_string()], NodeFilterTarget::Elections)
                .unwrap();
        let value: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        let participants = value["our_participants"].as_array().unwrap();

        assert_eq!(participants.len(), 1);
        assert_eq!(participants[0]["node_id"], "node2");
    }

    #[test]
    fn test_filter_response_by_nodes_for_validators() {
        let body = r#"{
            "ok": true,
            "result": {
                "controlled_nodes": [
                    {"node_id": "node1", "is_validator": true},
                    {"node_id": "node2", "is_validator": false}
                ]
            }
        }"#;

        let filtered =
            filter_response_by_nodes(body, &["node1".to_string()], NodeFilterTarget::Validators)
                .unwrap();
        let value: serde_json::Value = serde_json::from_str(&filtered).unwrap();
        let nodes = value["result"]["controlled_nodes"].as_array().unwrap();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0]["node_id"], "node1");
    }
}
