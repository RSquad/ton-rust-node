use crate::{
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    storage::{
        hashicorp_api::{Client, VaultConfig},
        hashicorp_token_provider::AuthConfig,
    },
};

fn test_client(transit_mount: &str, kv_mount: &str, kv_prefix: Option<&str>) -> Client {
    let pm: ProtectedMemory = ProtectedMemoryInner::from_slice(b"test").unwrap().into();
    Client::new(
        "http://vault:8200",
        AuthConfig::StaticToken(pm),
        VaultConfig {
            transit_mount: transit_mount.to_string(),
            kv_mount: kv_mount.to_string(),
            kv_prefix: kv_prefix.map(|s| s.to_string()),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn kv_data_path_defaults() {
    let c = test_client("transit", "secret", None);
    assert_eq!(c.kv_data_path("blobs/mykey"), "http://vault:8200/v1/secret/data/blobs/mykey");
}

#[test]
fn kv_data_path_custom_mount() {
    let c = test_client("transit", "ton", None);
    assert_eq!(c.kv_data_path("blobs/mykey"), "http://vault:8200/v1/ton/data/blobs/mykey");
}

#[test]
fn kv_data_path_with_prefix() {
    let c = test_client("transit", "ton", Some("mainnet"));
    assert_eq!(c.kv_data_path("blobs/mykey"), "http://vault:8200/v1/ton/data/mainnet/blobs/mykey");
}

#[test]
fn kv_data_path_nested_prefix() {
    let c = test_client("transit", "ton", Some("mainnet/validator-0"));
    assert_eq!(
        c.kv_data_path("blobs/k"),
        "http://vault:8200/v1/ton/data/mainnet/validator-0/blobs/k"
    );
}

#[test]
fn kv_meta_path_defaults() {
    let c = test_client("transit", "secret", None);
    assert_eq!(c.kv_meta_path("blobs"), "http://vault:8200/v1/secret/metadata/blobs");
}

#[test]
fn kv_meta_path_with_prefix() {
    let c = test_client("transit", "ton", Some("mainnet"));
    assert_eq!(c.kv_meta_path("blobs/k"), "http://vault:8200/v1/ton/metadata/mainnet/blobs/k");
}

#[test]
fn transit_mount_custom() {
    let c = test_client("ton-transit", "secret", None);
    let url = c.transit_mount_path("keys", Some("mykey"));
    assert_eq!(url, "http://vault:8200/v1/ton-transit/keys/mykey");
}

#[test]
fn url_escape() {
    let escaped = Client::escape("GEpySlMQ/GeNYV+fY27s0Z3gxniz1eAKBlWrbPcl3b4=");
    assert_eq!(escaped, "GEpySlMQ_GeNYV-fY27s0Z3gxniz1eAKBlWrbPcl3b4");
}
