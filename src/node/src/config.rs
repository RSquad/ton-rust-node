/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::network::{
    liteserver::{LiteServerConfig, LiteServerConfigJson},
    node_network::NodeNetwork,
};
use adnl::{
    client::AdnlClientConfigJson,
    common::{add_unbound_object_to_map_with_update, Wait},
    node::{AdnlNodeConfig, AdnlNodeConfigJson},
    server::{AdnlServerConfig, AdnlServerConfigJson},
};
use secrets_vault::{
    crypto::factory::{AutoCryptoFactory, CryptoFactory},
    errors::error::VaultError,
    make_secret_id,
    types::{
        algorithm::Algorithm as SecretAlgorithm, metadata::Metadata as SecretMetadata,
        secret::Secret, secret_id::SecretId, store_mode::StoreMode as SecretStoreMode,
    },
    vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    fmt::{Display, Formatter},
    fs::File,
    io::BufReader,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{self, AtomicI32},
        Arc,
    },
    time::Duration,
};
use storage::shardstate_db_async::CellsDbConfig;
use ton_api::{
    ton::{
        self,
        adnl::{address::address::Udp, addresslist::AddressList as AdnlAddressList},
        dht::node::Node as DhtNodeConfig,
        engine::validator::{
            customoverlay::CustomOverlay, customoverlaynode::CustomOverlayNode,
            customoverlaysconfig::CustomOverlaysConfig,
            CustomOverlaysConfig as CustomOverlaysConfigBoxed,
        },
        pk::privatekey::Ed25519 as Ed25519Private,
        pub_::publickey::Ed25519,
        PrivateKey,
    },
    IntoBoxed,
};
use ton_block::{
    base64_decode, base64_encode, ed25519_create_private_key, error, fail, BlockIdExt,
    Ed25519KeyOption, KeyId, KeyOption, KeyOptionJson, MsgAddressInt, Result, ShardIdent, UInt256,
};
use ton_block_json::PathMap;

#[macro_export]
macro_rules! key_option_public_key {
    ($key: expr) => {
        format!(
            "{{
               \"type_id\": 1209251014,
               \"pub_key\": \"{}\"
            }}",
            $key
        )
        .as_str()
    };
}

