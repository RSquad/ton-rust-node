/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

mod copy_file_to_hashicorp;
mod delete;
mod generate;
mod get;
mod import;
mod init;
mod list;
mod migrate;
mod sign;
mod utils;
mod verify;

use crate::utils::{parse_hex_bytes, HexBytes};
use clap::Parser;
use colored::Colorize;
use secrets_vault::types::algorithm::Algorithm;

#[derive(clap::Parser)]
#[command(name = "vault")]
#[command(version = "0.1.0")]
#[command(about = "CLI client for SecretsVault operations", long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    Init {},
    List {
        #[arg(long, short)]
        full: bool,

        /// Reveal private key material in --full output. WARNING: writes raw key bytes to stdout.
        #[arg(long)]
        show_private: bool,
    },
    Delete {
        #[arg(required = true)]
        secret_ids: Vec<String>,
    },
    Import {
        #[arg(long, required = true)]
        secret_id: String,

        #[arg(long, default_value = "None")]
        algorithm: String,

        #[arg(long)]
        extractable: bool,

        #[arg(long)]
        overwrite: bool,

        #[arg(long, required = true, value_parser = parse_hex_bytes, action = clap::ArgAction::Set)]
        data: HexBytes,
    },
    Generate {
        #[arg(long, required = true)]
        secret_id: String,

        #[arg(long, required = true)]
        algorithm: String,

        #[arg(long)]
        extractable: bool,
    },
    Get {
        #[arg(long, required = true)]
        secret_id: String,
    },
    Sign {
        #[arg(long, required = true)]
        secret_id: String,

        #[arg(long, required = true, value_parser = parse_hex_bytes, action = clap::ArgAction::Set)]
        data: HexBytes,
    },
    Verify {
        #[arg(long, required = true)]
        secret_id: String,

        #[arg(long, required = true, value_parser = parse_hex_bytes, action = clap::ArgAction::Set)]
        data: HexBytes,

        #[arg(long, required = true, value_parser = parse_hex_bytes, action = clap::ArgAction::Set)]
        signature: HexBytes,
    },
    Migrate {},
    CopyFileToHashicorp {
        /// Conflict policy when destination already has a secret with the same id
        #[arg(long, default_value = "fail")]
        on_conflict: String,

        /// Print plan without writing to destination
        #[arg(long)]
        dry_run: bool,

        /// Continue on per-secret errors instead of aborting
        #[arg(long)]
        continue_on_error: bool,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init {} => init::execute().await,
        Commands::List { full, show_private } => list::execute(full, show_private).await,
        Commands::Delete { secret_ids } => delete::execute(&secret_ids).await,
        Commands::Import { secret_id, algorithm, extractable, overwrite, data } => {
            let algo: Algorithm = match algorithm.parse() {
                Ok(algo) => algo,
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            };

            import::execute(&secret_id, data.0.as_slice(), algo, extractable, overwrite).await
        }
        Commands::Generate { secret_id, algorithm, extractable } => {
            let algo: Algorithm = match algorithm.parse() {
                Ok(algo) => algo,
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            };

            generate::execute(&secret_id, algo, extractable).await
        }
        Commands::Get { secret_id } => get::execute(&secret_id).await,
        Commands::Sign { secret_id, data } => sign::execute(&secret_id, data.0.as_slice()).await,
        Commands::Verify { secret_id, data, signature } => {
            verify::execute(&secret_id, data.0.as_slice(), signature.0.as_slice()).await
        }
        Commands::Migrate {} => migrate::execute().await,
        Commands::CopyFileToHashicorp { on_conflict, dry_run, continue_on_error } => {
            let on_conflict: copy_file_to_hashicorp::OnConflict = match on_conflict.parse() {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            };
            copy_file_to_hashicorp::execute(on_conflict, dry_run, continue_on_error).await
        }
    };

    if let Err(e) = result {
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}
