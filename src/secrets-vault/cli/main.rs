/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

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
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init {} => init::execute().await,
        Commands::List { full } => list::execute(full).await,
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
    };

    if let Err(e) = result {
        eprintln!("{} {}", "Error:".red().bold(), e);
        std::process::exit(1);
    }
}
