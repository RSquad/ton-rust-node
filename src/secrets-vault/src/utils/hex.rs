/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner};

#[inline]
fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

pub fn hex_decode(input_hex_str: &[u8], output: &mut [u8]) -> anyhow::Result<()> {
    let in_len = input_hex_str.len();

    if in_len < 2 || !in_len.is_multiple_of(2) {
        anyhow::bail!("Size of input hex string must be even and length is >= 2");
    }
    if output.len() < (in_len / 2) {
        anyhow::bail!("output buffer too small");
    }

    for (i, pair) in input_hex_str.chunks_exact(2).enumerate() {
        let hi = hex_val(pair[0]).ok_or_else(|| {
            anyhow::anyhow!("invalid hex char at index {}: 0x{:02x}", 2 * i, pair[0])
        })?;
        let lo = hex_val(pair[1]).ok_or_else(|| {
            anyhow::anyhow!("invalid hex char at index {}: 0x{:02x}", 2 * i + 1, pair[1])
        })?;

        output[i] = (hi << 4) | lo;
    }

    Ok(())
}

pub fn hex_val_to_pm(name: &str, val: &str) -> anyhow::Result<ProtectedMemory> {
    let hex_data = val_to_pm(name, val)?;
    let mut data = ProtectedMemoryInner::new(hex_data.len() / 2)?;
    {
        let mut handle = data.write_handle()?;
        hex_decode(hex_data.lock()?.as_ref(), handle.as_mut())?;
    }
    Ok(data.into())
}

pub fn val_to_pm(name: &str, val: &str) -> anyhow::Result<ProtectedMemory> {
    if val.is_empty() {
        anyhow::bail!("value '{}' is empty", name);
    }

    Ok(ProtectedMemoryInner::from_slice(val.as_bytes())?.into())
}
