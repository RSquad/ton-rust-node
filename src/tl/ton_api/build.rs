/*
 * Copyright (C) 2019-2022 TON Labs. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};
use ton_tl_codegen::Config;

fn get_value(cmd: &str, args: &[&str]) -> String {
    if let Ok(result) = Command::new(cmd).args(args).output() {
        if let Ok(result) = String::from_utf8(result.stdout) {
            return result;
        }
    }
    "Unknown".to_string()
}

const OUTPUT_DIR: &str = "src/ton";
const TL_DIR: &str = "tl";
const TL_FILES: [&str; 3] = ["lite_api.tl", "ton_api.tl", "tonlib_api.tl"];

fn main() {
    // TODO: This line was commented because of different behavior of cargo in rust ver 1.50.
    //       We can revert it, when this behavior is fixed.
    // println!("cargo:rerun-if-changed={}", OUTPUT_DIR);
    println!("cargo:rerun-if-changed={}", TL_DIR);
    println!("cargo:rerun-if-changed=../ton_tl_codegen");

    let mut files: Vec<PathBuf> =
        TL_FILES.iter().map(|file| Path::new(TL_DIR).join(file)).collect();
    files.sort();

    let mut input = String::new();
    for file in files {
        if !input.is_empty() {
            input += "---types---\n";
        }
        fs::File::open(&file)
            .unwrap_or_else(|_| {
                panic!("Unable to open file for reading: {}", file.to_string_lossy())
            })
            .read_to_string(&mut input)
            .unwrap_or_else(|_| panic!("Unable to read file contents: {}", file.to_string_lossy()));
        println!("cargo:rerun-if-changed={}", file.to_string_lossy());
    }

    let config_path = Path::new(TL_DIR).join("codegen.json");
    let config: Option<Config> = if config_path.exists() && config_path.is_file() {
        let mut config_string = String::new();
        fs::File::open(&config_path)
            .unwrap_or_else(|_| {
                panic!("Unable to open file for reading: {}", config_path.to_string_lossy())
            })
            .read_to_string(&mut config_string)
            .unwrap_or_else(|_| {
                panic!("Unable to read file contents: {}", config_path.to_string_lossy())
            });
        Some(serde_json::from_str(&config_string).unwrap_or_else(|_| {
            panic!("Unable to parse file as JSON: {}", config_path.to_string_lossy())
        }))
    } else {
        None
    };

    ton_tl_codegen::generate_code_for(config, &input, Path::new(OUTPUT_DIR));

    let git_branch = get_value("git", &["rev-parse", "--abbrev-ref", "HEAD"]);
    let git_commit = get_value("git", &["rev-parse", "HEAD"]);
    let commit_date = get_value("git", &["log", "-1", "--date=iso", "--pretty=format:%cd"]);
    let build_time = get_value("date", &["+%Y-%m-%d %T %z"]);
    let rust_version = get_value("rustc", &["--version"]);

    println!("cargo:rustc-env=BUILD_GIT_BRANCH={}", git_branch);
    println!("cargo:rustc-env=BUILD_GIT_COMMIT={}", git_commit);
    println!("cargo:rustc-env=BUILD_GIT_DATE={}", commit_date);
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);
    println!("cargo:rustc-env=BUILD_RUST_VERSION={}", rust_version);
}
