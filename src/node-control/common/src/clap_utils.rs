/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub fn parse_hex_any(s: &str) -> Result<String, String> {
    let s2 = strip_0x(s);

    if s2.is_empty() {
        return Err("hex string cannot be empty".into());
    }
    if s2.len() % 2 != 0 {
        return Err("hex must have an even number of characters (2 per byte)".into());
    }
    if !s2.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("hex may contain only 0-9, a-f, A-F".into());
    }

    Ok(s.to_owned())
}

pub fn parse_hex_32_bytes(s: &str) -> Result<String, String> {
    // validate general hex rules first
    let validated = parse_hex_any(s)?;
    let body = strip_0x(&validated);

    // 32 bytes => 64 hex chars
    if body.len() != 64 {
        return Err(format!("expected 32 bytes (64 hex chars), got {}", body.len()));
    }

    Ok(validated)
}

#[inline]
fn strip_0x(s: &str) -> &str {
    s.strip_prefix("0x").unwrap_or(s)
}
