/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use common::{
    app_config::{AppConfig, ElectionsConfig, KeyConfig, PoolConfig, WalletConfig},
    time_format,
    ton_utils::extract_max_factor,
    vault_signer::VaultSigner,
};
use contracts::{
    NominatorWrapper, NominatorWrapperImpl, TonWallet, WalletContract, contract_provider,
};
use secrets_vault::{
    types::{algorithm::Algorithm, secret_id::SecretId, secret_spec::SecretSpec},
    vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::{
    collections::HashMap,
    path::Path,
    str::FromStr,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};
use ton_block::MsgAddressInt;
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

pub struct RuntimeConfigStore {
    /// Combined config + dynamic state, swapped atomically on updates.
    state: RwLock<Arc<RuntimeState>>,
    /// Unix timestamp of the last config mutation (seconds).
    updated_at: AtomicU64,
    /// Path to the config file on disk, used for save/reload.
    config_path: String,
    /// Hash of the last config file content we loaded, to detect external changes.
    last_file_hash: Mutex<Option<u64>>,
}

struct RuntimeState {
    /// Current application configuration.
    config: Arc<AppConfig>,
    /// Optional secrets vault for key management.
    vault: Option<Arc<SecretVault>>,
    /// Lazily-loaded nominator pools, rebuilt when config changes.
    pools: Arc<HashMap<String, Arc<dyn NominatorWrapper>>>,
    /// Lazily-loaded wallets, rebuilt when config changes.
    wallets: Arc<HashMap<String, Arc<dyn TonWallet>>>,
    /// Shared TON HTTP API JSON-RPC client.
    rpc_client: Arc<ClientJsonRpc>,
    /// Master wallet used for service-level operations (deploy, transfers).
    master_wallet: Arc<dyn TonWallet>,
}

#[derive(Debug, Clone)]
pub struct RuntimeConfigError {
    message: &'static str,
}

impl std::fmt::Display for RuntimeConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for RuntimeConfigError {}

// Public API for the runtime config
pub trait RuntimeConfig: Send + Sync {
    fn get(&self) -> Arc<AppConfig>;
    fn master_wallet(&self) -> Arc<dyn TonWallet>;
    fn pools(&self) -> Arc<HashMap<String, Arc<dyn NominatorWrapper>>>;
    fn wallets(&self) -> Arc<HashMap<String, Arc<dyn TonWallet>>>;
    fn rpc_client(&self) -> Arc<ClientJsonRpc>;
    fn vault(&self) -> Option<Arc<SecretVault>>;
    fn update_config(&self, f: Box<dyn FnOnce(&mut AppConfig) + Send>) -> anyhow::Result<()>;
    fn save_to_file(&self);
}

impl RuntimeConfigStore {
    /// Initializes the runtime config store with the given app config.
    ///
    /// Creates an rpc client, opens vault and loads the master wallet.
    /// If any operation fails, returns an error.
    pub async fn initialize(app_cfg: Arc<AppConfig>, config_path: String) -> anyhow::Result<Self> {
        let hash = Self::hash_file(&Path::new(&config_path));

        let vault = Some(SecretVaultBuilder::from_env().await?);
        let rpc_client = Self::load_rpc_client(&app_cfg).await?;
        if let Some(elections) = app_cfg.elections.as_ref() {
            Self::validate_elections_max_factor_vs_chain(&rpc_client, elections).await?;
        }
        let master_wallet =
            Self::load_master_wallet(&app_cfg, rpc_client.clone(), vault.clone()).await?;
        let wallets = Self::load_wallets(&app_cfg, rpc_client.clone(), vault.clone()).await?;
        let pools = Self::load_pools(&app_cfg, rpc_client.clone(), &wallets).await?;

        Ok(Self {
            state: RwLock::new(Arc::new(RuntimeState {
                config: app_cfg,
                vault,
                pools,
                wallets,
                rpc_client,
                master_wallet,
            })),
            updated_at: AtomicU64::new(time_format::now()),
            config_path,
            last_file_hash: Mutex::new(hash),
        })
    }

    async fn reload(&self, new_config: AppConfig) -> anyhow::Result<()> {
        let vault = SecretVaultBuilder::from_env().await.context("failed to reopen vault")?;
        let rpc_client = Self::load_rpc_client(&new_config).await?;
        if let Some(elections) = new_config.elections.as_ref() {
            Self::validate_elections_max_factor_vs_chain(&rpc_client, elections).await?;
        }
        let master_wallet =
            Self::load_master_wallet(&new_config, rpc_client.clone(), Some(vault.clone())).await?;
        let wallets =
            Self::load_wallets(&new_config, rpc_client.clone(), Some(vault.clone())).await?;
        let pools = Self::load_pools(&new_config, rpc_client.clone(), &wallets).await?;

        let new_state = Arc::new(RuntimeState {
            config: Arc::new(new_config),
            vault: Some(vault),
            pools,
            wallets,
            rpc_client,
            master_wallet,
        });
        *self.state.write().map_err(|e| anyhow::anyhow!("state lock poisoned: {e}"))? = new_state;
        self.updated_at.store(time_format::now(), Ordering::Relaxed);
        Ok(())
    }

    async fn validate_elections_max_factor_vs_chain(
        rpc_client: &ClientJsonRpc,
        elections: &ElectionsConfig,
    ) -> anyhow::Result<()> {
        match rpc_client.get_config_param(17).await.and_then(extract_max_factor) {
            Ok(network_max_factor) => elections.validate(Some(network_max_factor)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "elections max_factor: failed to read config param 17 from chain; \
                     validating without network upper bound (re-check max_factor when TON HTTP API is reachable)"
                );
                elections.validate(None)
            }
        }
    }

    #[cfg(test)]
    pub fn from_app_config(app_config: Arc<AppConfig>) -> Self {
        use contracts::SmartContract;
        use ton_block::{Cell, StateInit};

        struct NoopWallet;
        #[async_trait::async_trait]
        impl SmartContract for NoopWallet {
            fn address(&self) -> MsgAddressInt {
                MsgAddressInt::with_standart(None, 0, [0u8; 32].into()).unwrap()
            }
            async fn balance(&self) -> anyhow::Result<u64> {
                Ok(0)
            }
        }
        #[async_trait::async_trait]
        impl TonWallet for NoopWallet {
            async fn message(
                &self,
                _dest: MsgAddressInt,
                _value: u64,
                _payload: Cell,
            ) -> anyhow::Result<Cell> {
                unimplemented!()
            }

            async fn deploy_message(&self, _value: u64, _payload: Cell) -> anyhow::Result<Cell> {
                unimplemented!()
            }

            async fn build_message(
                &self,
                _dest: MsgAddressInt,
                _value: u64,
                _payload: Cell,
                _bounce: bool,
                _seqno: Option<u32>,
                _state_init_external: Option<StateInit>,
                _state_init_internal: Option<StateInit>,
            ) -> anyhow::Result<Cell> {
                unimplemented!()
            }
        }
        let master_wallet = Arc::new(NoopWallet);
        let rpc_client = Arc::new(
            ClientJsonRpc::connect_many(
                app_config.ton_http_api.resolved_endpoints(),
                app_config.ton_http_api.api_key.clone(),
            )
            .unwrap(),
        );
        Self {
            state: RwLock::new(Arc::new(RuntimeState {
                config: app_config,
                vault: None,
                pools: Arc::new(HashMap::new()),
                wallets: Arc::new(HashMap::new()),
                master_wallet,
                rpc_client,
            })),
            updated_at: AtomicU64::new(time_format::now()),
            config_path: "noop".to_string(),
            last_file_hash: Mutex::new(None),
        }
    }

    pub fn updated_at(&self) -> u64 {
        self.updated_at.load(Ordering::Relaxed)
    }

    /// Updates the config by cloning the current state, applying the mutation
    /// to its config, and atomically swapping in the new snapshot.
    pub fn update_with<F>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut AppConfig),
    {
        let mut guard =
            self.state.write().map_err(|e| anyhow::anyhow!("state lock poisoned: {e}"))?;
        let old = Arc::clone(&guard);
        let mut cfg = (*old.config).clone();
        f(&mut cfg);
        *guard = Arc::new(RuntimeState {
            config: Arc::new(cfg),
            vault: old.vault.clone(),
            pools: Arc::clone(&old.pools),
            wallets: Arc::clone(&old.wallets),
            rpc_client: Arc::clone(&old.rpc_client),
            master_wallet: Arc::clone(&old.master_wallet),
        });
        self.updated_at.store(time_format::now(), Ordering::Relaxed);
        Ok(())
    }

    /// Save the current in-memory config to the config file if it has changed.
    /// Only saves the `bindings` `enable` field and `elections` section.
    pub fn save_to_file(&self) {
        let path = Path::new(&self.config_path);
        let config = self.get();
        match serde_json::to_string_pretty(&*config) {
            Ok(json) => {
                let current_hash = Self::hash_bytes(&json.as_bytes());
                let last_hash = *self.last_file_hash.lock().expect("last_file_hash lock");
                if Some(current_hash) == last_hash {
                    return;
                }
                if let Err(e) = std::fs::write(path, &json) {
                    tracing::error!("save config error: path='{}' error={}", path.display(), e);
                } else {
                    tracing::debug!("config saved to '{}'", path.display());
                    // Update the file hash so we don't treat our own write as an external change.
                    *self.last_file_hash.lock().expect("last_file_hash lock") = Some(current_hash);
                }
            }
            Err(e) => {
                tracing::error!("serialize config error: {}", e);
            }
        }
    }

    /// Reload config from the file if it has changed externally.
    pub async fn reload_from_file(&self) -> bool {
        let current_hash = Self::hash_file(&Path::new(&self.config_path));
        let last_hash = *self.last_file_hash.lock().expect("last_file_hash lock");
        if current_hash == last_hash {
            return false;
        }

        tracing::info!("config changed, reloading from '{}'", self.config_path);
        match AppConfig::load(Path::new(&self.config_path)) {
            Ok(file_cfg) => match self.reload(file_cfg).await {
                Ok(()) => {
                    *self.last_file_hash.lock().expect("last_file_hash lock") = current_hash;
                    true
                }
                Err(e) => {
                    tracing::error!("reload config error: {:#}", e);
                    false
                }
            },
            Err(e) => {
                tracing::error!("reload config error: path='{}' error={:#}", self.config_path, e);
                false
            }
        }
    }

    fn hash_file(path: &Path) -> Option<u64> {
        std::fs::read(path).ok().map(|data| Self::hash_bytes(&data))
    }

    fn hash_bytes(data: &[u8]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut hasher);
        hasher.finish()
    }

    async fn load_rpc_client(app_cfg: &AppConfig) -> anyhow::Result<Arc<ClientJsonRpc>> {
        let resolved = app_cfg.ton_http_api.resolved_endpoints();
        let rpc_client = Arc::new(
            ClientJsonRpc::connect_many(resolved.clone(), app_cfg.ton_http_api.api_key.clone())
                .context("ton api connection error")?,
        );
        let urls: Vec<&str> = resolved.iter().map(|(u, _)| u.as_str()).collect();
        tracing::info!("connected to ton api endpoints: {}", urls.join(", "));
        Ok(rpc_client)
    }

    async fn load_master_wallet(
        app_cfg: &AppConfig,
        rpc_client: Arc<ClientJsonRpc>,
        vault: Option<Arc<SecretVault>>,
    ) -> anyhow::Result<Arc<dyn TonWallet>> {
        let master_config = app_cfg
            .master_wallet
            .clone()
            .ok_or_else(|| anyhow::anyhow!("master wallet not configured"))?;
        tracing::info!("opening master wallet");
        let master_wallet = open_wallet(&master_config, rpc_client, vault, true)
            .await
            .context("open master wallet")?;
        tracing::info!("master wallet opened: address={}", master_wallet.address().to_string());
        Ok(master_wallet)
    }

    async fn load_pools(
        app_config: &AppConfig,
        rpc_client: Arc<ClientJsonRpc>,
        wallets: &HashMap<String, Arc<dyn TonWallet>>,
    ) -> anyhow::Result<Arc<HashMap<String, Arc<dyn NominatorWrapper>>>> {
        let mut map = HashMap::new();
        for (node_name, binding) in app_config.bindings.iter() {
            if let Some(pool_name) = &binding.pool {
                let cfg = app_config
                    .pools
                    .get(pool_name)
                    .context(format!("pool '{}' not found for node '{}'", pool_name, node_name))?;
                let validator_address = wallets
                    .get(node_name)
                    .context(format!("validator wallet not found: {}", node_name))?
                    .address();
                let pool = open_nominator_pool(cfg, rpc_client.clone(), &validator_address)
                    .map_err(|e| {
                        anyhow::anyhow!("node [{}] open nominator pool error: {:#}", node_name, e)
                    })?;
                tracing::info!(
                    "[{}] opened nominator pool: address={}",
                    node_name,
                    pool.address().to_string()
                );
                map.insert(node_name.to_owned(), pool);
            }
        }
        Ok(Arc::new(map))
    }

    async fn load_wallets(
        app_config: &AppConfig,
        rpc_client: Arc<ClientJsonRpc>,
        vault: Option<Arc<SecretVault>>,
    ) -> anyhow::Result<Arc<HashMap<String, Arc<dyn TonWallet>>>> {
        let mut map = HashMap::new();
        for (node_name, binding) in app_config.bindings.iter() {
            let cfg = app_config.wallets.get(&binding.wallet).context(format!(
                "wallet '{}' not found for node '{}'",
                binding.wallet, node_name
            ))?;
            let wallet =
                open_wallet(cfg, rpc_client.clone(), vault.clone(), false).await.map_err(|e| {
                    anyhow::anyhow!("node [{}] open validator wallet error: {:#}", node_name, e)
                })?;
            tracing::info!(
                "[{}] opened wallet: address={}",
                node_name,
                wallet.address().to_string()
            );
            map.insert(node_name.to_owned(), wallet);
        }
        Ok(Arc::new(map))
    }
}