#[async_trait::async_trait]
pub trait KeyRing: Sync + Send {
    async fn generate(&self, key_type: i32) -> Result<[u8; 32]>;
    async fn import_private_key(&self, key: PrivateKey) -> Result<[u8; 32]>;
    // find private key in KeyRing by public key hash
    fn find(&self, key_hash: &[u8; 32]) -> Result<Arc<dyn KeyOption>>;
    fn sign_data(&self, key_hash: &[u8; 32], data: &[u8]) -> Result<Vec<u8>>;
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct CellsGcConfig {
    pub gc_interval_sec: u32,
    pub cells_lifetime_sec: u64,
}

impl Default for CellsGcConfig {
    fn default() -> Self {
        CellsGcConfig { gc_interval_sec: 900, cells_lifetime_sec: 1800 }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct CollatorConfig {
    pub cutoff_timeout_ms: u32,
    pub stop_timeout_ms: u32,
    pub clean_timeout_percentage_points: u32,
    pub optimistic_clean_percentage_points: u32,
    pub max_secondary_clean_timeout_percentage_points: u32,
    pub max_collate_threads: u32,
    pub retry_if_empty: bool,
    pub finalize_empty_after_ms: u32,
    pub empty_collation_sleep_ms: u32,
    pub external_messages_timeout_percentage_points: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_messages_maximum_queue_length: Option<u32>, // None - unlimited
    pub dispatch_phase_2_max_total: usize,
    pub dispatch_phase_3_max_total: usize,
    pub dispatch_phase_2_max_per_initiator: usize,
    pub dispatch_phase_3_max_per_initiator: usize,
    pub defer_messages_after: usize,
    pub defer_out_queue_size_limit: usize,
    #[serde(deserialize_with = "from_str_vec", serialize_with = "to_str_vec")]
    pub priority_list: Vec<MsgAddressInt>,
    #[serde(deserialize_with = "from_str_vec", serialize_with = "to_str_vec")]
    pub whitelist: Vec<MsgAddressInt>,
}

fn from_str_vec<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    let s: Vec<String> = serde::Deserialize::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(s.len());
    for item in s {
        result.push(item.parse().map_err(serde::de::Error::custom)?);
    }
    Ok(result)
}

fn to_str_vec<T, S>(items: &Vec<T>, serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: ToString,
{
    let s: Vec<String> = items.iter().map(|item| item.to_string()).collect();
    serde::Serialize::serialize(&s, serializer)
}

impl Default for CollatorConfig {
    fn default() -> Self {
        Self {
            cutoff_timeout_ms: 1000,
            stop_timeout_ms: 1500,
            clean_timeout_percentage_points: 150, // 0.150 = 15% = 150ms
            optimistic_clean_percentage_points: 1000, // 1.000 = 100% = 150ms
            max_secondary_clean_timeout_percentage_points: 350, // 0.350 = 35% = 350ms
            max_collate_threads: 10,
            retry_if_empty: false,
            finalize_empty_after_ms: 800,
            empty_collation_sleep_ms: 100,
            external_messages_timeout_percentage_points: 100, // 0.1 = 10% = 100ms
            external_messages_maximum_queue_length: Some(25600),
            dispatch_phase_2_max_total: 150,
            dispatch_phase_3_max_total: 150,
            dispatch_phase_2_max_per_initiator: 20,
            dispatch_phase_3_max_per_initiator: 0,
            defer_messages_after: 10,
            defer_out_queue_size_limit: 2048,
            priority_list: Vec::new(),
            whitelist: Vec::new(),
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Copy, Default)]
pub enum ShardStatesCacheMode {
    Off, // States saved sinchronously and not cached.
    #[default]
    Moderate, // States saved asiynchronously.
}
impl ShardStatesCacheMode {
    pub fn _is_enabled(&self) -> bool {
        matches!(self, ShardStatesCacheMode::Moderate)
    }
    pub fn is_disabled(&self) -> bool {
        matches!(self, ShardStatesCacheMode::Off)
    }
}

pub struct SecretsVaultConfig {}

impl SecretsVaultConfig {
    const KEY_ADNL_KEY_ID: &str = "adnl_key_id";
    const KEY_ELECTION_ID: &str = "election_id";
    const KEY_EXPIRE_AT: &str = "expire_at";
    const KEY_TYPE: &str = "type";
    const KEY_VALIDATOR_KEY_ID: &str = "validator_key_id";
    const SID_PRIVATE_KEYS: &str = "private_keys";
    const SID_VALIDATOR_KEYS: &str = "validator_keys";
    const TYPE_PRIVATE_KEY: &str = "private_key";
    const TYPE_VALIDATOR_KEY: &str = "validator_key";

    async fn open_vault() -> Result<Option<Arc<SecretVault>>> {
        let vault = SecretVaultBuilder::from_url_or_env(None).await?;
        Ok(vault)
    }

    async fn on_drop(key_id: &str) -> Result<()> {
        async fn delete(vault: &SecretVault, sid: &str, key_id: &str) -> Option<anyhow::Error> {
            let secret_id = make_secret_id!(sid, key_id);
            vault.delete(&secret_id).await.err().filter(|e| {
                if let Some(e) = e.downcast_ref::<VaultError>() {
                    e.code() != VaultError::NOT_FOUND
                } else {
                    true
                }
            })
        }

        let Some(vault) = Self::open_vault().await? else {
            return Ok(());
        };
        log::info!("Drop key from the vault");
        let err1 = delete(&vault, Self::SID_PRIVATE_KEYS, key_id).await;
        let err2 = delete(&vault, Self::SID_VALIDATOR_KEYS, key_id).await;
        match (err1, err2) {
            (Some(err1), Some(err2)) => fail!("{err1}, {err2}"),
            (Some(err), None) | (None, Some(err)) => fail!("{err}"),
            (None, None) => Ok(()),
        }
    }

    pub async fn on_load(config: &mut TonNodeConfig) -> Result<()> {
        let Some(vault) = Self::open_vault().await? else {
            return Ok(());
        };

        // Create/Load secrets vault
        log::info!("Load keys from the vault");
        let metadata_list = vault.list_metadata().await?;

        // Read private keys
        for metadata in &metadata_list {
            let secret_id = metadata.secret_id.as_ref().ok_or_else(|| error!("Empty secret id"))?;
            let secret: Secret = vault.get(secret_id).await?;
            let Secret::Blob { blob } = secret else {
                continue;
            };
            if metadata.get_tag_str(Self::KEY_TYPE)? != Self::TYPE_PRIVATE_KEY {
                continue;
            }
            let private_key = blob.data().await?;
            let private_key_lock = private_key.lock().await?;
            let private_key_data: &[u8] = &private_key_lock.get(..32).unwrap();
            let key = UInt256::with_array(private_key_data.try_into()?);
            let private_key_in = PrivateKey::Pk_Ed25519(Ed25519Private { key });
            config.import_private_key(private_key_in)?;
            log::info!("Read key {} from the vault", secret_id);
        }

        // Read elections metadata
        for metadata in &metadata_list {
            let secret_id = metadata.secret_id.as_ref().ok_or_else(|| error!("Empty secret id"))?;
            let secret: Secret = vault.get(secret_id).await?;
            let Secret::Blob { blob: _ } = secret else {
                continue;
            };
            if metadata.get_tag_str(Self::KEY_TYPE)? != Self::TYPE_VALIDATOR_KEY {
                continue;
            }

            let validator_key_id = metadata.get_tag_blob_b64(Self::KEY_VALIDATOR_KEY_ID)?;
            let validator_key_id = validator_key_id.as_slice();
            let election_id = metadata.get_tag_i32(Self::KEY_ELECTION_ID)?;
            let expire_at = metadata.get_tag_i32(Self::KEY_EXPIRE_AT)?;
            config.add_validator_key(validator_key_id.try_into()?, election_id, expire_at)?;
            log::info!("Read metadata for key {} from the vault", base64_encode(validator_key_id));

            if !metadata.tags.contains_key(Self::KEY_ADNL_KEY_ID) {
                continue;
            }
            let adnl_key_id = metadata.get_tag_blob_b64(Self::KEY_ADNL_KEY_ID)?;
            let adnl_key_id = adnl_key_id.as_slice();
            config.add_validator_adnl_key(validator_key_id.try_into()?, adnl_key_id.try_into()?)?;
            log::info!("Read metadata for ADNL key {} from the vault", base64_encode(adnl_key_id));
        }

        Ok(())
    }

    async fn on_save(config: &TonNodeConfig) -> Result<()> {
        let Some(vault) = Self::open_vault().await? else {
            return Ok(());
        };

        // Create/Save secrets vault
        log::info!("Save keys to the vault");

        // Write private keys
        let mut secrets = Vec::new();
        if let Some(key_ring) = config.validator_key_ring.as_ref() {
            let crypto_factory = AutoCryptoFactory {};
            for (key_id, key_opt_json) in key_ring {
                log::info!("Write key {key_id} to the vault");
                let key_pvt = key_opt_json.get_pvt_key()?;
                let secret_id = make_secret_id!(Self::SID_PRIVATE_KEYS, key_id);
                let metadata = SecretMetadata::new(Some(&secret_id), SecretAlgorithm::None, true)
                    .with_tag(Self::KEY_TYPE, Self::TYPE_PRIVATE_KEY);
                let secret =
                    Secret::from_raw_data(&key_pvt, metadata, crypto_factory.new_crypto()?).await?;
                secrets.push((secret, SecretStoreMode::CreateOrReplace));
            }
        }

        // Write elections metadata
        if let Some(validator_keys) = config.validator_keys.as_ref() {
            let crypto_factory = AutoCryptoFactory {};
            for validator_key_json in validator_keys {
                log::info!(
                    "Write metadata for key {} to the vault",
                    &validator_key_json.validator_key_id
                );

                let secret_id: SecretId =
                    make_secret_id!(Self::SID_VALIDATOR_KEYS, &validator_key_json.validator_key_id);
                let mut metadata =
                    SecretMetadata::new(Some(&secret_id), SecretAlgorithm::None, true)
                        .with_tag(Self::KEY_TYPE, Self::TYPE_VALIDATOR_KEY)
                        .with_tag(Self::KEY_ELECTION_ID, validator_key_json.election_id.to_string())
                        .with_tag(Self::KEY_EXPIRE_AT, validator_key_json.expire_at.to_string())
                        .with_tag(Self::KEY_VALIDATOR_KEY_ID, &validator_key_json.validator_key_id);
                if let Some(adnl_key_id) = &validator_key_json.validator_adnl_key_id {
                    log::info!("Write metadata for ADNL key {} to the vault", &adnl_key_id);
                    metadata = metadata.with_tag(Self::KEY_ADNL_KEY_ID, adnl_key_id);
                }

                let secret =
                    Secret::from_raw_data(b"".as_slice(), metadata, crypto_factory.new_crypto()?)
                        .await?;
                secrets.push((secret, SecretStoreMode::CreateOrReplace));
            }
        }

        if !secrets.is_empty() {
            vault.put_vec(secrets).await?;
        }

        Ok(())
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct TonNodeConfig {
    log_config_name: Option<String>,
    ton_global_config_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    boot_from_zerostate: Option<bool>,
    internal_db_path: Option<String>,
    validation_countdown_mode: Option<String>,
    unsafe_catchain_patches_path: Option<String>,
    #[serde(skip_serializing)]
    ip_address: Option<String>,
    adnl_node: Option<AdnlNodeConfigJson>,
    json_rpc_server: Option<JsonRpcServerConfigJson>,
    metrics: Option<MetricsConfigJson>,
    #[serde(skip_serializing_if = "NodeExtensions::is_default")]
    #[serde(default)]
    extensions: NodeExtensions,
    validator_keys: Option<Vec<ValidatorKeysJson>>,
    #[serde(skip_serializing)]
    control_server_port: Option<u16>,
    control_server: Option<AdnlServerConfigJson>,
    #[serde(skip_serializing)]
    lite_server_port: Option<u16>,
    lite_server: Option<LiteServerConfigJson>,
    default_rldp_roundtrip_ms: Option<u32>,
    #[serde(default)]
    test_bundles_config: CollatorTestBundlesGeneralConfig,
    gc: Option<GC>,
    validator_key_ring: Option<HashMap<String, KeyOptionJson>>,
    #[serde(skip)]
    configs_dir: String,
    #[serde(skip)]
    port: Option<u16>,
    #[serde(skip)]
    file_name: String,
    #[serde(skip)]
    custom_overlays_file_name: String,
    #[serde(default)]
    restore_db: bool,
    #[serde(default)]
    cells_db_config: CellsDbConfig,
    #[serde(default)]
    collator_config: CollatorConfig,
    #[serde(default)]
    collator_config_mc: Option<CollatorConfig>,
    #[serde(default)]
    skip_saving_persistent_states: bool,
    #[serde(default)]
    states_cache_mode: ShardStatesCacheMode,
    #[serde(default)]
    sync_by_archives: bool,
    #[serde(default)]
    accelerated_consensus_disabled: bool,
    #[serde(skip)]
    custom_overlays: CustomOverlaysConfigBoxed,
    #[serde(default)]
    pss_downloading_threads: usize,
}

pub struct TonNodeGlobalConfig(TonNodeGlobalConfigJson);

#[derive(Default, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct NodeExtensions {
    pub disable_broadcast_retransmit: bool,
    pub adnl_compression: bool,
    pub broadcast_hops: Option<u8>,
}

impl NodeExtensions {
    fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct ValidatorKeysJson {
    #[serde(default)]
    pub expire_at: i32,
    pub election_id: i32,
    pub validator_key_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validator_adnl_key_id: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Default, Debug, Clone)]
pub struct GC {
    enable_for_archives: bool,
    archives_life_time_hours: Option<u32>, // Hours
    enable_for_shard_state_persistent: bool,
    #[serde(default)]
    cells_gc_config: CellsGcConfig,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize, Clone)]
#[serde(default)]
pub struct CollatorTestBundlesConfig {
    build_for_unknown_errors: bool,
    known_errors: Vec<String>,
    build_for_errors: bool,
    errors: Vec<String>,
    path: String,
}

impl CollatorTestBundlesConfig {
    pub fn is_enable(&self) -> bool {
        self.build_for_unknown_errors || (self.build_for_errors && !self.errors.is_empty())
    }

    pub fn need_to_build_for(&self, error: &str) -> bool {
        self.build_for_unknown_errors && self.known_errors.iter().all(|e| !error.contains(e))
            || self.build_for_errors && self.errors.iter().any(|e| error.contains(e))
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize, Clone)]
#[serde(default)]
pub struct CollatorTestBundlesGeneralConfig {
    pub collator: CollatorTestBundlesConfig,
    pub validator: CollatorTestBundlesConfig,
}

const LOCAL_HOST: &str = "127.0.0.1";

impl TonNodeConfig {
    pub const DEFAULT_DB_ROOT: &'static str = "node_db";
    pub const CUSTOM_OVERLAYS_CONFIG_NAME: &'static str = "custom-overlays.json";

    pub fn boot_from_zerostate(&self) -> bool {
        self.boot_from_zerostate.unwrap_or(false)
    }

    pub fn from_file(
        configs_dir: &str,
        json_file_name: &str,
        adnl_config: Option<AdnlNodeConfigJson>,
        default_config_name: &str,
        client_console_key: Option<String>,
    ) -> Result<Self> {
        let config_file_path = TonNodeConfig::build_path(configs_dir, json_file_name);
        let config_file = File::open(config_file_path.clone());

        let mut config_json = match config_file {
            Ok(file) => {
                let reader = BufReader::new(file);
                let config: TonNodeConfig = serde_json::from_reader(reader)?;
                if client_console_key.is_some() {
                    println!("Can't generate console_config.json: delete config.json before");
                }
                config
            }
            Err(_) => {
                // generate new config from default_config
                let path = TonNodeConfig::build_path(configs_dir, default_config_name);
                let default_config_file =
                    File::open(&path).map_err(|err| error!("Can't open {:?}: {}", path, err))?;

                let reader = BufReader::new(default_config_file);
                let mut config: TonNodeConfig = serde_json::from_reader(reader)?;
                // Set ADNL config
                config.adnl_node = if let Some(adnl_config) = adnl_config {
                    Some(adnl_config)
                } else {
                    let ip_address = if let Some(ip_address) = &config.ip_address {
                        ip_address
                    } else {
                        fail!("IP address is not set in default config")
                    };
                    let (adnl_config, _) = AdnlNodeConfig::with_ip_address_and_private_key_tags(
                        ip_address,
                        vec![NodeNetwork::TAG_DHT_KEY, NodeNetwork::TAG_OVERLAY_KEY],
                    )?;
                    Some(adnl_config)
                };
                config.create_and_save_console_configs(configs_dir, client_console_key)?;
                config.create_and_save_lite_configs(configs_dir)?;
                config.ip_address = None;
                std::fs::write(config_file_path, serde_json::to_string_pretty(&config)?)?;
                config
            }
        };

        let custom_overlays_config_path =
            TonNodeConfig::build_path(configs_dir, Self::CUSTOM_OVERLAYS_CONFIG_NAME);
        config_json.custom_overlays_file_name =
            custom_overlays_config_path.to_string_lossy().to_string();
        if Path::exists(&custom_overlays_config_path) {
            config_json.custom_overlays =
                Self::load_custom_overlays_json(&config_json.custom_overlays_file_name)?;
        } else {
            config_json.custom_overlays = CustomOverlaysConfigBoxed::default();
        }

        config_json.configs_dir = configs_dir.to_string();
        config_json.file_name = json_file_name.to_string();
        Ok(config_json)
    }

    pub fn adnl_node(&self) -> Result<AdnlNodeConfig> {
        let adnl_node =
            self.adnl_node.as_ref().ok_or_else(|| error!("ADNL node is not configured!"))?;

        let mut ret = AdnlNodeConfig::from_json_config(adnl_node)?;
        if let Some(port) = self.port {
            ret.set_port(port)
        }
        Ok(ret)
    }

    pub fn json_rpc_server(&self) -> Result<Option<JsonRpcServerConfig>> {
        match &self.json_rpc_server {
            Some(r) => Ok(Some(JsonRpcServerConfig::from_json_config(r)?)),
            None => Ok(None),
        }
    }

    pub fn metrics(&self) -> Result<Option<MetricsConfig>> {
        match &self.metrics {
            Some(m) => Ok(Some(MetricsConfig::from_json_config(m)?)),
            None => Ok(None),
        }
    }

    pub fn control_server(&self) -> Result<Option<AdnlServerConfig>> {
        self.control_server.as_ref().map(AdnlServerConfig::from_json_config).transpose()
    }

    pub fn lite_server(&self) -> Result<Option<LiteServerConfig>> {
        self.lite_server.as_ref().map(LiteServerConfig::from_json_config).transpose()
    }

    pub fn log_config_path(&self) -> Option<PathBuf> {
        if let Some(log_config_name) = &self.log_config_name {
            return Some(self.build_config_path(log_config_name));
        }
        None
    }

    pub fn unsafe_catchain_patches_files(&self) -> Vec<String> {
        let mut result = Vec::new();
        if let Some(catchain_patches) = &self.unsafe_catchain_patches_path {
            let log_path = self.build_config_path(catchain_patches);
            if let Ok(dir) = std::fs::read_dir(log_path) {
                for filename in dir.into_iter().flatten() {
                    if let Some(path_str) = filename.path().to_str() {
                        if path_str.ends_with(".json") {
                            result.push(path_str.to_string());
                        }
                    }
                }
            }
        }
        result
    }

    pub fn validation_countdown_mode(&self) -> Option<String> {
        self.validation_countdown_mode.clone()
    }

    pub fn gc_archives_life_time_hours(&self) -> Option<u32> {
        if let Some(gc) = &self.gc {
            if gc.enable_for_archives {
                return gc.archives_life_time_hours.or(Some(0));
            }
        }
        None
    }

    pub fn internal_db_path(&self) -> &str {
        self.internal_db_path.as_deref().unwrap_or(Self::DEFAULT_DB_ROOT)
    }

    pub fn cells_gc_config(&self) -> CellsGcConfig {
        match &self.gc {
            Some(conf) => conf.cells_gc_config.clone(),
            None => CellsGcConfig::default(),
        }
    }

    pub fn enable_shard_state_persistent_gc(&self) -> bool {
        self.gc.as_ref().map(|c| c.enable_for_shard_state_persistent).unwrap_or(false)
    }

    #[cfg(test)]
    pub fn set_internal_db_path(&mut self, path: String) {
        self.internal_db_path.replace(path);
    }

    pub fn default_rldp_roundtrip(&self) -> Option<u32> {
        self.default_rldp_roundtrip_ms
    }

    pub fn test_bundles_config(&self) -> &CollatorTestBundlesGeneralConfig {
        &self.test_bundles_config
    }
    pub fn extensions(&self) -> &NodeExtensions {
        &self.extensions
    }
    pub fn restore_db(&self) -> bool {
        self.restore_db
    }
    pub fn skip_saving_persistent_states(&self) -> bool {
        self.skip_saving_persistent_states
    }
    pub fn states_cache_mode(&self) -> ShardStatesCacheMode {
        self.states_cache_mode
    }
    pub fn sync_by_archives(&self) -> bool {
        self.sync_by_archives
    }
    pub fn cells_db_config(&self) -> &CellsDbConfig {
        &self.cells_db_config
    }

    pub fn is_accelerated_consensus_disabled(&self) -> bool {
        self.accelerated_consensus_disabled
    }

    #[cfg(test)]
    pub fn set_port(&mut self, port: u16) {
        self.port.replace(port);
    }

    pub fn collator_config(&self) -> &CollatorConfig {
        &self.collator_config
    }

    pub fn collator_config_mc(&self) -> &CollatorConfig {
        &self.collator_config_mc.as_ref().unwrap_or(&self.collator_config)
    }

    pub fn custom_overlays_config(&self) -> &[CustomOverlay] {
        self.custom_overlays.overlays()
    }

    pub fn pss_downloading_threads(&self) -> usize {
        self.pss_downloading_threads
    }

    pub fn load_global_config(&self) -> Result<TonNodeGlobalConfig> {
        let name = self
            .ton_global_config_name
            .as_ref()
            .ok_or_else(|| error!("global config information not found in config.json!"))?;
        let global_config_path = self.build_config_path(name);
        /*
                let data = std::fs::read_to_string(global_config_path)
                    .map_err(|err| error!("Global config file is not found! : {}", err))?;
        */
        TonNodeGlobalConfig::from_json_file(global_config_path)
    }

    // Unused
    //    pub fn remove_all_validator_keys(&mut self) {
    //        self.validator_keys = None;
    //    }

    fn create_and_save_configs(
        &mut self,
        configs_dir: &str,
        port: Option<u16>,
        client_config_name: &str,
        client_pub_keys: Option<Vec<String>>,
    ) -> Result<Option<AdnlServerConfigJson>> {
        let server_address = if let Some(port) = port {
            format!("{}:{}", LOCAL_HOST, port)
        } else {
            println!(
                "Can`t generate {client_config_name}: \
                default config doesn`t contain server port"
            );
            return Ok(None);
        };
        let (server_private_key, server_key) = Ed25519KeyOption::generate_with_json()?;

        // generate and save client console template
        let client_config_file_path = TonNodeConfig::build_path(configs_dir, client_config_name);
        let client_config = AdnlClientConfigJson::with_params(
            &server_address,
            serde_json::from_str(key_option_public_key!(base64_encode(server_key.pub_key()?)))?,
            None,
            None,
        );
        std::fs::write(client_config_file_path, serde_json::to_string_pretty(&client_config)?)
            .map_err(|err| error!("Can`t create {client_config_name}: {}", err))?;

        // generate and save server config
        let client_keys = if let Some(client_pub_keys) = client_pub_keys {
            let mut keys = Vec::new();
            for client_pub_key in client_pub_keys {
                keys.push(serde_json::from_str(client_pub_key.as_str())?);
            }
            Some(keys)
        } else {
            None
        };

        let server_config = AdnlServerConfigJson::with_params(
            server_address,
            server_private_key,
            client_keys,
            None,
        );
        Ok(Some(server_config))
    }

    fn create_and_save_console_configs(
        &mut self,
        configs_dir: &str,
        client_pub_key: Option<String>,
    ) -> Result<()> {
        self.control_server = self.create_and_save_configs(
            configs_dir,
            self.control_server_port.clone(),
            "console_config.json",
            Some(client_pub_key.map(|key| vec![key]).unwrap_or(vec![])),
        )?;
        self.control_server_port = None;
        Ok(())
    }

    fn create_and_save_lite_configs(&mut self, configs_dir: &str) -> Result<()> {
        self.lite_server = self
            .create_and_save_configs(
                configs_dir,
                self.lite_server_port.clone(),
                "lite_client_config.json",
                None,
            )?
            .map(|adnl| LiteServerConfigJson::from_server_config(adnl));
        self.lite_server_port = None;
        Ok(())
    }

    fn get_validator_key_info(&self, validator_key_id: &str) -> Result<Option<ValidatorKeysJson>> {
        if let Some(validator_keys) = &self.validator_keys {
            for key_json in validator_keys {
                if key_json.validator_key_id == validator_key_id {
                    return Ok(Some(key_json.clone()));
                }
            }
        }
        Ok(None)
    }

    fn get_validator_key_info_by_election_id(
        &self,
        election_id: &i32,
    ) -> Result<Option<ValidatorKeysJson>> {
        if let Some(validator_keys) = &self.validator_keys {
            for key_json in validator_keys {
                if key_json.election_id == *election_id {
                    return Ok(Some(key_json.clone()));
                }
            }
        }
        Ok(None)
    }

    fn update_validator_key_info(
        &mut self,
        updated_info: ValidatorKeysJson,
    ) -> Result<ValidatorKeysJson> {
        if let Some(validator_keys) = &mut self.validator_keys {
            for keys_info in validator_keys.iter_mut() {
                if keys_info.election_id == updated_info.election_id {
                    keys_info.expire_at = updated_info.expire_at;
                    keys_info.validator_key_id = updated_info.validator_key_id;
                    keys_info.validator_adnl_key_id = updated_info.validator_adnl_key_id;
                    return Ok(keys_info.clone());
                }
            }
        }
        fail!("Validator keys information was not found!");
    }

    pub fn build_config_path(&self, file_name: &str) -> PathBuf {
        Self::build_path(&self.configs_dir, file_name)
    }

    fn build_path(directory_name: &str, file_name: &str) -> PathBuf {
        let path = Path::new(directory_name);
        path.join(file_name)
    }

    async fn save_to_file(&self, file_name: &str) -> Result<()> {
        let config_file_path = self.build_config_path(file_name);
        std::fs::write(config_file_path, serde_json::to_string_pretty(&self)?)?;

        // Temporary workaround: save secrets from config into the vault
        SecretsVaultConfig::on_save(&self).await?;

        Ok(())
    }

    async fn save_custom_overlays_json(conf: &CustomOverlaysConfigBoxed, path: &str) -> Result<()> {
        let mut list = Vec::new();
        for overlay in conf.overlays() {
            let mut nodes = Vec::new();
            for node in &overlay.nodes {
                let mut map = serde_json::Map::new();
                map.insert(
                    "adnl_id".to_string(),
                    serde_json::Value::String(base64_encode(&node.adnl_id.as_slice())),
                );
                map.insert(
                    "msg_sender".to_string(),
                    serde_json::Value::Bool((&node.msg_sender).into()),
                );
                map.insert(
                    "msg_sender_priority".to_string(),
                    serde_json::Value::Number(node.msg_sender_priority.into()),
                );
                map.insert(
                    "block_sender".to_string(),
                    serde_json::Value::Bool((&node.block_sender).into()),
                );
                nodes.push(serde_json::Value::Object(map));
            }
            let mut shards = Vec::new();
            for shard in &overlay.sender_shards {
                let mut map = serde_json::Map::new();
                map.insert(
                    "workchain".to_string(),
                    serde_json::Value::Number(shard.workchain.into()),
                );
                map.insert("shard".to_string(), serde_json::Value::Number(shard.shard.into()));
                shards.push(serde_json::Value::Object(map));
            }
            let mut map = serde_json::Map::new();
            map.insert("name".to_string(), serde_json::Value::String(overlay.name.clone()));
            map.insert("nodes".to_string(), serde_json::Value::Array(nodes));
            map.insert("sender_shards".to_string(), serde_json::Value::Array(shards));
            map.insert(
                "skip_public_msg_send".to_string(),
                serde_json::Value::Bool((&overlay.skip_public_msg_send).into()),
            );
            list.push(serde_json::Value::Object(map));
        }
        let mut map = serde_json::Map::new();
        map.insert("overlays".to_string(), serde_json::Value::Array(list));
        std::fs::write(path, serde_json::to_string_pretty(&map)?)?;
        Ok(())
    }

    fn load_custom_overlays_json(path: &str) -> Result<CustomOverlaysConfigBoxed> {
        let file = File::open(path).map_err(|err| error!("Can't open {:?}: {}", path, err))?;
        let reader = BufReader::new(file);
        let json_value: serde_json::Value = serde_json::from_reader(reader)?;
        Self::parse_custom_overlays_json(json_value)
    }

    pub fn parse_custom_overlay_json(json_value: &serde_json::Value) -> Result<CustomOverlay> {
        let mut nodes = Vec::new();
        let overlay_json = PathMap::new(
            json_value.as_object().ok_or_else(|| error!("Invalid custom overlay format"))?,
        );
        let nodes_json = overlay_json.get_obj_vec("nodes")?;
        for node_json in nodes_json {
            let adnl_id_b64 = node_json.get_str("adnl_id")?;
            let adnl_id = base64_decode(&adnl_id_b64)?;
            if adnl_id.len() != 32 {
                fail!(
                    "Invalid adnl_id length {} (expected 32) in custom overlay node",
                    adnl_id.len()
                );
            }
            let msg_sender = node_json.get_bool("msg_sender")?;
            let msg_sender_priority = node_json.get_num32("msg_sender_priority")?;
            let block_sender = node_json.get_bool("block_sender")?;
            nodes.push(CustomOverlayNode {
                adnl_id: UInt256::from_raw(adnl_id, 256),
                msg_sender: msg_sender.into(),
                msg_sender_priority: msg_sender_priority as i32,
                block_sender: block_sender.into(),
            });
        }
        let mut sender_shards = Vec::new();
        let sender_shards_json = overlay_json.get_obj_vec("sender_shards")?;
        for shard_json in sender_shards_json {
            let workchain = shard_json.get_num32("workchain")? as i32;
            let shard = shard_json.get_num64("shard")? as i64;
            sender_shards.push(ton::ton_node::shardid::ShardId { workchain, shard });
        }
        let name = overlay_json.get_str("name")?.to_string();
        let skip_public_msg_send = overlay_json.get_bool("skip_public_msg_send")?;
        let overlay = CustomOverlay {
            name,
            nodes,
            sender_shards,
            skip_public_msg_send: skip_public_msg_send.into(),
        };
        Ok(overlay)
    }

    fn parse_custom_overlays_json(
        json_value: serde_json::Value,
    ) -> Result<CustomOverlaysConfigBoxed> {
        let map = json_value
            .as_object()
            .ok_or_else(|| error!("Invalid custom overlays config format"))?;
        let map = PathMap::new(map);
        let overlays_json = map.get_vec("overlays")?;
        let mut overlays = Vec::new();
        for overlay_json in overlays_json {
            overlays.push(Self::parse_custom_overlay_json(overlay_json)?);
        }
        Ok(CustomOverlaysConfig { overlays }.into_boxed())
    }

    fn generate_and_save_keys(&mut self, _key_type: i32) -> Result<([u8; 32], Arc<dyn KeyOption>)> {
        let (private, public) = Ed25519KeyOption::generate_with_json()?;
        let key_id = public.id().data();
        log::info!("generate_and_save_keys: generate new key (id: {})", base64_encode(key_id),);
        let key_ring = self.validator_key_ring.get_or_insert_default();
        key_ring.insert(base64_encode(key_id), private);
        Ok((*key_id, public))
    }

    fn import_private_key(
        &mut self,
        pvt_key: PrivateKey,
    ) -> Result<([u8; 32], Arc<dyn KeyOption>)> {
        match pvt_key {
            PrivateKey::Pk_Ed25519(pvt_key) => {
                let (private, public) = Ed25519KeyOption::create_from_private_key_with_json(
                    ed25519_create_private_key(pvt_key.key.as_array())?,
                )?;
                let key_id = public.id().data();
                log::info!(
                    "import_private_key: import private key key (id: {})",
                    base64_encode(key_id)
                );
                let key_ring = self.validator_key_ring.get_or_insert_with(HashMap::new);
                key_ring.insert(base64_encode(key_id), private);
                Ok((*key_id, public))
            }
            _ => fail!("Unsupported key type"),
        }
    }

    fn is_correct_election_id(&self, _election_id: i32) -> bool {
        // Temporary change
        // When importing keys from a C++ node, it is acceptable to import them in any chronological order
        /*
        if let Some(validator_keys) = &self.validator_keys {
            for key_json in validator_keys {
                if key_json.election_id > election_id {
                    return false;
                }
            }
        }
        */
        true
    }

    fn add_validator_key(
        &mut self,
        key_id: &[u8; 32],
        election_id: i32,
        expire_at: i32,
    ) -> Result<ValidatorKeysJson> {
        let key_info = ValidatorKeysJson {
            expire_at,
            election_id,
            validator_key_id: base64_encode(key_id),
            validator_adnl_key_id: None,
        };

        if !self.is_correct_election_id(election_id) {
            fail!("Invalid arg: bad election_id!");
        }
        let added_key_info = self.get_validator_key_info_by_election_id(&election_id)?;
        match &mut self.validator_keys {
            Some(validator_keys) => match added_key_info {
                Some(_) => {
                    self.update_validator_key_info(key_info.clone())?;
                }
                None => {
                    validator_keys.push(key_info.clone());
                }
            },
            None => {
                let keys = vec![key_info.clone()];
                self.validator_keys = Some(keys);
            }
        }
        Ok(key_info)
    }

    fn add_validator_adnl_key(
        &mut self,
        validator_key_id: &[u8; 32],
        adnl_key_id: &[u8; 32],
    ) -> Result<ValidatorKeysJson> {
        if let Some(mut key_info) = self.get_validator_key_info(&base64_encode(validator_key_id))? {
            key_info.validator_adnl_key_id = Some(base64_encode(adnl_key_id));
            self.update_validator_key_info(key_info)
        } else {
            fail!("Validator key have not been added!")
        }
    }

    async fn remove_validator_key(
        &mut self,
        validator_key_id: String,
        election_id: i32,
    ) -> Result<bool> {
        if let Some(validator_keys) = self.validator_keys.as_mut() {
            let pos = validator_keys.iter().position(|item| {
                (item.validator_key_id == validator_key_id) && (item.election_id == election_id)
            });
            if let Some(pos) = pos {
                validator_keys.swap_remove(pos);
                SecretsVaultConfig::on_drop(validator_key_id.as_str()).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn remove_key_from_key_ring(&mut self, validator_key_id: &str) -> Result<()> {
        if let Some(key_ring) = self.validator_key_ring.as_mut() {
            key_ring.remove(validator_key_id);
            SecretsVaultConfig::on_drop(validator_key_id).await?;
        }
        Ok(())
    }
}

pub enum ConfigEvent {
    AddValidatorAdnlKey(Arc<KeyId>, i32),
    //RemoveValidatorAdnlKey(Arc<KeyId>, i32)
}

#[async_trait::async_trait]
pub trait NodeConfigSubscriber: Send + Sync {
    async fn event(&self, sender: ConfigEvent) -> Result<bool>;
}

#[derive(Debug)]
enum Task {
    Generate(i32),
    ImportPrivateKey(PrivateKey),
    AddValidatorKey([u8; 32], i32, i32),
    AddValidatorAdnlKey([u8; 32], [u8; 32]),
    GetKey([u8; 32]),
    ExportAdnlKeys,
    AddCustomOverlay(CustomOverlay),
    DelCustomOverlay(String),
    GetCustomOverlays,
}

#[derive(Debug)]
enum Answer {
    Generate(Result<[u8; 32]>),
    ImportPrivateKey(Result<[u8; 32]>),
    GetKey(Option<Arc<dyn KeyOption>>),
    Result(Result<()>),
    AdnlKeys(Vec<(String, usize)>),
    CustomOverlays(Result<CustomOverlaysConfigBoxed>),
}

pub struct NodeConfigHandlerContext {
    reader: tokio::sync::mpsc::UnboundedReceiver<(Arc<Wait<Answer>>, Task)>,
    config: TonNodeConfig,
}

pub struct NodeConfigHandler {
    runtime_handle: tokio::runtime::Handle,
    sender: tokio::sync::mpsc::UnboundedSender<(Arc<Wait<Answer>>, Task)>,
    key_ring: Arc<lockfree::map::Map<String, Arc<dyn KeyOption>>>,
    validator_keys: Arc<ValidatorKeys>,
}

impl NodeConfigHandler {
    pub fn create(
        config: TonNodeConfig,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<(Arc<Self>, NodeConfigHandlerContext)> {
        let (sender, reader) = tokio::sync::mpsc::unbounded_channel();
        let config_handler = Arc::new(NodeConfigHandler {
            runtime_handle,
            sender,
            key_ring: Arc::new(lockfree::map::Map::new()),
            validator_keys: Arc::new(ValidatorKeys::new()),
        });

        Ok((config_handler, NodeConfigHandlerContext { reader, config }))
    }

    pub fn get_validator_status(&self) -> bool {
        self.validator_keys.is_empty()
    }

    pub async fn add_validator_key(
        &self,
        key_hash: &[u8; 32],
        election_date: ton::int,
        expire_at: ton::int,
    ) -> Result<()> {
        let (wait, mut queue_reader) = Wait::new();
        let task = Task::AddValidatorKey(*key_hash, election_date, expire_at);
        let pushed_task = (wait.clone(), task);
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error add_validator_key: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::Result(result))) => result,
            Some(Some(_)) => fail!("Bad answer (AddValidatorKey)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    pub async fn add_validator_adnl_key(
        &self,
        validator_key_hash: &[u8; 32],
        validator_adnl_key_hash: &[u8; 32],
    ) -> Result<()> {
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (
            wait.clone(),
            Task::AddValidatorAdnlKey(*validator_key_hash, *validator_adnl_key_hash),
        );
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error add_validator_adnl_key: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::Result(result))) => result,
            Some(Some(_)) => fail!("Bad answer (AddValidatorAdnlKey)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    // Unused
    ///// returns validator's public key
    //    pub fn get_current_validator_key(&self, vset: &ValidatorSet) -> Option<[u8; 32]> {
    //        // search by adnl_id in validator_keys first
    //        for id_key in self.validator_keys.values.iter() {
    //            if let Some(adnl_id) = id_key.1.validator_adnl_key_id.as_ref() {
    //                match UInt256::from_str(adnl_id) {
    //                    Ok(adnl_id) => {
    //                        let pub_key_opt = vset.list().iter().find_map(|descr| {
    //                            if descr.adnl_addr.as_ref() == Some(&adnl_id) {
    //                                Some(descr.public_key.key_bytes().clone())
    //                            } else {
    //                                None
    //                            }
    //                        });
    //                        if let Some(pub_key) = pub_key_opt.as_ref() {
    //                            log::info!("get_current_validator_key returns pub_key {}", hex::encode(pub_key));
    //                            return pub_key_opt
    //                        }
    //                    }
    //                    Err(err) => log::warn!("adnl_id error: {}", err)
    //                }
    //            }
    //        }
    //        // then search by key_id from vset in keyring
    //        for descr in vset.list().iter() {
    //            let key_id = base64_encode(descr.compute_node_id_short().as_slice());
    //            let pub_key_found = self.key_ring.iter().position(|k_v| k_v.0 == key_id).is_some();
    //            if pub_key_found {
    //                log::info!("get_current_validator_key returns pub_key {}", hex::encode(descr.public_key.key_bytes()));
    //                return Some(descr.public_key.key_bytes().clone())
    //            }
    //        }
    //        log::warn!("get_current_validator_key key not found");
    //        None
    //    }

    // Unused
    //    pub fn workchain_id(&self) -> Option<i32> {
    //        self.workchain_id
    //    }

    pub fn get_actual_validator_adnl_key_ids(&self) -> Result<Vec<Arc<KeyId>>> {
        self.get_actual_key_ids(|| self.validator_keys.get_validator_adnl_key_ids())
    }

    pub fn get_actual_validator_key_ids(&self) -> Result<Vec<Arc<KeyId>>> {
        self.get_actual_key_ids(|| self.validator_keys.get_validator_key_ids())
    }

    pub fn get_actual_validator_keys(&self) -> Result<Vec<ValidatorKeysJson>> {
        let mut result = Vec::new();
        for validator_key_id in self.validator_keys.get_validator_key_ids().into_iter() {
            let Some(key) = self.validator_keys.get(&validator_key_id) else {
                log::warn!("Validator key id {validator_key_id} not found in validator_keys");
                continue;
            };
            result.push(key);
        }
        Ok(result)
    }

    pub async fn add_custom_overlay(
        &self,
        overlay: CustomOverlay,
    ) -> Result<CustomOverlaysConfigBoxed> {
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::AddCustomOverlay(overlay));
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error add_custom_overlay: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::CustomOverlays(r))) => r,
            Some(Some(_)) => fail!("Bad answer (Result needed)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    pub async fn del_custom_overlay(&self, name: String) -> Result<CustomOverlaysConfigBoxed> {
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::DelCustomOverlay(name));
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error del_custom_overlay: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::CustomOverlays(r))) => r,
            Some(Some(_)) => fail!("Bad answer (Result needed)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    pub async fn show_custom_overlays(&self) -> Result<CustomOverlaysConfigBoxed> {
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::GetCustomOverlays);
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error show_custom_overlays: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::CustomOverlays(r))) => r,
            Some(Some(_)) => fail!("Bad answer (Result needed)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    pub async fn get_adnl_keys(&self) -> Result<Vec<(String, usize)>> {
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::ExportAdnlKeys);
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            log::warn!("Error get_adnl_keys {e}");
            return Ok(vec![]);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(Some(Answer::AdnlKeys(keys))) => Ok(keys),
            _ => Ok(vec![]),
        }
    }

    pub async fn get_validator_key(
        &self,
        key_id: &Arc<KeyId>,
    ) -> Option<(Arc<dyn KeyOption>, i32)> {
        match self.validator_keys.get(&base64_encode(key_id.data())) {
            Some(key) => {
                //       let result = if let Some(key) = self.key_ring.get(&key_id) {
                //           Some(key.(val(), key_election_id))

                if let Some(key_opt) = self.get_key_raw(*key_id.data()).await {
                    Some((key_opt, key.election_id))
                } else {
                    None
                }
            }
            None => None,
        }
    }

    async fn get_key_raw(&self, key_hash: [u8; 32]) -> Option<Arc<dyn KeyOption>> {
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::GetKey(key_hash));
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            log::warn!("Error get_key_raw {}", e);
            return None;
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(Some(Answer::GetKey(key))) => key,
            _ => None,
        }
    }

    fn get_actual_key_ids(&self, src: impl Fn() -> Vec<String>) -> Result<Vec<Arc<KeyId>>> {
        let mut result = Vec::new();
        for key_id in src().iter() {
            let id = base64_decode(key_id)?;
            result.push(KeyId::from_data(id[..].try_into()?));
        }
        Ok(result)
    }

    // generates a new key and saves it to config
    // returns public key hash as key_id
    async fn generate_and_save(
        key_ring: &Arc<lockfree::map::Map<String, Arc<dyn KeyOption>>>,
        key_type: i32,
        config: &mut TonNodeConfig,
        config_name: &str,
    ) -> Result<[u8; 32]> {
        log::info!("start generate key (type: {})", key_type);
        let (key_id, public_key) = config.generate_and_save_keys(key_type)?;
        config.save_to_file(config_name).await?;

        let id = base64_encode(key_id);
        log::info!("finish generate key (type: {key_type}), key_id: {id}");
        key_ring.insert(id, public_key.clone());
        Ok(key_id)
    }

    async fn import_private_key(
        key_ring: &Arc<lockfree::map::Map<String, Arc<dyn KeyOption>>>,
        key: PrivateKey,
        config: &mut TonNodeConfig,
        config_name: &str,
    ) -> Result<[u8; 32]> {
        log::info!("start import private key");
        let (key_id, public_key) = config.import_private_key(key)?;
        config.save_to_file(config_name).await?;

        let id = base64_encode(key_id);
        key_ring.insert(id, public_key.clone());
        log::info!("finish import private key, key_id: {}", base64_encode(key_id));
        Ok(key_id)
    }

    async fn revision_validator_keys(
        validator_keys: &Arc<ValidatorKeys>,
        config: &mut TonNodeConfig,
    ) -> Result<()> {
        if let Some(config_validator_keys) = &config.validator_keys {
            if config_validator_keys.len() > 2 {
                let oldest_validator_key = NodeConfigHandler::get_oldest_validator_key(config);
                if let Some(oldest_key) = oldest_validator_key {
                    config
                        .remove_validator_key(
                            oldest_key.validator_key_id.clone(),
                            oldest_key.election_id,
                        )
                        .await?;
                    validator_keys.remove(&oldest_key)?;
                    config.remove_key_from_key_ring(&oldest_key.validator_key_id.clone()).await?;
                    if let Some(adnl_key_id) = oldest_key.validator_adnl_key_id {
                        config.remove_key_from_key_ring(&adnl_key_id).await?;
                    }
                }
            }
        }
        Ok(())
    }

    async fn add_validator_adnl_key_and_save(
        self: Arc<Self>,
        validator_keys: &Arc<ValidatorKeys>,
        config: &mut TonNodeConfig,
        validator_key_hash: &[u8; 32],
        validator_adnl_key_hash: &[u8; 32],
        subscribers: &[Arc<dyn NodeConfigSubscriber>],
    ) -> Result<()> {
        let key = config.add_validator_adnl_key(validator_key_hash, validator_adnl_key_hash)?;
        let election_id = key.election_id;
        //if key.validator_adnl_key_id.is_some() {
        validator_keys.add(key)?;
        //}

        let adnl_key_id = KeyId::from_data(*validator_adnl_key_hash);

        for subscriber in subscribers.iter() {
            let subscriber = subscriber.clone();
            let adnl_key_id = adnl_key_id.clone();
            self.clone().runtime_handle.spawn(async move {
                if let Err(e) = subscriber
                    .event(ConfigEvent::AddValidatorAdnlKey(adnl_key_id, election_id))
                    .await
                {
                    log::warn!("add_validator_adnl_key_and_save subscriber error: {e:?}");
                }
            });
        }

        // check validator keys
        Self::revision_validator_keys(validator_keys, config).await?;
        config.save_to_file(&config.file_name).await?;
        Ok(())
    }

    async fn add_validator_key_and_save(
        validator_keys: Arc<ValidatorKeys>,
        config: &mut TonNodeConfig,
        key_id: &[u8; 32],
        election_id: i32,
        expire_at: i32,
    ) -> Result<()> {
        let key = config.add_validator_key(key_id, election_id, expire_at)?;
        validator_keys.add(key)?;
        config.save_to_file(&config.file_name).await?;
        Ok(())
    }

    async fn add_custom_overlay_and_save(
        config: &mut TonNodeConfig,
        overlay: CustomOverlay,
    ) -> Result<CustomOverlaysConfigBoxed> {
        let CustomOverlaysConfigBoxed::Engine_Validator_CustomOverlaysConfig(conf) =
            &mut config.custom_overlays;
        conf.overlays.push(overlay);
        TonNodeConfig::save_custom_overlays_json(
            &config.custom_overlays,
            &config.custom_overlays_file_name,
        )
        .await?;
        Ok(config.custom_overlays.clone())
    }

    async fn del_custom_overlay_and_save(
        config: &mut TonNodeConfig,
        name: String,
    ) -> Result<CustomOverlaysConfigBoxed> {
        let CustomOverlaysConfigBoxed::Engine_Validator_CustomOverlaysConfig(conf) =
            &mut config.custom_overlays;
        conf.overlays.retain(|overlay| overlay.name != name);
        TonNodeConfig::save_custom_overlays_json(
            &config.custom_overlays,
            &config.custom_overlays_file_name,
        )
        .await?;
        Ok(config.custom_overlays.clone())
    }

    fn get_oldest_validator_key(config: &TonNodeConfig) -> Option<ValidatorKeysJson> {
        let mut oldest_validator_key: Option<ValidatorKeysJson> = None;
        if let Some(validator_keys) = &config.validator_keys {
            for key in validator_keys.iter() {
                if let Some(oldest_val_key) = &oldest_validator_key {
                    if key.election_id < oldest_val_key.election_id {
                        oldest_validator_key = Some(key.clone());
                    }
                } else {
                    oldest_validator_key = Some(key.clone());
                }
            }
        }
        oldest_validator_key
    }

    fn get_key(config: &TonNodeConfig, key_id: [u8; 32]) -> Option<Arc<dyn KeyOption>> {
        if let Some(validator_key_ring) = &config.validator_key_ring {
            if let Some(key_data) = validator_key_ring.get(&base64_encode(key_id)) {
                match Ed25519KeyOption::from_private_key_json(key_data) {
                    Ok(key) => return Some(key),
                    _ => return None,
                }
            }
        }
        None
    }

    fn export_adnl_keys(actual_config: &TonNodeConfig) -> Vec<(String, usize)> {
        let mut result = Vec::new();
        if let Some(adnl_config) = &actual_config.adnl_node {
            if let Ok(overlay_key) = adnl_config.key_by_tag(NodeNetwork::TAG_OVERLAY_KEY, true) {
                result.push((base64_encode(overlay_key.id().data()), NodeNetwork::TAG_OVERLAY_KEY));
            }
            if let Ok(dht_key) = adnl_config.key_by_tag(NodeNetwork::TAG_DHT_KEY, true) {
                result.push((base64_encode(dht_key.id().data()), NodeNetwork::TAG_DHT_KEY));
            }
        }
        result
    }

    fn load_config(
        &self,
        config: &TonNodeConfig,
        subscribers: &[Arc<dyn NodeConfigSubscriber>],
    ) -> Result<()> {
        // load key ring
        if let Some(key_ring) = &config.validator_key_ring {
            for (key_id, key) in key_ring.iter() {
                if let Err(e) = self.add_key_to_dynamic_key_ring(key_id.to_string(), key) {
                    log::warn!("fail added key from key ring: {}", e);
                }
            }
        }

        // load validator keys
        if let Some(validator_keys) = &config.validator_keys {
            for key in validator_keys.iter() {
                if let Err(e) = self.validator_keys.add(key.clone()) {
                    log::warn!("fail added key to validator keys map: {}", e);
                }
                if let Some(validator_adnl_key_id) = &key.validator_adnl_key_id {
                    let adnl_key_id = base64_decode(validator_adnl_key_id)?;
                    let adnl_key_id = KeyId::from_data(adnl_key_id[..].try_into()?);
                    let election_id = key.election_id;
                    let subscribers = subscribers.to_vec();
                    self.runtime_handle.spawn(async move {
                        for subscriber in subscribers.iter() {
                            if let Err(e) = subscriber
                                .event(ConfigEvent::AddValidatorAdnlKey(
                                    adnl_key_id.clone(),
                                    election_id,
                                ))
                                .await
                            {
                                log::warn!("load_config subscriber error: {e:?}");
                            }
                        }
                    });
                }
            }
        }
        Ok(())
    }

    fn add_key_to_dynamic_key_ring(&self, key_id: String, key_json: &KeyOptionJson) -> Result<()> {
        let key = match *key_json.type_id() {
            Ed25519KeyOption::KEY_TYPE => Ed25519KeyOption::from_private_key_json(key_json)?,
            _ => fail!("Unknown key type (key_id: {})", key_id),
        };
        if let Some(key) = self.key_ring.insert(key.id().to_string(), key) {
            log::warn!("Added key was already in key ring collection (id: {})", key.key());
        }

        Ok(())
    }

    pub fn start_sheduler(
        self: Arc<Self>,
        config_handler_context: NodeConfigHandlerContext,
        subscribers: Vec<Arc<dyn NodeConfigSubscriber>>,
    ) -> Result<()> {
        let name = config_handler_context.config.file_name.clone();
        let mut actual_config = config_handler_context.config;
        let mut reader = config_handler_context.reader;
        let key_ring = self.key_ring.clone();
        let validator_keys = self.validator_keys.clone();
        self.load_config(&actual_config, &subscribers)?;

        self.clone().runtime_handle.spawn(async move {
            while let Some(task) = reader.recv().await {
                let answer = match task.1 {
                    Task::Generate(key_type) => {
                        let result = NodeConfigHandler::generate_and_save(
                            &key_ring,
                            key_type,
                            &mut actual_config,
                            &name,
                        )
                        .await;
                        Answer::Generate(result)
                    }
                    Task::ImportPrivateKey(ref key) => {
                        let result = NodeConfigHandler::import_private_key(
                            &key_ring,
                            key.clone(),
                            &mut actual_config,
                            &name,
                        )
                        .await;
                        Answer::ImportPrivateKey(result)
                    }
                    Task::AddValidatorAdnlKey(key, adnl_key) => {
                        let result = NodeConfigHandler::add_validator_adnl_key_and_save(
                            self.clone(),
                            &validator_keys,
                            &mut actual_config,
                            &key,
                            &adnl_key,
                            &subscribers,
                        )
                        .await;
                        Answer::Result(result)
                    }
                    Task::AddValidatorKey(key, election_id, expire_at) => {
                        let result = NodeConfigHandler::add_validator_key_and_save(
                            validator_keys.clone(),
                            &mut actual_config,
                            &key,
                            election_id,
                            expire_at,
                        )
                        .await;
                        Answer::Result(result)
                    }
                    Task::GetKey(key_data) => {
                        let result = NodeConfigHandler::get_key(&actual_config, key_data);
                        Answer::GetKey(result)
                    }
                    Task::ExportAdnlKeys => {
                        let keys = NodeConfigHandler::export_adnl_keys(&actual_config);
                        Answer::AdnlKeys(keys)
                    }
                    Task::AddCustomOverlay(overlay) => {
                        let result = NodeConfigHandler::add_custom_overlay_and_save(
                            &mut actual_config,
                            overlay,
                        )
                        .await;
                        Answer::CustomOverlays(result)
                    }
                    Task::DelCustomOverlay(name) => {
                        let result = NodeConfigHandler::del_custom_overlay_and_save(
                            &mut actual_config,
                            name,
                        )
                        .await;
                        Answer::CustomOverlays(result)
                    }
                    Task::GetCustomOverlays => {
                        Answer::CustomOverlays(Ok(actual_config.custom_overlays.clone()))
                    }
                };
                task.0.respond(Some(answer));
            }
            reader.close();
        });
        Ok(())
    }
}

