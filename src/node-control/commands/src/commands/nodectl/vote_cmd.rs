/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::{
    output_format::OutputFormat,
    utils::{api_delete, api_get, api_post, resolve_service_url},
};
use anyhow::Context;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::{
    io::{IsTerminal, Write, stdin, stdout},
};

#[derive(clap::Args, Clone)]
#[command(about = "Config proposals voting (REST client; requires running nodectl service)")]
pub struct VoteCmd {
    #[arg(
        short = 'u',
        long = "url",
        value_hint = clap::ValueHint::Url,
        help = "URL to the node control service API (overrides --config; env: NODECTL_URL)",
        env = "NODECTL_URL",
        global = true
    )]
    url: Option<String>,

    #[arg(
        long = "token",
        env = "NODECTL_API_TOKEN",
        value_name = "TOKEN",
        help = "JWT token (nominator for read, operator for add/rm; env: NODECTL_API_TOKEN)",
        global = true
    )]
    token: Option<String>,

    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file (for service URL from http.bind)",
        default_value = "nodectl-config.json",
        env = "CONFIG_PATH",
        global = true
    )]
    config: String,

    #[command(subcommand)]
    action: VoteAction,
}

#[derive(clap::Subcommand, Clone)]
enum VoteAction {
    /// List active config proposals
    Ls(VoteLsCmd),
    /// Inspect a specific proposal
    Inspect(VoteInspectCmd),
    /// Add a proposal to the voting config
    Add(VoteAddCmd),
    /// Remove a proposal from the voting config
    Rm(VoteRmCmd),
}

impl VoteCmd {
    pub async fn run(&self) -> anyhow::Result<()> {
        let base_url = resolve_service_url(self.url.as_deref(), Some(self.config.as_str()))?;
        let token = self.token.as_deref();
        match &self.action {
            VoteAction::Ls(cmd) => cmd.run(&base_url, token).await,
            VoteAction::Inspect(cmd) => cmd.run(&base_url, token).await,
            VoteAction::Add(cmd) => cmd.run(&base_url, token).await,
            VoteAction::Rm(cmd) => cmd.run(&base_url, token).await,
        }
    }
}

// ── ls ──────────────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct VoteLsCmd {
    #[arg(long = "format", default_value = "table")]
    format: OutputFormat,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ProposalRow {
    hash: String,
    param_id: i32,
    is_critical: bool,
    expires: u32,
    expires_in: String,
    voters_count: usize,
    weight_remaining: i64,
    rounds_remaining: u8,
    wins: u8,
    losses: u8,
    tracked: bool,
}

impl VoteLsCmd {
    async fn run(&self, base_url: &str, token: Option<&str>) -> anyhow::Result<()> {
        let body = api_get(base_url, "/v1/voting/proposals", token).await?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        let rows: Vec<ProposalRow> = serde_json::from_value(
            v.get("result")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("voting proposals: missing 'result'"))?,
        )?;

        if rows.is_empty() {
            println!("No active proposals");
            return Ok(());
        }

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            }
            OutputFormat::Table => {
                println!(
                    "\n  {:<3} {:<66} {:<7} {:<9} {:<14} {:<7} {:<7} {}",
                    "".bold(),
                    "Hash".cyan().bold(),
                    "Param".cyan().bold(),
                    "Critical".cyan().bold(),
                    "Expires".cyan().bold(),
                    "Voters".cyan().bold(),
                    "Rounds".cyan().bold(),
                    "W/L".cyan().bold(),
                );
                println!("  {}", "\u{2500}".repeat(126).dimmed());
                for row in &rows {
                    let marker = if row.tracked { "*" } else { " " };
                    println!(
                        "  {:<3} {:<66} p{:<6} {:<9} {:<14} {:<7} {:<7} {}/{}",
                        marker.green().bold(),
                        row.hash,
                        row.param_id,
                        if row.is_critical { "yes" } else { "no" },
                        row.expires_in,
                        row.voters_count,
                        row.rounds_remaining,
                        row.wins,
                        row.losses,
                    );
                }
                if rows.iter().any(|r| r.tracked) {
                    println!("\n  {} tracked by voting task", "*".green().bold());
                }
                println!();
            }
        }

        Ok(())
    }
}