impl RuntimeConfig for RuntimeConfigStore {
    /// Returns a cheap Arc reference to the current config snapshot.
    /// The returned Arc won't reflect future updates.
    fn get(&self) -> Arc<AppConfig> {
        let state = self.state.read().expect("Runtime state poisoned (read)");
        Arc::clone(&state.config)
    }

    fn master_wallet(&self) -> Arc<dyn TonWallet> {
        let state = self.state.read().expect("Runtime state poisoned (read)");
        Arc::clone(&state.master_wallet)
    }

    fn pools(&self) -> Arc<HashMap<String, Arc<dyn NominatorWrapper>>> {
        let state = self.state.read().expect("Runtime state poisoned (read)");
        Arc::clone(&state.pools)
    }

    fn wallets(&self) -> Arc<HashMap<String, Arc<dyn TonWallet>>> {
        let state = self.state.read().expect("Runtime state poisoned (read)");
        Arc::clone(&state.wallets)
    }

    fn rpc_client(&self) -> Arc<ClientJsonRpc> {
        let state = self.state.read().expect("Runtime state poisoned (read)");
        Arc::clone(&state.rpc_client)
    }

    fn vault(&self) -> Option<Arc<SecretVault>> {
        let state = self.state.read().expect("Runtime state poisoned (read)");
        state.vault.clone()
    }

