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