// ── inspect ─────────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct VoteInspectCmd {
    /// Proposal hash (hex)
    hash: String,

    #[arg(long = "format", default_value = "table")]
    format: OutputFormat,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProposalDetail {
    hash: String,
    param_id: i32,
    param_hash: Option<String>,
    param_cell_boc: Option<String>,
    is_critical: bool,
    expires: u32,
    expires_in: String,
    voters: Vec<u16>,
    weight_remaining: i64,
    vset_id: String,
    rounds_remaining: u8,
    wins: u8,
    losses: u8,
}

impl VoteInspectCmd {
    async fn run(&self, base_url: &str, token: Option<&str>) -> anyhow::Result<()> {
        let h = parse_proposal_hash_hex_normalized(&self.hash)?;
        let path = format!("/v1/voting/proposals/{h}");
        let body = api_get(base_url, &path, token).await?;
        let v: serde_json::Value = serde_json::from_str(&body)?;
        let detail: ProposalDetail = serde_json::from_value(
            v.get("result")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("inspect: missing 'result'"))?,
        )?;

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            }
            OutputFormat::Table => {
                println!("\n{}", "Proposal Details".cyan().bold());
                println!("{}", "\u{2500}".repeat(80).dimmed());
                println!("  {:<20} {}", "Hash:".cyan().bold(), detail.hash);
                println!("  {:<20} p{}", "Param ID:".cyan().bold(), detail.param_id);
                println!(
                    "  {:<20} {}",
                    "Critical:".cyan().bold(),
                    if detail.is_critical { "yes" } else { "no" }
                );
                println!(
                    "  {:<20} {} ({})",
                    "Expires:".cyan().bold(),
                    detail.expires,
                    detail.expires_in
                );
                println!("  {:<20} {}", "Rounds remaining:".cyan().bold(), detail.rounds_remaining);
                println!("  {:<20} {}", "Wins:".cyan().bold(), detail.wins);
                println!("  {:<20} {}", "Losses:".cyan().bold(), detail.losses);
                println!("  {:<20} {}", "Weight remaining:".cyan().bold(), detail.weight_remaining);
                println!("  {:<20} {}", "Vset ID:".cyan().bold(), &detail.vset_id);
                println!("  {:<20} {:?}", "Voters:".cyan().bold(), detail.voters);
                if let Some(ref boc) = detail.param_cell_boc {
                    println!("  {:<20} {}", "Param cell (b64):".cyan().bold(), boc);
                }
                if let Some(ref ph) = detail.param_hash {
                    println!("  {:<20} {}", "Param hash:".cyan().bold(), ph);
                }
                println!();
            }
        }

        Ok(())
    }
}

// ── add ─────────────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct VoteAddCmd {
    /// Proposal hash (hex). If omitted, shows interactive selection.
    #[arg(long = "hash")]
    hash: Option<String>,
}

impl VoteAddCmd {
    async fn run(&self, base_url: &str, token: Option<&str>) -> anyhow::Result<()> {
        let selected_hash = match &self.hash {
            Some(h) => parse_proposal_hash_hex_normalized(h)?,
            None => {
                require_interactive()?;
                let rows = fetch_proposal_rows(base_url, token).await?;
                if rows.is_empty() {
                    anyhow::bail!("no active proposals on-chain");
                }
                select_proposal(&rows)?
            }
        };

        if fetch_tracked_lower(base_url, token).await?.contains(&selected_hash.to_lowercase()) {
            println!("Proposal {selected_hash} is already tracked");
            return Ok(());
        }

        let body = serde_json::json!({ "hash": selected_hash });
        api_post(base_url, "/v1/voting/proposals", token, &body).await?;

        println!("{} proposal {selected_hash} added to voting config", "OK".green().bold());
        Ok(())
    }
}

// ── rm ──────────────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct VoteRmCmd {
    /// Proposal hash (hex). If omitted, shows interactive selection.
    #[arg(long = "hash")]
    hash: Option<String>,
}

