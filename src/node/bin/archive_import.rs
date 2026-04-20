/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use clap::{Arg, ArgAction, Command};
use node::archive_import::{run_import, ImportConfig};
use std::path::PathBuf;

fn main() {
    env_logger::Builder::from_default_env().format_timestamp_millis().init();

    let matches = Command::new("archive_import")
        .about("Import raw .pack archive files into epoch-based storage")
        .arg(
            Arg::new("archives-path")
                .long("archives-path")
                .required(true)
                .help("Path to directory with source .pack files"),
        )
        .arg(
            Arg::new("epochs-path")
                .long("epochs-path")
                .required(true)
                .help("Path where epoch directories will be created"),
        )
        .arg(
            Arg::new("epoch-size")
                .long("epoch-size")
                .default_value("10000000")
                .help("Number of MC blocks per epoch (must be multiple of 20000)"),
        )
        .arg(
            Arg::new("node-db-path")
                .long("node-db-path")
                .required(true)
                .help("Path to node database directory"),
        )
        .arg(
            Arg::new("mc-zerostate")
                .long("mc-zerostate")
                .required(true)
                .help("Path to masterchain zerostate .boc file"),
        )
        .arg(
            Arg::new("wc-zerostate")
                .long("wc-zerostate")
                .action(ArgAction::Append)
                .required(true)
                .help("Path to workchain zerostate .boc file (one per workchain)"),
        )
        .arg(
            Arg::new("global-config")
                .long("global-config")
                .required(true)
                .help("Path to global config JSON file (describes zerostate and hard forks)"),
        )
        .arg(
            Arg::new("skip-validation")
                .long("skip-validation")
                .action(ArgAction::SetTrue)
                .help("Skip block proof validation (for re-importing already validated archives)"),
        )
        .arg(Arg::new("copy").long("copy").action(ArgAction::SetTrue).help(
            "Copy source .pack files instead of moving them. Use for keeping original \
                    files or when source and destination are on different filesystems.",
        ))
        .get_matches();

    let config = ImportConfig {
        archives_path: PathBuf::from(matches.get_one::<String>("archives-path").unwrap()),
        epochs_path: PathBuf::from(matches.get_one::<String>("epochs-path").unwrap()),
        epoch_size: matches
            .get_one::<String>("epoch-size")
            .unwrap()
            .parse()
            .expect("epoch-size must be a number"),
        node_db_path: PathBuf::from(matches.get_one::<String>("node-db-path").unwrap()),
        mc_zerostate_path: PathBuf::from(matches.get_one::<String>("mc-zerostate").unwrap()),
        wc_zerostate_paths: matches
            .get_many::<String>("wc-zerostate")
            .unwrap()
            .map(|s| PathBuf::from(s))
            .collect(),
        global_config_path: PathBuf::from(matches.get_one::<String>("global-config").unwrap()),
        skip_validation: matches.get_flag("skip-validation"),
        move_files: !matches.get_flag("copy"),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");

    if let Err(e) = rt.block_on(run_import(config)) {
        log::error!("Import failed: {}", e);
        std::process::exit(1);
    }
}
