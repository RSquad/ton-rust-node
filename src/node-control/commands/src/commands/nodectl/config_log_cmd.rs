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
    utils::{api_get, api_post, resolve_service_url},
};
use colored::Colorize;
use common::app_config::{LogConfig, LogOutput, LogRotation};
use std::path::PathBuf;

/// Manage log configuration
#[derive(clap::Args, Clone)]
#[command(about = "Manage log configuration")]
pub struct LogCmd {
    #[command(subcommand)]
    action: LogAction,
}

#[derive(clap::Subcommand, Clone)]
pub enum LogAction {
    /// Display current log settings
    Ls(LogLsCmd),
    /// Update log settings
    Set(LogSetCmd),
}

#[derive(clap::Args, Clone)]
pub struct LogLsCmd {
    #[arg(long = "format", default_value = "table", help = "Output format: table or json")]
    format: OutputFormat,
}

#[derive(clap::ValueEnum, Clone)]
pub enum LogLevelArg {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevelArg {
    fn to_tracing_level(&self) -> tracing::Level {
        match self {
            LogLevelArg::Trace => tracing::Level::TRACE,
            LogLevelArg::Debug => tracing::Level::DEBUG,
            LogLevelArg::Info => tracing::Level::INFO,
            LogLevelArg::Warn => tracing::Level::WARN,
            LogLevelArg::Error => tracing::Level::ERROR,
        }
    }
}

#[derive(clap::ValueEnum, Clone)]
pub enum RotationArg {
    Daily,
    Hourly,
    Never,
}

impl From<RotationArg> for LogRotation {
    fn from(arg: RotationArg) -> Self {
        match arg {
            RotationArg::Daily => LogRotation::Daily,
            RotationArg::Hourly => LogRotation::Hourly,
            RotationArg::Never => LogRotation::Never,
        }
    }
}

#[derive(clap::ValueEnum, Clone)]
pub enum OutputArg {
    Console,
    File,
    All,
}

impl From<OutputArg> for LogOutput {
    fn from(arg: OutputArg) -> Self {
        match arg {
            OutputArg::Console => LogOutput::Console,
            OutputArg::File => LogOutput::File,
            OutputArg::All => LogOutput::All,
        }
    }
}

#[derive(clap::Args, Clone)]
pub struct LogSetCmd {
    /// Log level (case insensitive)
    #[arg(long = "level", short = 'l', value_enum, ignore_case = true)]
    level: Option<LogLevelArg>,

    /// Log file path
    #[arg(long = "path", short = 'p')]
    path: Option<PathBuf>,

    /// Log rotation policy
    #[arg(long = "rotation", short = 'r', value_enum)]
    rotation: Option<RotationArg>,

    /// Output mode
    #[arg(long = "output", short = 'o', value_enum)]
    output: Option<OutputArg>,

    /// Max log file size in MB
    #[arg(long = "max-size-mb", short = 's')]
    max_size_mb: Option<u64>,

    /// Max number of rotated log files to keep
    #[arg(long = "max-files", short = 'f')]
    max_files: Option<usize>,
}

impl LogCmd {
    pub async fn run(
        &self,
        config_path: Option<&str>,
        url: Option<&str>,
        token: Option<&str>,
    ) -> anyhow::Result<()> {
        match &self.action {
            LogAction::Ls(cmd) => cmd.run(url, token, config_path).await,
            LogAction::Set(cmd) => cmd.run(url, token, config_path).await,
        }
    }
}

impl LogLsCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;
        let body = api_get(&base_url, "/v1/log", token).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)?;
        let result = &resp["result"];

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(result)?);
            }
            OutputFormat::Table => {
                let log = serde_json::from_value::<LogConfig>(result.clone())
                    .map_err(|e| anyhow::anyhow!("failed to parse log config: {e}"))?;
                print_log_table(&log);
            }
        }
        Ok(())
    }
}

#[derive(serde::Serialize)]
struct LogSetBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rotation: Option<LogRotation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<LogOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_size_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_files: Option<usize>,
}

impl LogSetCmd {
    pub async fn run(
        &self,
        url: Option<&str>,
        token: Option<&str>,
        config_path: Option<&str>,
    ) -> anyhow::Result<()> {
        let base_url = resolve_service_url(url, config_path)?;

        let body = LogSetBody {
            level: self.level.as_ref().map(|l| format!("{}", l.to_tracing_level())),
            path: self.path.as_ref().map(|p| p.display().to_string()),
            rotation: self.rotation.as_ref().map(|r| r.clone().into()),
            output: self.output.as_ref().map(|o| o.clone().into()),
            max_size_mb: self.max_size_mb,
            max_files: self.max_files,
        };

        let resp = api_post(&base_url, "/v1/log", token, &body).await?;
        let parsed: serde_json::Value = serde_json::from_str(&resp)?;
        let result = &parsed["result"];
        let log: LogConfig = serde_json::from_value(result.clone())
            .map_err(|e| anyhow::anyhow!("failed to parse log config: {e}"))?;
        print_log_table(&log);
        Ok(())
    }
}

fn rotation_display(r: &LogRotation) -> &str {
    match r {
        LogRotation::Daily => "daily",
        LogRotation::Hourly => "hourly",
        LogRotation::Never => "never",
    }
}

fn output_display(o: &LogOutput) -> &str {
    match o {
        LogOutput::Console => "console",
        LogOutput::File => "file",
        LogOutput::All => "all",
    }
}

fn print_log_table(log: &LogConfig) {
    let path_str = log
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(not set)".dimmed().to_string());

    println!("\n  {}", "Log Configuration".cyan().bold());
    println!("  {}", "─".repeat(60).dimmed());
    println!("  {:<24} {}", "Level:".bold(), log.level);
    println!("  {:<24} {}", "Output:".bold(), output_display(&log.output));
    println!("  {:<24} {}", "File Path:".bold(), path_str);
    println!("  {:<24} {}", "Rotation:".bold(), rotation_display(&log.rotation));
    println!("  {:<24} {} MB", "Max File Size:".bold(), log.max_size_mb);
    println!("  {:<24} {}", "Max Files:".bold(), log.max_files);
    println!();
}