#[async_trait::async_trait]
impl KeyRing for NodeConfigHandler {
    async fn generate(&self, key_type: i32) -> Result<[u8; 32]> {
        log::info!("request generate key (key_type: {})", key_type);
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::Generate(key_type));
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error generate: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::Generate(result))) => result,
            Some(Some(_)) => fail!("Bad answer (Generate)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    async fn import_private_key(&self, key: PrivateKey) -> Result<[u8; 32]> {
        log::info!("request import private key");
        let (wait, mut queue_reader) = Wait::new();
        let pushed_task = (wait.clone(), Task::ImportPrivateKey(key));
        wait.request();
        if let Err(e) = self.sender.send(pushed_task) {
            fail!("Error generate: {}", e);
        }
        match wait.wait(&mut queue_reader, true).await {
            Some(None) => fail!("Answer was not set!"),
            Some(Some(Answer::ImportPrivateKey(result))) => result,
            Some(Some(_)) => fail!("Bad answer (ImportPrivateKey)!"),
            None => fail!("Waiting returned an internal error!"),
        }
    }

    fn sign_data(&self, key_hash: &[u8; 32], data: &[u8]) -> Result<Vec<u8>> {
        let private = self.find(key_hash)?;
        Ok(private.sign(data)?.to_vec())
    }

    // find private key in KeyRing by public key hash
    fn find(&self, key_id: &[u8; 32]) -> Result<Arc<dyn KeyOption>> {
        let id = base64_encode(key_id);
        match self.key_ring.get(&id) {
            Some(key) => Ok(key.val().clone()),
            None => fail!("key not found for hash: {}", &id),
        }
    }
}

