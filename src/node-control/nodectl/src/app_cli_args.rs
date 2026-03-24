/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use commands::commands::cli_cmd::Commands as CliCommands;
use std::sync::OnceLock;

static CLI_ARGS: OnceLock<AppCliArgs> = OnceLock::new();

fn long_version() -> &'static str {
    concat!(
        env!("CARGO_PKG_VERSION"),
        "\nCOMMIT_ID: ",
        env!("BUILD_GIT_COMMIT"),
        "\nBUILD_DATE: ",
        env!("BUILD_TIME"),
        "\nCOMMIT_DATE: ",
        env!("BUILD_GIT_DATE"),
        "\nGIT_BRANCH: ",
        env!("BUILD_GIT_BRANCH"),
        "\nRUST_VERSION: ",
        env!("BUILD_RUST_VERSION"),
    )
}

#[derive(clap::Parser, Clone)]
#[command(author, version, about, long_about, long_version = long_version())]
pub struct AppCliArgs {
    #[command(subcommand)]
    pub command: Option<CliCommands>,
}

impl AppCliArgs {
    pub fn parse() -> anyhow::Result<&'static Self> {
        let args = <Self as clap::Parser>::parse();
        CLI_ARGS.set(args).map_err(|_| anyhow::anyhow!("CLI args already initialized"))?;
        Ok(CLI_ARGS.get().unwrap())
    }

    pub fn print_help() {
        let mut cmd = <Self as clap::CommandFactory>::command();
        cmd.print_help().expect("failed to print help");
        println!();
    }
}
