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
use colored::Colorize;
use std::io::Write;
use validator_session::*;

include!("../../../common/src/info.rs");

fn init_logger() {
    if !is_test_logging_enabled() {
        return;
    }

    env_logger::Builder::new()
        .format(move |buf, record| {
            let message = format!("{}", record.args());
            let level = format!("{}", record.level());
            let line = match record.line() {
                Some(line) => format!("({})", line),
                None => "".to_string(),
            };
            let source = format!("{}{}", record.target(), line);
            let thread_name = {
                let current_thread = std::thread::current();

                if let Some(name) = current_thread.name() {
                    name.to_string()
                } else {
                    let id = current_thread.id();
                    format!("#{:?}", id).replace("ThreadId(", "").replace(")", "")
                }
            };

            let (message, level) = match record.level() {
                log::Level::Error => (message.red(), level.red()),
                log::Level::Warn => (message.yellow(), level.yellow()),
                log::Level::Trace => (message.dimmed(), level.dimmed()),
                log::Level::Info => {
                    if record.target() == module_path!() {
                        (message.bright_green().bold(), level.bright_green().bold())
                    } else {
                        (message.bright_white().bold(), level.bright_white().bold())
                    }
                }
                _ => (message.normal(), level.normal()),
            };

            let (message, level) = if thread_name == "VS2" {
                (message.bright_green().bold(), level.bright_green().bold())
            } else {
                (message, level)
            };

            match record.level() {
              log::Level::Trace /*| log::Level::Debug*/ => Ok(()),
              _ => {
                  writeln!(
                      buf,
                      "{} [{: <5}] - {: <5} - {: <45}| {}",
                      chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%f"),
                      level,
                      thread_name,
                      source,
                      message
                  )?;

                  std::io::stdout().flush()
              }
          }
        })
        .filter(None, log::LevelFilter::Trace)
        .init();
}

#[test]
fn test_state_hashes() {
    init_logger();
    let options = SessionOptions::default();
    let total_nodes: u32 = 100;
    crate::tests::test_state_hashes_part1(&options, total_nodes);
    crate::tests::test_state_hashes_part2(&options, total_nodes);
}