impl TonNodeGlobalConfig {
    /// Constructor from json file
    pub fn from_json_file(json_file: impl AsRef<Path>) -> Result<Self> {
        let ton_node_global_cfg_json = TonNodeGlobalConfigJson::from_json_file(json_file)?;
        Ok(TonNodeGlobalConfig(ton_node_global_cfg_json))
    }
    /*
        pub fn from_json(json : &str) -> Result<Self> {
            let ton_node_global_cfg_json = TonNodeGlobalConfigJson::from_json(&json)?;
            Ok(TonNodeGlobalConfig(ton_node_global_cfg_json))
        }
    */

    pub fn zero_state(&self) -> Result<BlockIdExt> {
        self.0.zero_state()
    }

    pub fn init_block(&self) -> Result<Option<BlockIdExt>> {
        self.0.init_block()
    }

    pub fn hardforks(&self) -> Result<Vec<BlockIdExt>> {
        self.0.hardforks()
    }

    pub fn dht_nodes(&self) -> Result<Vec<DhtNodeConfig>> {
        self.0.get_dht_nodes_configs()
    }

    // Unused
    //    pub fn dht_param_a(&self) -> Result<i32> {
    //        self.0.dht.a.ok_or_else(|| error!("Dht param a is not set!"))
    //    }

