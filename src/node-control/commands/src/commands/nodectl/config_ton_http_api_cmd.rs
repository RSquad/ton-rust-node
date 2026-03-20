/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::utils::save_config;
use colored::Colorize;
use common::app_config::{AppConfig, EndpointEntry};
use std::path::Path;

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
#[command(about = "Set ton-http-api url and optional api key")]
pub struct TonHttpApiSetCmd {
    #[arg(short = 'u', long = "url")]
    url: String,
    #[arg(short = 'k', long = "api-key")]
    api_key: Option<String>,
}

#[derive(clap::Args, Clone)]
#[command(about = "Add one or more failover endpoint URLs for ton-http-api")]
pub struct TonHttpApiAddCmd {
    #[arg(short = 'u', long = "url", required = true)]
    urls: Vec<String>,
    /// Per-endpoint API key applied to all URLs in this invocation.
    /// When omitted, the endpoints inherit the global api_key.
    #[arg(short = 'k', long = "api-key")]
    api_key: Option<String>,
}

impl TonHttpApiCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        match &self.action {
            TonHttpApiAction::Set(cmd) => cmd.run(path).await,
            TonHttpApiAction::Add(cmd) => cmd.run(path).await,
        }
    }
}

impl TonHttpApiSetCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;
        let url = self.url.trim().to_string();
        if url.is_empty() {
            anyhow::bail!("--url value must not be empty");
        }

        config.ton_http_api.urls = vec![EndpointEntry::Url(url)];
        config.ton_http_api.api_key = self.api_key.clone();
        save_config(&config, path)?;

        let api_key_info =
            self.api_key.as_deref().map(|_| ", api_key=***").unwrap_or(", api_key=none");
        println!(
            "\n{} ton-http-api set: url='{}'{}\n",
            "OK".green().bold(),
            config.ton_http_api.urls[0].url(),
            api_key_info
        );
        Ok(())
    }
}

impl TonHttpApiAddCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = AppConfig::load(path)?;
        let new_urls: Vec<String> =
            self.urls.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect();

        if new_urls.is_empty() {
            anyhow::bail!("At least one non-empty --url value is required");
        }

        let mut existing = config.ton_http_api.endpoints();
        for url in &new_urls {
            if !existing.iter().any(|e| e == url) {
                let entry = match &self.api_key {
                    Some(key) => EndpointEntry::WithKey { url: url.clone(), api_key: key.clone() },
                    None => EndpointEntry::Url(url.clone()),
                };
                config.ton_http_api.urls.push(entry);
                existing.push(url.clone());
            }
        }
        save_config(&config, path)?;

        println!(
            "\n{} ton-http-api endpoints: [{}]\n",
            "OK".green().bold(),
            config.ton_http_api.endpoints().join(", "),
        );
        Ok(())
    }
}
