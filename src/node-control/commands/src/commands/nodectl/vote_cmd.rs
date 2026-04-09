/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::commands::nodectl::{output_format::OutputFormat, utils::save_config};
use anyhow::Context;
use base64::Engine;
use colored::Colorize;
use common::app_config::{AppConfig, VotingConfig};
use contracts::{ConfigContractImpl, ConfigContractWrapper, ConfigProposal, contract_provider};
use std::{
    io::{IsTerminal, Write},
    path::Path,
    sync::Arc,
    time::SystemTime,
};
use ton_block::write_boc;
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

#[derive(clap::Args, Clone)]
#[command(about = "Config proposals voting")]
pub struct VoteCmd {
    #[arg(
        short = 'c',
        long = "config",
        help = "Path to the configuration file",
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
        match &self.action {
            VoteAction::Ls(cmd) => cmd.run(&self.config).await,
            VoteAction::Inspect(cmd) => cmd.run(&self.config).await,
            VoteAction::Add(cmd) => cmd.run(&self.config).await,
            VoteAction::Rm(cmd) => cmd.run(&self.config),
        }
    }
}

// ── ls ──────────────────────────────────────────────────────────────────────

#[derive(clap::Args, Clone)]
struct VoteLsCmd {
    #[arg(long = "format", default_value = "table")]
    format: OutputFormat,
}

#[derive(serde::Serialize)]
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

fn proposal_to_row(p: &ConfigProposal, tracked_hashes: &[String]) -> ProposalRow {
    let hash = hex::encode(p.hash);
    ProposalRow {
        tracked: tracked_hashes.contains(&hash),
        hash,
        param_id: p.param.id,
        is_critical: p.is_critical,
        expires: p.expires,
        expires_in: format_expires(p.expires),
        voters_count: p.voters.len(),
        weight_remaining: p.weight_remaining,
        rounds_remaining: p.rounds_remaining,
        wins: p.wins,
        losses: p.losses,
    }
}