impl VoteRmCmd {
    async fn run(&self, base_url: &str, token: Option<&str>) -> anyhow::Result<()> {
        let tracked = fetch_tracked_hashes(base_url, token).await?;
        if tracked.is_empty() {
            println!("No proposals in voting config");
            return Ok(());
        }

        let selected_hash = match &self.hash {
            Some(h) => {
                let normalized = parse_proposal_hash_hex_normalized(h)?;
                if !tracked.iter().any(|t| t.eq_ignore_ascii_case(&normalized)) {
                    anyhow::bail!("proposal {h} is not in voting config");
                }
                normalized
            }
            None => {
                require_interactive()?;
                select_tracked_proposal(&tracked)?
            }
        };

        let path = format!(
            "/v1/voting/proposals/{}",
            parse_proposal_hash_hex_normalized(&selected_hash)?
        );
        api_delete(base_url, &path, token).await?;

        println!(
            "{} proposal {} removed from voting config",
            "OK".green().bold(),
            selected_hash
        );
        Ok(())
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

async fn fetch_proposal_rows(base_url: &str, token: Option<&str>) -> anyhow::Result<Vec<ProposalRow>> {
    let body = api_get(base_url, "/v1/voting/proposals", token).await?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    serde_json::from_value(
        v.get("result")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("voting proposals: missing 'result'"))?,
    )
    .context("parse proposal rows")
}

async fn fetch_tracked_hashes(base_url: &str, token: Option<&str>) -> anyhow::Result<Vec<String>> {
    let body = api_get(base_url, "/v1/voting/config", token).await?;
    let v: serde_json::Value = serde_json::from_str(&body)?;
    let arr = v
        .get("result")
        .and_then(|r| r.get("proposals"))
        .and_then(|p| p.as_array())
        .ok_or_else(|| anyhow::anyhow!("voting config: missing result.proposals"))?;
    Ok(arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

async fn fetch_tracked_lower(base_url: &str, token: Option<&str>) -> anyhow::Result<Vec<String>> {
    Ok(fetch_tracked_hashes(base_url, token)
        .await?
        .into_iter()
        .map(|h| h.to_lowercase())
        .collect())
}

fn parse_proposal_hash_hex_normalized(s: &str) -> anyhow::Result<String> {
    let bytes = hex::decode(s.trim()).context("invalid hex")?;
    if bytes.len() != 32 {
        anyhow::bail!("proposal hash must be 32 bytes, got {}", bytes.len());
    }
    Ok(hex::encode(bytes))
}

fn require_interactive() -> anyhow::Result<()> {
    if !stdin().is_terminal() {
        anyhow::bail!("--hash is required in non-interactive mode");
    }
    Ok(())
}

fn select_proposal(proposals: &[ProposalRow]) -> anyhow::Result<String> {
    println!("\n  Active proposals:\n");
    for (i, p) in proposals.iter().enumerate() {
        let marker = if p.tracked { "*" } else { " " };
        println!(
            "  {}{} [{}] p{} critical={} {} voters={}",
            marker.green().bold(),
            format!("  {}", i + 1).bold(),
            p.hash.chars().take(16).collect::<String>(),
            p.param_id,
            if p.is_critical { "yes" } else { "no" },
            p.expires_in,
            p.voters_count,
        );
    }
    if proposals.iter().any(|p| p.tracked) {
        println!("\n  {} already tracked", "*".green().bold());
    }

    let idx = prompt_selection(proposals.len())?;
    Ok(proposals[idx].hash.clone())
}

fn select_tracked_proposal(tracked: &[String]) -> anyhow::Result<String> {
    println!("\n  Tracked proposals:\n");
    for (i, hash) in tracked.iter().enumerate() {
        println!("  {} {}", format!("  {}", i + 1).bold(), hash);
    }

    let idx = prompt_selection(tracked.len())?;
    Ok(tracked[idx].clone())
}

fn prompt_selection(count: usize) -> anyhow::Result<usize> {
    print!("\n  Select [1-{}]: ", count);
    stdout().flush()?;

    let mut input = String::new();
    stdin().read_line(&mut input)?;
    let n: usize = input.trim().parse().context("invalid number")?;
    if n == 0 || n > count {
        anyhow::bail!("selection out of range");
    }
    Ok(n - 1)
}
