/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::utils::{api_post, resolve_service_url};
use colored::Colorize;

#[derive(clap::Args, Clone)]
#[command(about = "Manage ton-http-api configuration")]
pub struct TonHttpApiCmd {
    #[command(subcommand)]
    action: TonHttpApiAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum TonHttpApiAction {
    /// Set ton-http-api url and optional api key
    Set(TonHttpApiSetCmd),
    /// Add one or more failover endpoint URLs
    Add(TonHttpApiAddCmd),
}

#[derive(clap::Args, Clone)]
#[command(about = "Set ton-http-api endpoint and optional api key")]
pub struct TonHttpApiSetCmd {
    #[arg(short = 'e', long = "endpoint", help = "TON HTTP API endpoint")]
    endpoint: String,
    #[arg(short = 'k', long = "api-key")]
    api_key: Option<String>,
}

#[derive(clap::Args, Clone)]
#[command(about = "Add one or more failover endpoint URLs for ton-http-api")]
pub struct TonHttpApiAddCmd {
    #[arg(short = 'e', long = "endpoint", required = true, help = "TON HTTP API endpoint")]
    endpoints: Vec<String>,
    /// Per-endpoint API key applied to all URLs in this invocation.
    /// When omitted, the endpoints inherit the global api_key.
    #[arg(short = 'k', long = "api-key")]
    api_key: Option<String>,
}

impl TonHttpApiCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            TonHttpApiAction::Set(cmd) => cmd.run(url, token, config_path).await,
            TonHttpApiAction::Add(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

/// Shared body for `POST /v1/ton-http-api`.
#[derive(serde::Serialize)]
struct TonHttpApiBody<'a> {
    urls: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    append: bool,
}

fn print_endpoints(resp: &str) -> anyhow::Result<()> {
    let parsed: serde_json::Value = serde_json::from_str(resp)?;
    let endpoints = parsed["result"]["endpoints"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    println!("\n{} ton-http-api endpoints: [{}]\n", "OK".green().bold(), endpoints);
    Ok(())
}

impl TonHttpApiSetCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = TonHttpApiBody {
            urls: vec![self.endpoint.as_str()],
            api_key: self.api_key.as_deref(),
            append: false,
        };
        let resp = api_post(&base_url, "/v1/ton-http-api", token, &body).await?;
        print_endpoints(&resp)
    }
}

impl TonHttpApiAddCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = TonHttpApiBody {
            urls: self.endpoints.iter().map(|s| s.as_str()).collect(),
            api_key: self.api_key.as_deref(),
            append: true,
        };
        let resp = api_post(&base_url, "/v1/ton-http-api", token, &body).await?;
        print_endpoints(&resp)
    }
}
