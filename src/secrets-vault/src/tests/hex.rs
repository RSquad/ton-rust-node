/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::utils::hex::hex_decode;

#[test]
fn test_decode_simple_hex() -> anyhow::Result<()> {
    let input = b"1234";
    let mut output = vec![0u8; 2];

    hex_decode(input, &mut output)?;
    assert_eq!(&output[..], &[0x12, 0x34]);

    Ok(())
}

#[test]
fn test_decode_two_chars() -> anyhow::Result<()> {
    let input = b"ab";
    let mut output = vec![0u8; 1];

    hex_decode(input, &mut output)?;
    assert_eq!(output[0], 0xAB);

    Ok(())
}

#[test]
fn test_decode_all_lowercase() -> anyhow::Result<()> {
    let input = b"0123456789abcdef";
    let mut output = vec![0u8; 8];

    hex_decode(input, &mut output)?;
    assert_eq!(&output[..], &[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);

    Ok(())
}

#[test]
fn test_decode_all_uppercase() -> anyhow::Result<()> {
    let input = b"0123456789ABCDEF";
    let mut output = vec![0u8; 8];

    hex_decode(input, &mut output)?;
    assert_eq!(&output[..], &[0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF]);

    Ok(())
}

#[test]
fn test_decode_mixed_case() -> anyhow::Result<()> {
    let input = b"aAbBcCdDeEfF";
    let mut output = vec![0u8; 6];

    hex_decode(input, &mut output)?;
    assert_eq!(&output[..], &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);

    Ok(())
}

#[test]
fn test_decode_empty_input() {
    let input = b"";
    let mut output = vec![];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must be even"));
}

#[test]
fn test_decode_single_char() {
    let input = b"a";
    let mut output = vec![0u8; 1];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must be even"));
}

#[test]
fn test_decode_odd_length() {
    let input = b"abc";
    let mut output = vec![0u8; 2];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("must be even"));
}

#[test]
fn test_decode_output_too_small() {
    let input = b"1234";
    let mut output = vec![0u8; 1];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("too small"));
}

#[test]
fn test_decode_larger_output() -> anyhow::Result<()> {
    let input = b"1234";
    let mut output = vec![0u8; 5];

    hex_decode(input, &mut output)?;
    assert_eq!(&output[0..2], &[0x12, 0x34]);
    assert_eq!(&output[2..], &[0, 0, 0]);

    Ok(())
}

#[test]
fn test_decode_invalid_char_first_position() {
    let input = b"g0";
    let mut output = vec![0u8; 1];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid hex char"));
    assert!(err.contains("index 0"));
}

#[test]
fn test_decode_invalid_char_middle() {
    let input = b"12g4";
    let mut output = vec![0u8; 2];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("invalid hex char"));
    assert!(err.contains("index 2"));
}

#[test]
fn test_decode_null_byte() {
    let input = &[b'1', b'2', 0x00, b'4'];
    let mut output = vec![0u8; 2];

    let result = hex_decode(input, &mut output);
    assert!(result.is_err());
}

#[test]
fn test_decode_large_hex_string() -> anyhow::Result<()> {
    let hex_str = "42".repeat(1000);
    let input = hex_str.as_bytes();
    let mut output = vec![0u8; 1000];

    hex_decode(input, &mut output)?;
    for &byte in &output {
        assert_eq!(byte, 0x42);
    }

    Ok(())
}

#[test]
fn test_decode_all_byte_values() -> anyhow::Result<()> {
    let mut hex_str = String::new();
    for i in 0..=255u8 {
        hex_str.push_str(&format!("{:02x}", i));
    }

    let input = hex_str.as_bytes();
    let mut output = vec![0u8; 256];

    hex_decode(input, &mut output)?;

    for (i, &byte) in output.iter().enumerate() {
        assert_eq!(byte, i as u8);
    }

    Ok(())
}
