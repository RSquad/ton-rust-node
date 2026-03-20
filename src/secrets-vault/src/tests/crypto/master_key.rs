/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::crypto::master_key::MasterKey;

macro_rules! with_env_var {
    ($var_name:expr, $var_value:expr, $test_body:expr) => {{
        unsafe {
            std::env::set_var($var_name, $var_value);
        }

        let result = $test_body;

        unsafe {
            std::env::remove_var($var_name);
        }

        result
    }};
}

#[tokio::test]
async fn test_from_config_environment_valid_key() {
    let test_key = "abcdef00000000000011223344556677889900112233445566778899001122ff";
    let var_name = "SECRET_VAULT_MASTER_KEY";

    let result =
        with_env_var!(var_name, test_key, async { MasterKey::from_env(var_name).await }.await);

    assert!(result.is_ok(), "Should successfully create MasterKey from valid environment variable");
}

#[tokio::test]
async fn test_from_config_environment_missing_var() {
    let var_name = "NONEXISTENT_MASTER_KEY_VAR";

    // Ensure the variable doesn't exist
    unsafe {
        std::env::remove_var(var_name);
    }

    let result = MasterKey::from_env(var_name).await;

    assert!(result.is_err(), "Should fail when environment variable is missing");
}

#[tokio::test]
async fn test_from_config_environment_invalid_hex() {
    let test_key = "ghijkl00000000000011223344556677889900112233445566778899001122ff"; // Invalid hex
    let var_name = "SECRET_VAULT_MASTER_KEY_INVALID";

    let result =
        with_env_var!(var_name, test_key, async { MasterKey::from_env(var_name).await }.await);

    assert!(result.is_err(), "Should fail when key contains invalid hexadecimal characters");
}