impl VoteLsCmd {
    async fn run(&self, config_path: &str) -> anyhow::Result<()> {
        let (config, rpc_client) = load_config_rpc(config_path)?;
        let config_contract = ConfigContractImpl::new(contract_provider!(rpc_client));

        let proposals = config_contract.list_proposals().await.context("list_proposals")?;

        if proposals.is_empty() {
            println!("No active proposals");
            return Ok(());
        }

        let tracked = tracked_proposals(&config);
        let rows: Vec<ProposalRow> =
            proposals.iter().map(|p| proposal_to_row(p, &tracked)).collect();

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

#[derive(serde::Serialize)]
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

impl From<&ConfigProposal> for ProposalDetail {
    fn from(p: &ConfigProposal) -> Self {
        Self {
            hash: hex::encode(p.hash),
            param_id: p.param.id,
            param_hash: p.param.hash.map(hex::encode),
            param_cell_boc: p.param.cell.as_ref().and_then(|c| {
                write_boc(c).ok().map(|boc| base64::engine::general_purpose::STANDARD.encode(&boc))
            }),
            is_critical: p.is_critical,
            expires: p.expires,
            expires_in: format_expires(p.expires),
            voters: p.voters.clone(),
            weight_remaining: p.weight_remaining,
            vset_id: hex::encode(p.vset_id),
            rounds_remaining: p.rounds_remaining,
            wins: p.wins,
            losses: p.losses,
        }
    }
}

impl VoteInspectCmd {
    async fn run(&self, config_path: &str) -> anyhow::Result<()> {
        let phash = parse_proposal_hash(&self.hash)?;
        let (_config, rpc_client) = load_config_rpc(config_path)?;
        let config_contract = ConfigContractImpl::new(contract_provider!(rpc_client));

        let proposal = config_contract
            .get_proposal(phash)
            .await
            .context("get_proposal")?
            .ok_or_else(|| anyhow::anyhow!("proposal not found"))?;

        let detail = ProposalDetail::from(&proposal);

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
                if let Some(ref h) = detail.param_hash {
                    println!("  {:<20} {}", "Param hash:".cyan().bold(), h);
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
    async fn run(&self, config_path: &str) -> anyhow::Result<()> {
        let path = Path::new(config_path);
        let (mut config, rpc_client) = load_config_rpc(config_path)?;
        let config_contract = ConfigContractImpl::new(contract_provider!(rpc_client));

        let proposals = config_contract.list_proposals().await.context("list_proposals")?;
        if proposals.is_empty() {
            anyhow::bail!("no active proposals on-chain");
        }

        let tracked = tracked_proposals(&config);

        let selected_hash = match &self.hash {
            Some(h) => {
                let phash = parse_proposal_hash(h)?;
                if !proposals.iter().any(|p| p.hash == phash) {
                    anyhow::bail!("proposal {} not found on-chain", h);
                }
                hex::encode(phash)
            }
            None => {
                require_interactive()?;
                select_proposal(&proposals, &tracked)?
            }
        };

        if tracked.contains(&selected_hash) {
            println!("Proposal {} is already tracked", selected_hash);
            return Ok(());
        }

        add_proposal_to_config(&mut config, &selected_hash);
        save_config(&config, path)?;

        println!("{} proposal {} added to voting config", "OK".green().bold(), selected_hash);
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
    fn run(&self, config_path: &str) -> anyhow::Result<()> {
        let path = Path::new(config_path);
        let mut config = AppConfig::load(path)?;

        let tracked = tracked_proposals(&config);

        if tracked.is_empty() {
            println!("No proposals in voting config");
            return Ok(());
        }

        let selected_hash = match &self.hash {
            Some(h) => {
                let normalized = h.to_lowercase();
                if !tracked.contains(&normalized) {
                    anyhow::bail!("proposal {} is not in voting config", h);
                }
                normalized
            }
            None => {
                require_interactive()?;
                select_tracked_proposal(&tracked)?
            }
        };

        let voting = config.voting.as_mut().unwrap();
        voting.proposals.retain(|h| h != &selected_hash);
        save_config(&config, path)?;

        println!("{} proposal {} removed from voting config", "OK".green().bold(), selected_hash);
        Ok(())
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn load_config_rpc(config_path: &str) -> anyhow::Result<(AppConfig, Arc<ClientJsonRpc>)> {
    let config = AppConfig::load(Path::new(config_path))?;
    let rpc_client = Arc::new(
        ClientJsonRpc::connect_many(
            config.ton_http_api.resolved_endpoints(),
            config.ton_http_api.api_key.clone(),
        )
        .context("ClientJsonRpc")?,
    );
    Ok((config, rpc_client))
}

fn tracked_proposals(config: &AppConfig) -> Vec<String> {
    config.voting.as_ref().map(|v| v.proposals.clone()).unwrap_or_default()
}

fn parse_proposal_hash(s: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = hex::decode(s).context("invalid hex")?;
    if bytes.len() != 32 {
        anyhow::bail!("proposal hash must be 32 bytes, got {}", bytes.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn format_expires(expires: u32) -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs()
        as u32;
    if expires <= now {
        return "expired".to_string();
    }
    let diff = expires - now;
    let days = diff / 86400;
    let hours = (diff % 86400) / 3600;
    let mins = (diff % 3600) / 60;
    if days > 0 {
        format!("in {}d {}h", days, hours)
    } else if hours > 0 {
        format!("in {}h {}m", hours, mins)
    } else {
        format!("in {}m", mins)
    }
}

fn require_interactive() -> anyhow::Result<()> {
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("--hash is required in non-interactive mode");
    }
    Ok(())
}

fn select_proposal(proposals: &[ConfigProposal], tracked: &[String]) -> anyhow::Result<String> {
    println!("\n  Active proposals:\n");
    for (i, p) in proposals.iter().enumerate() {
        let hash = hex::encode(p.hash);
        let marker = if tracked.contains(&hash) { "*" } else { " " };
        println!(
            "  {}{} [{}] p{} critical={} {} voters={}",
            marker.green().bold(),
            format!("  {}", i + 1).bold(),
            &hash[..16],
            p.param.id,
            if p.is_critical { "yes" } else { "no" },
            format_expires(p.expires),
            p.voters.len(),
        );
    }
    if tracked.iter().any(|h| proposals.iter().any(|p| hex::encode(p.hash) == *h)) {
        println!("\n  {} already tracked", "*".green().bold());
    }

    let idx = prompt_selection(proposals.len())?;
    Ok(hex::encode(proposals[idx].hash))
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
    std::io::stdout().flush()?;

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let n: usize = input.trim().parse().context("invalid number")?;
    if n == 0 || n > count {
        anyhow::bail!("selection out of range");
    }
    Ok(n - 1)
}

fn add_proposal_to_config(config: &mut AppConfig, hash: &str) {
    match config.voting.as_mut() {
        Some(voting) => {
            voting.proposals.push(hash.to_string());
        }
        None => {
            config.voting =
                Some(VotingConfig { proposals: vec![hash.to_string()], tick_interval: 40 });
        }
    }
}
