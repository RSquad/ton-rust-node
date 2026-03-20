/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    errors::error::VaultError,
    events::null_handler::NullEventHandler,
    tests::fixture::*,
    types::{algorithm::Algorithm, secret_spec::SecretSpec},
    vault_builder::SecretVaultBuilder,
};
use std::sync::Arc;

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
#[serial_test::serial]
async fn test_getters_initial_state() {
    let builder = SecretVaultBuilder::default();

    assert!(builder.storage().is_none());
    assert!(builder.event_handler().is_none());
}

#[tokio::test]
#[serial_test::serial]
pub async fn test_getter_crypto() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let builder = SecretVaultBuilder::default().with_storage(storage);

        assert!(builder.storage().is_some());
        assert!(builder.event_handler().is_none());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
pub async fn test_builder_with_all_components() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let event_handler = Arc::new(NullEventHandler);
        let builder =
            SecretVaultBuilder::default().with_storage(storage).with_event_handler(event_handler);

        assert!(builder.storage().is_some());
        assert!(builder.event_handler().is_some());

        let vault = builder.build().await?;
        let secret_id = "all_components_test".into();
        let spec = SecretSpec::new(Algorithm::Ed25519);
        vault.generate_secret(&spec, &secret_id).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_valid_file_scheme() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) = create_url(config.storage_type, None::<fn(String) -> String>)?;
        let vault = SecretVaultBuilder::from_url(&url).await?;
        clear_vault(vault.as_ref()).await?;
        let ed_id = "test_secret_ed25519".into();
        let ed_spec = SecretSpec::new(Algorithm::Ed25519);
        let ed1 = vault.generate_secret(&ed_spec, &ed_id).await?;
        let ed2 = vault.get(&ed_id).await?;
        assert!(ed1.eq_secret(&ed2).await?);

        let sym_id = "test_secret_blob".into();
        let sym_spec = SecretSpec::new(Algorithm::None).size(32);
        let sym1 = vault.generate_secret(&sym_spec, &sym_id).await?;
        let sym2 = vault.get(&sym_id).await?;
        assert!(sym1.eq_secret(&sym2).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_with_whitespace() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) = create_url(config.storage_type, Some(|e| format!("   {}   ", e)))?;
        let vault = SecretVaultBuilder::from_url(&url).await?;
        clear_vault(vault.as_ref()).await?;
        let secret_id = "whitespace_test".into();
        let spec = SecretSpec::new(Algorithm::Ed25519);
        vault.generate_secret(&spec, &secret_id).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_case_insensitive_scheme() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) = create_url(
            config.storage_type,
            Some::<fn(String) -> String>(|e| {
                e.replace("file", "FILE").replace("hashicorp", "HASHICORP")
            }),
        )?;
        let vault = SecretVaultBuilder::from_url(&url).await?;
        clear_vault(vault.as_ref()).await?;
        let secret_id = "case_test".into();
        let spec = SecretSpec::new(Algorithm::Ed25519);
        vault.generate_secret(&spec, &secret_id).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_case_insensitive_master_key_param() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) = create_url(
            config.storage_type,
            Some::<fn(String) -> String>(|e| {
                e.replace("master_key", "MASTER_KEY").replace("api_key", "API_KEY")
            }),
        )?;
        let vault = SecretVaultBuilder::from_url(&url).await?;
        clear_vault(vault.as_ref()).await?;
        let secret_id = "param_case_test".into();
        let spec = SecretSpec::new(Algorithm::Ed25519);
        vault.generate_secret(&spec, &secret_id).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_multiple_query_params() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) = create_url(
            config.storage_type,
            Some::<fn(String) -> String>(|e| format!("{}&foo=d&baz=x", e)),
        )?;
        SecretVaultBuilder::from_url(&url).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_empty_query_segments() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) =
            create_url(config.storage_type, Some::<fn(String) -> String>(|e| format!("{}&&", e)))?;
        let vault = SecretVaultBuilder::from_url(&url).await?;
        clear_vault(vault.as_ref()).await?;
        let secret_id = "empty_segments_test".into();
        let spec = SecretSpec::new(Algorithm::Ed25519);
        vault.generate_secret(&spec, &secret_id).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_nested_path() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let master_key_hex = "abcdef00000000000011223344556677889900112233445566778899001122ff";
    let nested_path = temp_dir.path().join("subdir/nested/vault.json");
    let url = format!("file://{}?master_key={}", nested_path.to_str().unwrap(), master_key_hex);
    let vault = SecretVaultBuilder::from_url(&url).await?;
    let secret_id = "nested_path_test".into();
    let spec = SecretSpec::new(Algorithm::Ed25519);
    vault.generate_secret(&spec, &secret_id).await?;

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_missing_delimiter() -> anyhow::Result<()> {
    let url = "filevault.json?master_key=abcdef";
    let result = SecretVaultBuilder::from_url(url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_unsupported_scheme() -> anyhow::Result<()> {
    let master_key_hex = "abcdef00000000000011223344556677889900112233445566778899001122ff";
    let url = format!("http://vault.json?master_key={}", master_key_hex);
    let result = SecretVaultBuilder::from_url(&url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_missing_path() -> anyhow::Result<()> {
    let master_key_hex = "abcdef00000000000011223344556677889900112233445566778899001122ff";
    let url = format!("file://?master_key={}", master_key_hex);
    let result = SecretVaultBuilder::from_url(&url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_empty_path() -> anyhow::Result<()> {
    let master_key_hex = "abcdef00000000000011223344556677889900112233445566778899001122ff";
    let url = format!("file://   ?master_key={}", master_key_hex);
    let result = SecretVaultBuilder::from_url(&url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_missing_master_key_param() -> anyhow::Result<()> {
    let url = "file://vault.json?foo=bar";
    let result = SecretVaultBuilder::from_url(url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_no_query_string() -> anyhow::Result<()> {
    let url = "file://vault.json";
    let result = SecretVaultBuilder::from_url(url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_invalid_query_param_format() -> anyhow::Result<()> {
    let url = "file://vault.json?invalid_param";
    let result = SecretVaultBuilder::from_url(url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_invalid_hex_master_key() -> anyhow::Result<()> {
    let url = "file://vault.json?master_key=not_valid_hex";
    let result = SecretVaultBuilder::from_url(url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_empty_master_key_value() -> anyhow::Result<()> {
    let url = "file://vault.json?master_key=";
    let result = SecretVaultBuilder::from_url(url).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_env_variable_full_url() -> anyhow::Result<()> {
    for config in fixture() {
        let (url, _tmp) = create_url(config.storage_type, None::<fn(String) -> String>)?;

        let vault_res: anyhow::Result<Arc<crate::vault::SecretVault>> = with_env_var!(
            "VAULT_URL",
            &url,
            async {
                let url = SecretVaultBuilder::read_url_from_env()?
                    .ok_or_else(|| VaultError::invalid_config_url(""))?;
                let vault = SecretVaultBuilder::from_url(&url).await?;
                clear_vault(vault.as_ref()).await?;
                Ok(vault)
            }
            .await
        );

        let secret_id = "env_url_test".into();
        let spec = SecretSpec::new(Algorithm::Ed25519);
        vault_res?.generate_secret(&spec, &secret_id).await?;
    }
    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_from_url_env_variable_not_set() -> anyhow::Result<()> {
    unsafe {
        std::env::remove_var("VAULT_URL");
    }

    let result = SecretVaultBuilder::read_url_from_env()?;
    assert!(result.is_none());

    Ok(())
}
