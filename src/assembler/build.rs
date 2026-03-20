/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::process::Command;

fn main() {
    let mut git_branch = String::from("Unknown");
    let mut git_commit = String::from("Unknown");
    let mut commit_date = String::from("Unknown");
    let mut build_time = String::from("Unknown");

    let branch = Command::new("git").args(["rev-parse", "--abbrev-ref", "HEAD"]).output();

    if let Ok(branch) = branch {
        git_branch = String::from_utf8(branch.stdout).unwrap_or_else(|_| "Unknown".to_string());
    }

    let last = Command::new("git").args(["rev-parse", "HEAD"]).output();
    if let Ok(last) = last {
        git_commit = String::from_utf8(last.stdout).unwrap_or_else(|_| "Unknown".to_string());
    }

    let time =
        Command::new("git").args(["log", "-1", "--date=iso", "--pretty=format:%cd"]).output();
    if let Ok(time) = time {
        commit_date = String::from_utf8(time.stdout).unwrap_or_else(|_| "Unknown".to_string());
    }

    let b_time = Command::new("date").args(["+%Y-%m-%d %T %z"]).output();
    if let Ok(b_time) = b_time {
        build_time = String::from_utf8(b_time.stdout).unwrap_or_else(|_| "Unknown".to_string());
    }

    println!("cargo:rustc-env=BUILD_GIT_BRANCH={}", git_branch);
    println!("cargo:rustc-env=BUILD_GIT_COMMIT={}", git_commit);
    println!("cargo:rustc-env=BUILD_GIT_DATE={}", commit_date);
    println!("cargo:rustc-env=BUILD_TIME={}", build_time);
}