    // Unused
    //    pub fn dht_param_k(&self) -> Result<i32> {
    //        self.0.dht.k.ok_or_else(|| error!("Dht param k is not set!"))
    //    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct TonNodeGlobalConfigJson {
    #[serde(alias = "@type")]
    type_node: String,
    dht: DhtGlobalConfig,
    validator: Validator,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct DhtGlobalConfig {
    #[serde(alias = "@type")]
    type_dht: Option<String>,
    k: Option<i32>,
    a: Option<i32>,
    static_nodes: DhtNodes,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct DhtNodes {
    #[serde(alias = "@type")]
    type_dht: Option<String>,
    nodes: Vec<DhtNode>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct DhtNode {
    #[serde(alias = "@type")]
    type_node: Option<String>,
    id: IdDhtNode,
    addr_list: AddressList,
    version: Option<i32>,
    signature: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct IdDhtNode {
    #[serde(alias = "@type")]
    type_node: Option<String>,
    key: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct AddressList {
    #[serde(alias = "@type")]
    type_node: Option<String>,
    addrs: Vec<Address>,
    version: Option<i32>,
    reinit_date: Option<i32>,
    priority: Option<i32>,
    expire_at: Option<i32>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct Address {
    #[serde(alias = "@type")]
    type_node: Option<String>,
    ip: Option<i64>,
    port: Option<u16>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct Validator {
    #[serde(alias = "@type")]
    type_node: Option<String>,
    zero_state: ConfigBlockId,
    init_block: Option<ConfigBlockId>,
    hardforks: Vec<ConfigBlockId>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ConfigBlockId {
    workchain: Option<i32>,
    shard: Option<i64>,
    seqno: Option<i32>,
    root_hash: Option<String>,
    file_hash: Option<String>,
}

pub const PUB_ED25519: &str = "pub.ed25519";

impl IdDhtNode {
    pub fn convert_key(&self) -> Result<Arc<dyn KeyOption>> {
        let type_id = self.type_node.as_ref().ok_or_else(|| error!("Type_node is not set!"))?;

        if !type_id.eq(PUB_ED25519) {
            fail!("unknown type_node!")
        };

        let key = if let Some(key) = &self.key {
            base64_decode(key)?
        } else {
            fail!("No public key!");
        };

        let pub_key = key[..32].try_into()?;
        Ok(Ed25519KeyOption::from_public_key(pub_key))
    }
}

impl TonNodeGlobalConfigJson {
    /// Constructs new configuration from JSON data
    pub fn from_json_file(json_file: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(json_file.as_ref())
            .map_err(|err| error!("cannot open file {:?} : {}", json_file.as_ref(), err))?;
        let reader = BufReader::new(file);
        Ok(serde_json::from_reader(reader)?)
    }
    /*
        pub fn from_json(json: &str) -> Result<Self> {
            let json_config: TonNodeGlobalConfigJson = serde_json::from_str(json)?;
            Ok(json_config)
        }
    */

    pub fn get_dht_nodes_configs(&self) -> Result<Vec<DhtNodeConfig>> {
        let mut result = Vec::new();
        for dht_node in self.dht.static_nodes.nodes.iter() {
            let key = dht_node.id.convert_key()?;
            let mut addrs = Vec::new();
            for addr in dht_node.addr_list.addrs.iter() {
                let ip = if let Some(ip) = addr.ip {
                    ip
                } else {
                    continue;
                };
                let port = if let Some(port) = addr.port { port } else { continue };
                let addr = Udp { ip: ip as i32, port: port as i32 }.into_boxed();
                addrs.push(addr);
            }
            let version =
                if let Some(version) = dht_node.addr_list.version { version } else { continue };
            let reinit_date = if let Some(reinit_date) = dht_node.addr_list.reinit_date {
                reinit_date
            } else {
                continue;
            };
            let priority =
                if let Some(priority) = dht_node.addr_list.priority { priority } else { continue };
            let expire_at = if let Some(expire_at) = dht_node.addr_list.expire_at {
                expire_at
            } else {
                continue;
            };
            let addr_list = AdnlAddressList { addrs, version, reinit_date, priority, expire_at };
            let version = if let Some(version) = dht_node.version { version } else { continue };
            let signature =
                if let Some(signature) = &dht_node.signature { signature } else { continue };
            let node = DhtNodeConfig {
                id: Ed25519 { key: UInt256::with_array(key.pub_key()?.try_into()?) }.into_boxed(),
                addr_list,
                version,
                signature: base64_decode(signature)?,
            };
            result.push(node) //convert_to_dht_node_cfg()?);
        }
        Ok(result)
    }

    fn parse_block_id(&self, block_id: &ConfigBlockId) -> Result<BlockIdExt> {
        let workchain_id =
            block_id.workchain.ok_or_else(|| error!("Unknown workchain id (of zero_state)!"))?;

        let seqno =
            block_id.seqno.ok_or_else(|| error!("Unknown workchain seqno (of zero_state)!"))?;

        let shard =
            block_id.shard.ok_or_else(|| error!("Unknown workchain shard (of zero_state)!"))?;

        let root_hash = block_id
            .root_hash
            .as_ref()
            .ok_or_else(|| error!("Unknown workchain root_hash (of zero_state)!"))?
            .parse()?;

        let file_hash = block_id
            .file_hash
            .as_ref()
            .ok_or_else(|| error!("Unknown workchain file_hash (of zero_state)!"))?
            .parse()?;

        Ok(BlockIdExt {
            shard_id: ShardIdent::with_tagged_prefix(workchain_id, shard as u64)?,
            seq_no: seqno as u32,
            root_hash,
            file_hash,
        })
    }

    pub fn zero_state(&self) -> Result<BlockIdExt> {
        self.parse_block_id(&self.validator.zero_state)
            .map_err(|err| error!("zero state parse error: {}", err))
    }

    pub fn init_block(&self) -> Result<Option<BlockIdExt>> {
        match self.validator.init_block {
            Some(ref init_block) => match self.parse_block_id(init_block) {
                Ok(block_id) => Ok(Some(block_id)),
                Err(err) => fail!("init block parse error: {}", err),
            },
            None => Ok(None),
        }
    }

    fn hardforks(&self) -> Result<Vec<BlockIdExt>> {
        log::info!("hardforks count {}", self.validator.hardforks.len());
        self.validator.hardforks.iter().try_fold(Vec::new(), |mut vec, block_id| {
            match self.parse_block_id(block_id) {
                Ok(block_id) => {
                    vec.push(block_id);
                    Ok(vec)
                }
                Err(err) => fail!("hardforks parse error: {}", err),
            }
        })
    }
}

pub struct ValidatorManagerConfig {
    pub update_interval: Duration,
    pub unsafe_resync_catchains: HashSet<u32>,
    /// Maps catchain_seqno to block_seqno and unsafe rotation id
    pub unsafe_catchain_rotates: HashMap<u32, (u32, u32)>,
    pub no_countdown_for_zerostate: bool,
    pub accelerated_consensus_disabled: bool,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct UnsafeCatchainRotation {
    catchain_seqno: u32,
    block_seqno: u32,
    unsafe_rotation_id: u32,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct ValidatorManagerConfigImpl {
    unsafe_resync_catchains: Vec<u32>,
    unsafe_catchain_rotates: Vec<UnsafeCatchainRotation>,
}

impl Display for ValidatorManagerConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "validation countdown mode: {}; update interval: {} ms; resync: [{}]; rotates: [{}]",
            if self.no_countdown_for_zerostate { "except-zerostate" } else { "always" },
            self.update_interval.as_millis(),
            self.unsafe_resync_catchains.iter().map(|n| format!("{} ", n)).collect::<String>(),
            self.unsafe_catchain_rotates
                .iter()
                .map(|(cc, (blk, uid))| format!("({},{})=>{} ", cc, blk, uid))
                .collect::<String>()
        )
    }
}

impl ValidatorManagerConfig {
    pub fn read_configs(
        config_files: Vec<String>,
        validation_countdown_mode: Option<String>,
        accelerated_consensus_disabled: bool,
    ) -> ValidatorManagerConfig {
        log::debug!(target: "validator", "Reading validator manager config files: {}",
            config_files.iter().map(|x| format!("{}; ",x)).collect::<String>());

        let mut validator_config = ValidatorManagerConfig::default();
        match validation_countdown_mode {
            Some(x) if x == "always" => validator_config.no_countdown_for_zerostate = false,
            Some(x) if x == "except-zerostate" => {
                validator_config.no_countdown_for_zerostate = true
            }
            Some(x) => log::error!(
                "Incorrect option: validation_countdown_mode must be either 'always' or 'except-zerostate', '{}' found",
                x
            ),
            None => (),
        }

        validator_config.accelerated_consensus_disabled = accelerated_consensus_disabled;

        'iterate_configs: for one_config in config_files.into_iter() {
            if let Ok(config_file) = std::fs::File::open(one_config.clone()) {
                let reader = std::io::BufReader::new(config_file);
                let config: ValidatorManagerConfigImpl = match serde_json::from_reader(reader) {
                    Err(e) => {
                        log::warn!(
                            "Not ValidatorManagerConfig, but expected to be: {}, error: {}",
                            one_config,
                            e
                        );
                        continue 'iterate_configs;
                    }
                    Ok(cfg) => cfg,
                };

                for resync in config.unsafe_resync_catchains.into_iter() {
                    validator_config.unsafe_resync_catchains.insert(resync);
                }

                for rotate in config.unsafe_catchain_rotates.into_iter() {
                    validator_config.unsafe_catchain_rotates.insert(
                        rotate.catchain_seqno,
                        (rotate.block_seqno, rotate.unsafe_rotation_id),
                    );
                }
            }
        }

        log::info!(target: "validator", "Validator manager config has been read: {}", validator_config);

        validator_config
    }

    pub fn check_unsafe_catchain_rotation(
        &self,
        block_seqno_opt: Option<u32>,
        catchain_seqno: u32,
    ) -> Option<u32> {
        if let Some(blk) = block_seqno_opt {
            match self.unsafe_catchain_rotates.get(&catchain_seqno) {
                Some((required_block_seqno, rotation_id)) if *required_block_seqno <= blk => {
                    Some(*rotation_id)
                }
                _ => None,
            }
        } else {
            None
        }
    }
}

impl Default for ValidatorManagerConfig {
    fn default() -> Self {
        ValidatorManagerConfig {
            update_interval: Duration::from_secs(3),
            unsafe_resync_catchains: HashSet::new(),
            unsafe_catchain_rotates: HashMap::new(),
            no_countdown_for_zerostate: false,
            accelerated_consensus_disabled: false,
        }
    }
}

struct ValidatorKeys {
    values: lockfree::map::Map<i32, ValidatorKeysJson>, // election_id, keys_info
    index: lockfree::map::Map<i32, i32>,                // current_election_id, next_election_id
    first: AtomicI32,
}

impl ValidatorKeys {
    fn new() -> Self {
        ValidatorKeys {
            values: lockfree::map::Map::new(),
            index: lockfree::map::Map::new(),
            first: AtomicI32::new(0),
        }
    }

    fn is_empty(&self) -> bool {
        self.first.load(atomic::Ordering::Relaxed) > 0
    }

    fn add(&self, key: ValidatorKeysJson) -> Result<()> {
        // inserted in sorted order
        let mut first = false;

        add_unbound_object_to_map_with_update(&self.values, key.election_id, |_| {
            if self
                .first
                .compare_exchange(
                    0,
                    key.election_id,
                    atomic::Ordering::Relaxed,
                    atomic::Ordering::Relaxed,
                )
                .is_ok()
            {
                first = true;
            }
            Ok(Some(key.clone()))
        })?;

        if first {
            return Ok(());
        }

        let mut current = self.first.load(atomic::Ordering::Relaxed);
        match current.cmp(&key.election_id) {
            std::cmp::Ordering::Greater => {
                add_unbound_object_to_map_with_update(&self.index, key.election_id, |_| {
                    if let Err(prev) = self.first.fetch_update(
                        atomic::Ordering::Relaxed,
                        atomic::Ordering::Relaxed,
                        |x| {
                            if x > key.election_id {
                                Some(key.election_id)
                            } else {
                                None
                            }
                        },
                    ) {
                        let old = self
                            .index
                            .insert(prev, key.election_id)
                            .ok_or_else(|| error!("validator keys collections was broken!"))?;
                        Ok(Some(*old.val()))
                    } else {
                        Ok(Some(current))
                    }
                })?;
                return Ok(());
            }
            std::cmp::Ordering::Equal => {
                return Ok(());
            }
            std::cmp::Ordering::Less => (),
        }

        loop {
            if let Some(item) = &self.index.get(&current) {
                match item.val().cmp(&key.election_id) {
                    std::cmp::Ordering::Greater => {
                        add_unbound_object_to_map_with_update(&self.index, *item.key(), |_| {
                            self.index.insert(key.election_id, *item.val());
                            Ok(Some(key.election_id))
                        })?;
                        break;
                    }
                    std::cmp::Ordering::Equal => {
                        break;
                    }
                    std::cmp::Ordering::Less => {
                        current = *item.val();
                    }
                }
            } else {
                self.index.insert(current, key.election_id);
                break;
            };
        }

        Ok(())
    }

    fn remove(&self, key: &ValidatorKeysJson) -> Result<bool> {
        let mut current = self.first.load(atomic::Ordering::Relaxed);

        if current == key.election_id {
            if let Some(item) = &self.index.get(&current) {
                self.first.store(*item.val(), atomic::Ordering::Relaxed);
            } else {
                self.first.store(0, atomic::Ordering::Relaxed);
            }
            return Ok(true);
        }

        while let Some(item) = &self.index.get(&current) {
            if item.val() == &key.election_id {
                if let Some(removed_item) = &self.index.get(item.val()) {
                    self.index.insert(*item.key(), *removed_item.val());
                } else {
                    // remove last element
                    self.index.remove(item.key());
                }
                return Ok(true);
            } else {
                current = *item.val();
            }
        }
        Ok(false)
    }

    fn get(&self, id_key: &str) -> Option<ValidatorKeysJson> {
        let mut current = self.first.load(atomic::Ordering::Relaxed);
        loop {
            if let Some(result) = self.get_try(id_key, current) {
                return Some(result);
            }
            match self.index.get(&current) {
                Some(next) => current = *next.val(),
                None => return None,
            }
        }
    }

    fn get_try(&self, id_key: &str, index: i32) -> Option<ValidatorKeysJson> {
        let mut result = None;
        if let Some(key) = self.values.get(&index) {
            if key.val().validator_key_id == id_key {
                result = Some(key.val().clone());
            } else if let Some(adnl_key) = &key.val().validator_adnl_key_id {
                if adnl_key == id_key {
                    result = Some(key.val().clone());
                }
            }
        }
        result
    }

    fn get_validator_adnl_key_ids(&self) -> Vec<String> {
        let mut adnl_ids = Vec::new();
        let mut current = self.first.load(atomic::Ordering::Relaxed);
        loop {
            if let Some(validator_info) = self.values.get(&current) {
                if let Some(adnl_key) = &validator_info.val().validator_adnl_key_id {
                    adnl_ids.push(adnl_key.clone());
                } else {
                    adnl_ids.push(validator_info.val().validator_key_id.clone());
                }
            }
            match self.index.get(&current) {
                Some(next) => current = *next.val(),
                None => return adnl_ids,
            }
        }
    }

    fn get_validator_key_ids(&self) -> Vec<String> {
        let mut key_ids = Vec::new();
        let mut current = self.first.load(atomic::Ordering::Relaxed);
        loop {
            if let Some(validator_info) = self.values.get(&current) {
                key_ids.push(validator_info.val().validator_key_id.clone());
            }
            match self.index.get(&current) {
                Some(next) => current = *next.val(),
                None => return key_ids,
            }
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct JsonRpcServerConfigJson {
    pub address: String,
}
#[derive(Debug)]
pub struct JsonRpcServerConfig {
    pub address: SocketAddr,
}
impl JsonRpcServerConfig {
    /// Construct from JSON config structure
    pub fn from_json_config(json: &JsonRpcServerConfigJson) -> Result<Self> {
        Ok(Self { address: json.address.parse()? })
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct MetricsConfigJson {
    pub address: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub histogram_buckets: HashMap<String, Vec<f64>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub global_labels: HashMap<String, String>,
}

const DEFAULT_TIME_BUCKETS: &[f64] = &[
    0.000001, 0.0001, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 60.0,
    120.0, 300.0, 600.0, 3600.0,
];

#[derive(Debug)]
pub struct MetricsConfig {
    pub address: SocketAddr,
    pub histogram_buckets: HashMap<String, Vec<f64>>,
    pub global_labels: HashMap<String, String>,
}
impl MetricsConfig {
    pub fn from_json_config(json: &MetricsConfigJson) -> Result<Self> {
        let mut histogram_buckets = json.histogram_buckets.clone();
        if !histogram_buckets.contains_key("seconds") {
            histogram_buckets.insert("seconds".to_string(), DEFAULT_TIME_BUCKETS.to_vec());
        }
        Ok(Self {
            address: json.address.parse()?,
            histogram_buckets,
            global_labels: json.global_labels.clone(),
        })
    }
}

#[cfg(test)]
#[path = "tests/test_config.rs"]
mod tests;