    fn update_config(&self, f: Box<dyn FnOnce(&mut AppConfig) + Send>) -> anyhow::Result<()> {
        self.update_with(f)
    }

    fn save_to_file(&self) {
        RuntimeConfigStore::save_to_file(self)
    }
}

async fn open_wallet(
    wallet_config: &WalletConfig,
    rpc_client: Arc<ClientJsonRpc>,
    vault: Option<Arc<SecretVault>>,
    generate_secret: bool,
) -> anyhow::Result<Arc<dyn TonWallet>> {
    let master_secret = match wallet_config.key.read_secret(vault.clone()).await {
        Ok(secret) => secret,
        Err(e) if !generate_secret => {
            anyhow::bail!("read wallet secret from config error: {:#}", e)
        }
        Err(e) => {
            tracing::warn!("read wallet secret from config error: {:#}", e);
            if let Some(vault) = vault {
                let spec = SecretSpec::new(Algorithm::Ed25519);
                match &wallet_config.key {
                    KeyConfig::VaultKey { name } => {
                        tracing::info!("generate wallet secret in vault: name={}", name);
                        vault
                            .generate_secret(&spec, &SecretId::new(name))
                            .await
                            .context("generate wallet secret")?
                    }
                    _ => anyhow::bail!("invalid master wallet key config: {:#}", e),
                }
            } else {
                anyhow::bail!("vault is required but not configured: {:#}", e);
            }
        }
    };

    let wallet_signer = VaultSigner::new(master_secret).await?;
    let wallet = WalletContract::new(
        Box::new(wallet_signer),
        wallet_config.version,
        wallet_config.subwallet_id,
        wallet_config.workchain,
        contract_provider!(rpc_client),
    )
    .await?;

    Ok(Arc::new(wallet))
}

