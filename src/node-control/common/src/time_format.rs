/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::UnixTime;

pub fn format_ts(timestamp: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(timestamp as i64, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| format!("{}", timestamp))
}

pub fn since_now(timestamp: u64) -> String {
    let now = UnixTime::now();
    if timestamp <= now {
        return "0s".to_string();
    }
    let duration = timestamp - now;
    let hours = (duration % 86400) / 3600;
    let minutes = (duration % 3600) / 60;
    let seconds = duration % 60;
    format!("{}h {}m {}s", hours, minutes, seconds)
}

pub fn now() -> u64 {
    UnixTime::now()
}
