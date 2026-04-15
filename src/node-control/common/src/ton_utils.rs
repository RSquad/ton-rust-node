/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::str::FromStr;
use ton_block::{ConfigParamEnum, MsgAddressInt};

pub fn nanotons_to_dec_string(value: u64) -> String {
    value.to_string()
}

pub fn tons_f64_to_nanotons(tons: f64) -> u64 {
    (tons * 1_000_000_000.0).round() as u64
}

pub fn nanotons_to_tons_f64(nanotons: u64) -> f64 {
    nanotons as f64 / 1_000_000_000.0
}

/// Elector uses fixed-point `max_stake_factor`: raw value is multiplier × 65536 (e.g. 3× → `3 * 65536`).
pub const MAX_STAKE_FACTOR_SCALE: f32 = 65536.0;

/// Converts chain `max_stake_factor` (raw) to float multiplier (e.g. `196608` → `3.0`).
#[inline]
pub fn max_stake_factor_raw_to_multiplier(raw: u32) -> f32 {
    raw as f32 / MAX_STAKE_FACTOR_SCALE
}

/// Extracts the network `max_factor` from a `ConfigParamEnum` (must be param 17; field `max_stake_factor`) as a float multiplier.
pub fn extract_max_factor(param: ConfigParamEnum) -> anyhow::Result<f32> {
    match param {
        ConfigParamEnum::ConfigParam17(c) => {
            Ok(max_stake_factor_raw_to_multiplier(c.max_stake_factor))
        }
        _ => anyhow::bail!("expected config param 17 (stakes config)"),
    }
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

/// Trims surrounding whitespace and validates that `addr` is a well-formed TON address
/// (raw `0:<hex>` or base64url bounceable). Returns the trimmed string on success.
///
/// `field_name` is used only to produce a clear error message (e.g. `"address"`, `"owner"`).
pub fn normalize_ton_address(addr: &str, field_name: &str) -> anyhow::Result<String> {
    let trimmed = addr.trim();
    if trimmed.is_empty() {
        anyhow::bail!("{field_name} must not be empty");
    }
    MsgAddressInt::from_str(trimmed).map_err(|_| {
        anyhow::anyhow!(
            "invalid TON address for {field_name}: '{trimmed}'. Expected format: raw address or base64url"
        )
    })?;
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{display_tons, normalize_ton_address};

    #[test]
    fn test_normalize_ton_address_valid_raw() {
        assert_eq!(
            normalize_ton_address(
                "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb",
                "owner",
            )
            .unwrap(),
            "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb"
        );
    }

    #[test]
    fn test_normalize_ton_address_trims_spaces() {
        assert_eq!(
            normalize_ton_address(
                "  0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb  ",
                "owner",
            )
            .unwrap(),
            "0:c5770dc489bef32419959c174b787ab95ff9109e0e43239c18059509819697fb"
        );
    }

    #[test]
    fn test_normalize_ton_address_empty() {
        let err = normalize_ton_address("   ", "owner").unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn test_normalize_ton_address_invalid() {
        let err = normalize_ton_address("not-an-address", "owner").unwrap_err();
        assert!(err.to_string().contains("invalid TON address"));
    }

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
