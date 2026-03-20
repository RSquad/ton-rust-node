/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
fn assert_json_eq(json: &str, expected: &str, name: &str) {
    let expected = expected.replace("\r", "");
    let expected = if let Some(expected) = expected.strip_suffix("\n") {
        expected.to_string()
    } else {
        expected
    };
    if json != expected {
        std::fs::write(format!("../target/{}.json", name), json).unwrap();
        pretty_assertions::assert_eq!(json, expected, "JSON mismatch for {}", name);
        panic!("json != expected")
    }
}
