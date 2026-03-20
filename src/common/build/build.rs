/*
 * Copyright 2018-2022 TON DEV SOLUTIONS LTD.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the Apache License, Version 2.0.
 * See the common/LICENSE file in this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::process::Command;

fn get_value(cmd: &str, args: &[&str]) -> String {
    if let Ok(result) = Command::new(cmd).args(args).output() {
        if let Ok(result) = String::from_utf8(result.stdout) {
            return result;
        }
    }
    "Unknown".to_string()
}

fn get_env_or_command(env_var: &str, cmd: &str, args: &[&str]) -> String {
    std::env::var(env_var).unwrap_or_else(|_| get_value(cmd, args))
}

fn main() {
    let git_branch =
        get_env_or_command("GIT_BRANCH", "git", &["rev-parse", "--abbrev-ref", "HEAD"]);
    let git_commit = get_env_or_command("GIT_COMMIT", "git", &["rev-parse", "HEAD"]);
    let commit_date = get_env_or_command(
        "GIT_COMMIT_DATE",
        "git",
        &["log", "-1", "--date=iso", "--pretty=format:%cd"],
    );
    let build_time = get_value("date", &["+%Y-%m-%d %T %z"]);
    let rust_version = get_value("rustc", &["--version"]);

    println!("cargo:rustc-env=BUILD_GIT_BRANCH={}", git_branch);
    println!("cargo:rustc-env=BUILD_GIT_COMMIT={}", git_commit);
    println!("cargo:rustc-env=BUILD_GIT_DATE={}", commit_date);
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);
    println!("cargo:rustc-env=BUILD_RUST_VERSION={}", rust_version);
}