fn open_nominator_pool(
    config: &PoolConfig,
    rpc_client: Arc<ClientJsonRpc>,
    validator_addr: &MsgAddressInt,
) -> anyhow::Result<Arc<dyn NominatorWrapper>> {
    match config {
        PoolConfig::SNP { address, owner } => {
            let pool = match (address, owner) {
                (Some(address), Some(owner)) => {
                    let addr = MsgAddressInt::from_str(address)
                        .context(format!("invalid pool address: {}", address))?;
                    let owner_addr = MsgAddressInt::from_str(owner)
                        .context(format!("invalid pool owner address: {}", owner))?;
                    let pool = NominatorWrapperImpl::from_init_data(
                        contract_provider!(rpc_client.clone()),
                        &owner_addr,
                        validator_addr,
                        -1,
                    )?;
                    let calculated_addr =
                        NominatorWrapperImpl::calculate_address(-1, &owner_addr, validator_addr)?;
                    if calculated_addr != addr {
                        anyhow::bail!(
                            "calculated pool address does not match the defined address: defined={}, calculated={}",
                            addr,
                            calculated_addr
                        );
                    }
                    pool
                }
                (None, Some(owner)) => {
                    let owner_addr = MsgAddressInt::from_str(owner)
                        .context(format!("invalid pool owner address: {}", owner))?;
                    NominatorWrapperImpl::from_init_data(
                        contract_provider!(rpc_client.clone()),
                        &owner_addr,
                        validator_addr,
                        -1,
                    )?
                }
                (Some(address), None) => {
                    let addr = MsgAddressInt::from_str(address)
                        .context(format!("invalid pool address: {}", address))?;
                    NominatorWrapperImpl::new(contract_provider!(rpc_client.clone()), addr)
                }
                (None, None) => {
                    anyhow::bail!("pool has neither address nor owner configured");
                }
            };
            Ok(Arc::new(pool))
        }
        _ => anyhow::bail!("unsupported pool kind"),
    }
}
