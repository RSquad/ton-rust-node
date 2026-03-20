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
use common::app_config::{AppConfig, LogConfig, LogOutput, LogRotation};
use std::path::{Path, PathBuf};

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

#[derive(clap::ValueEnum, Clone, Default)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
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
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        match &self.action {
            LogAction::Ls(cmd) => cmd.run(path).await,
            LogAction::Set(cmd) => cmd.run(path).await,
        }
    }
}

impl LogLsCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        let config = AppConfig::load(path)?;
        let log = config.log.as_ref().cloned().unwrap_or_default();

        match self.format {
            OutputFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&log)?);
            }
            OutputFormat::Table => {
                print_log_table(&log);
            }
        }
        Ok(())
    }
}

impl LogSetCmd {
    pub async fn run(&self, path: &Path) -> anyhow::Result<()> {
        if self.level.is_none()
            && self.path.is_none()
            && self.rotation.is_none()
            && self.output.is_none()
            && self.max_size_mb.is_none()
            && self.max_files.is_none()
        {
            anyhow::bail!(
                "No settings specified. Use --level, --path, --rotation, --output, --max-size-mb, or --max-files"
            );
        }

        let mut config = AppConfig::load(path)?;
        let log = config.log.get_or_insert_with(LogConfig::default);

        let mut changes = Vec::new();

        if let Some(level) = &self.level {
            log.level = level.to_tracing_level();
            changes.push(format!("level = {}", log.level));
        }
        if let Some(p) = &self.path {
            log.path = Some(p.clone());
            changes.push(format!("path = {}", p.display()));
        }
        if let Some(rotation) = &self.rotation {
            log.rotation = rotation.clone().into();
            changes.push(format!("rotation = {:?}", log.rotation).to_lowercase());
        }
        if let Some(output) = &self.output {
            log.output = output.clone().into();
            changes.push(format!("output = {:?}", log.output).to_lowercase());
        }
        if let Some(max_size) = self.max_size_mb {
            log.max_size_mb = max_size;
            changes.push(format!("max_size_mb = {}", max_size));
        }
        if let Some(max_files) = self.max_files {
            log.max_files = max_files;
            changes.push(format!("max_files = {}", max_files));
        }

        // Validate: file/all output requires a log path
        if matches!(log.output, LogOutput::File | LogOutput::All) && log.path.is_none() {
            anyhow::bail!(
                "Output mode '{}' requires a log file path. Use --path to set one.",
                output_display(&log.output)
            );
        }

        save_config(&config, path)?;
        println!("{} Log settings updated: {}", "OK".green().bold(), changes.join(", "));
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
