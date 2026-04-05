/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub fn nanotons_to_dec_string(value: u64) -> String {
    value.to_string()
}

pub fn tons_f64_to_nanotons(tons: f64) -> u64 {
    (tons * 1_000_000_000.0).round() as u64
}

pub fn nanotons_to_tons_f64(nanotons: u64) -> f64 {
    nanotons as f64 / 1_000_000_000.0
}

pub fn display_tons(nanotons: u64) -> String {
    format!("{:.4}", nanotons_to_tons_f64(nanotons))
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// Parse a nanotons decimal string and format as TON (4 decimal places).
/// Returns the original string if it cannot be parsed.
pub fn display_tons_from_str(nanotons_str: &str) -> String {
    nanotons_str
        .trim()
        .parse::<u64>()
        .map(display_tons)
        .unwrap_or_else(|_| nanotons_str.to_string())
}

#[cfg(test)]
mod tests {
    use super::display_tons;
    #[test]
    fn test_display_tons() {
        assert_eq!(display_tons(0_100_000_000), "0.1");
        assert_eq!(display_tons(1_000_000_000), "1");
        assert_eq!(display_tons(1_100_000_000), "1.1");
        assert_eq!(display_tons(1_100_100_000), "1.1001");
        assert_eq!(display_tons(1_100_010_000), "1.1");
        assert_eq!(display_tons(123_000_000_000), "123");
        assert_eq!(display_tons(123_450_000_000), "123.45");
        assert_eq!(display_tons(123_000_100_000), "123.0001");
        assert_eq!(display_tons(123_000_180_000), "123.0002");
    }
}
