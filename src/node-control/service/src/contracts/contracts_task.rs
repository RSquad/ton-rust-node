/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::runtime_config::RuntimeConfig;
use anyhow::Context;
use common::{
    app_config::{AppConfig, ContractsAutomationConfig},
    task_cancellation::CancellationCtx,
};
use contracts::{
    NominatorWrapper, PoolKind, TonWallet, contract_provider,
    nominator::ton_core_pool as tc_messages,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::{self, MissedTickBehavior};
use ton_block::{Cell, MsgAddressInt, write_boc};
use ton_http_api_client::v2::{client_json_rpc::ClientJsonRpc, data_models::AccountState};

/// Gas cost for sending a simple message from a wallet (deploy, top-up).
const WALLET_GAS: u64 = 100_000_000; // 0.1 TON
/// Gas for masterchain pool operations (update_validator_set, etc.)
/// Masterchain gas prices are ~25x basechain; 0.1 TON is not enough
/// for load_data + get_current_validator_set + cell_hash + save_data.
const POOL_OP_GAS: u64 = 500_000_000; // 0.5 TON

pub(crate) async fn run(
    cancellation_ctx: CancellationCtx,
    _app_config: Arc<AppConfig>,
    runtime_cfg: Arc<dyn RuntimeConfig>,
) -> anyhow::Result<()> {
    let master_wallet = runtime_cfg.master_wallet();
    let pools = runtime_cfg.pools();
    let wallets = runtime_cfg.wallets();
    let rpc_client = runtime_cfg.rpc_client();
    let monitor = ContractsMonitor { master_wallet, pools, wallets, rpc_client, runtime_cfg };
    monitor.run_loop(cancellation_ctx).await
}

struct ContractsMonitor {
    master_wallet: Arc<dyn TonWallet>,
    pools: Arc<HashMap<String, Arc<dyn NominatorWrapper>>>,
    wallets: Arc<HashMap<String, Arc<dyn TonWallet>>>,
    rpc_client: Arc<ClientJsonRpc>,
    runtime_cfg: Arc<dyn RuntimeConfig>,
}

impl ContractsMonitor {
    fn automation(&self) -> ContractsAutomationConfig {
        self.runtime_cfg.get().automation.clone()
    }

    async fn run_loop(&self, cancellation_ctx: CancellationCtx) -> anyhow::Result<()> {
        let mut cancel = cancellation_ctx.subscribe();
        let mut tick_secs = self.automation().tick_interval_sec.max(1);
        let mut ticker = time::interval(Duration::from_secs(tick_secs));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = self.run().await {
                        tracing::error!(target: "contracts", "run error: {:#}", e);
                    }

                    let next_secs = self.automation().tick_interval_sec.max(1);
                    if next_secs != tick_secs {
                        tick_secs = next_secs;
                        let period = Duration::from_secs(tick_secs);
                        // `time::interval` fires its first tick immediately; after a period change,
                        // schedule the next tick one full period from now to avoid two runs in a row.
                        let start = time::Instant::now() + period;
                        ticker = time::interval_at(start, period);
                        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
                    }
                }
                _ = cancel.changed() => {
                    tracing::info!(target: "contracts", "cancel received");
                    return Ok(());
                }
            }
        }
    }

    async fn run(&self) -> anyhow::Result<()> {
        let auto = self.automation();
        // Master is only needed for deploy/top-up paths. TonCore `update_validator_set` uses the
        // per-node validator wallet, so observe-only mode can run without a funded master.
        if !auto.auto_deploy && !auto.auto_topup {
            self.ensure_pool_validator_sets_updated().await?;
            return Ok(());
        }
        if !self.ensure_master_deployed(&auto).await? {
            return Ok(());
        }
        let provider = contract_provider!(self.rpc_client.clone());
        let mut seqno = provider
            .get_method(self.master_wallet.address().await?.to_string(), "seqno", vec![])
            .await?
            .i64(0)?;
        if !self.ensure_wallets_deployed(&auto, &mut seqno).await? {
            return Ok(());
        }
        if !self.ensure_pools_deployed(&auto, &mut seqno).await? {
            return Ok(());
        }
        if !self.ensure_wallet_balances(&auto, &mut seqno).await? {
            return Ok(());
        }
        if !self.ensure_pool_validator_sets_updated().await? {
            return Ok(());
        }
        tracing::info!(target: "contracts", "all contracts are ready");
        Ok(())
    }

    async fn account_info(&self, address: &MsgAddressInt) -> anyhow::Result<(AccountState, u64)> {
        let info = self
            .rpc_client
            .get_address_information(address)
            .await
            .context("get_address_information")?;
        Ok((info.state, info.balance))
    }

    async fn broadcast(&self, cell: &Cell) -> anyhow::Result<()> {
        let boc = write_boc(cell).context("write_boc")?;
        self.rpc_client.send_boc(&boc).await
    }

    fn insufficient_master_balance_error(balance: u64, required: u64) -> anyhow::Error {
        anyhow::anyhow!(
            "master wallet balance is low: balance={:.4} TON need={:.4} TON",
            balance as f64 / 1e9,
            required as f64 / 1e9,
        )
    }

    /// Step 1: Deploy master wallet if uninitialized.
    /// Returns `true` when master is active and ready for subsequent steps.
    /// Returns an error if master is frozen or has insufficient balance.
    async fn ensure_master_deployed(
        &self,
        auto: &ContractsAutomationConfig,
    ) -> anyhow::Result<bool> {
        let addr = self.master_wallet.address().await?;
        let (state, balance) = self.account_info(&addr).await.context("get master wallet state")?;

        match state {
            AccountState::Active => return Ok(true),
            AccountState::Frozen => {
                anyhow::bail!("master wallet is frozen: address={}", addr);
            }
            AccountState::Uninitialized => {}
        }

        let min_balance = auto.wallet.deploy;
        if balance < min_balance {
            return Err(Self::insufficient_master_balance_error(balance, min_balance));
        }

        tracing::info!(
            target: "contracts",
            "deploy master wallet: address={}, balance={} TON",
            addr,
            balance as f64 / 1e9,
        );
        let msg = self
            .master_wallet
            .deploy_message(0, Cell::default())
            .await
            .context("build master wallet deploy message")?;
        self.broadcast(&msg).await.context("send master wallet deploy")?;
        Ok(false)
    }

    /// Step 2: Deploy uninitialized wallets through the master wallet.
    ///
    /// The master wallet sends an internal message carrying the wallet's
    /// state_init and configured `wallet.deploy` (nanotons), deploying and funding it in one go.
    ///
    /// Returns `false` if master balance is insufficient (caller should sleep).
    async fn ensure_wallets_deployed(
        &self,
        auto: &ContractsAutomationConfig,
        seqno: &mut i64,
    ) -> anyhow::Result<bool> {
        if !auto.auto_deploy {
            return Ok(true);
        }
        let mut all_deployed = true;
        let mut processed_wallets = HashSet::new();

        for (node_id, wallet) in self.wallets.iter() {
            let wallet_addr = wallet.address().await?;
            let is_new = processed_wallets.insert(wallet_addr.clone());
            if !is_new {
                tracing::debug!(
                    target: "contracts",
                    "[{}] skipping wallet deploy: address {} already processed",
                    node_id,
                    wallet_addr
                );
                continue;
            }

            match self.deploy_wallet(auto, &node_id, wallet.clone(), *seqno).await {
                Ok(true) => (),
                Ok(false) => {
                    all_deployed = false;
                    *seqno += 1;
                }
                Err(e) => {
                    all_deployed = false;
                    tracing::error!(target: "contracts", "[{}] deploy wallet error: {:#}", node_id, e);
                }
            };
        }
        Ok(all_deployed)
    }

    async fn deploy_wallet(
        &self,
        auto: &ContractsAutomationConfig,
        node_id: &str,
        wallet: Arc<dyn TonWallet>,
        seqno: i64,
    ) -> anyhow::Result<bool> {
        let addr = wallet.address().await?;
        let (state, balance) = self.account_info(&addr).await.context("get wallet state")?;

        match state {
            AccountState::Active => return Ok(true),
            AccountState::Frozen => {
                anyhow::bail!("wallet is frozen: address={}", addr);
            }
            AccountState::Uninitialized => {}
        };

        tracing::info!(
            target: "contracts",
            "[{}] wallet is uninitialized: address={} balance={:.4} TON",
            node_id, addr, balance as f64 / 1e9,
        );

        let deploy_amount = auto.wallet.deploy;
        let master_balance = self.master_wallet.balance().await.context("master wallet balance")?;
        if master_balance < deploy_amount + WALLET_GAS {
            return Err(Self::insufficient_master_balance_error(
                master_balance,
                deploy_amount + WALLET_GAS,
            ));
        }

        let si = wallet.state_init().await?;

        tracing::info!(
            target: "contracts",
            "[{}] deploy wallet: amount={:.4} TON",
            node_id, deploy_amount as f64 / 1e9,
        );
        let msg = self
            .master_wallet
            .build_message(
                addr,
                deploy_amount,
                Cell::default(),
                false,
                Some(u32::try_from(seqno)?),
                None,
                Some(si),
            )
            .await?;
        self.broadcast(&msg).await?;
        Ok(false)
    }

    /// Step 3: Deploy uninitialized nominator pools through the master wallet.
    ///
    /// If a pool has a `state_init`, it is deployed in a single message (funds + code).
    /// Otherwise only funds are sent and a warning is logged.
    ///
    /// Returns `false` if master balance is insufficient (caller should sleep).
    async fn ensure_pools_deployed(
        &self,
        auto: &ContractsAutomationConfig,
        seqno: &mut i64,
    ) -> anyhow::Result<bool> {
        if !auto.auto_deploy {
            return Ok(true);
        }
        let mut all_deployed = true;
        for (node_id, pool_binding) in self.pools.iter() {
            for pool in pool_binding.inner_pools() {
                match self.deploy_pool(auto, node_id, pool, *seqno).await {
                    Ok(true) => (),
                    Ok(false) => {
                        all_deployed = false;
                        *seqno += 1;
                    }
                    Err(e) => {
                        all_deployed = false;
                        tracing::error!(target: "contracts", "[{}] deploy pool error: {:#}", node_id, e);
                    }
                };
            }
        }
        Ok(all_deployed)
    }

    async fn deploy_pool(
        &self,
        auto: &ContractsAutomationConfig,
        node_id: &str,
        pool: Arc<dyn NominatorWrapper>,
        seqno: i64,
    ) -> anyhow::Result<bool> {
        let pool_addr = pool.address().await?;
        let (state, _) = self.account_info(&pool_addr).await.context("get pool state")?;

        match state {
            AccountState::Active => return Ok(true),
            AccountState::Frozen => {
                anyhow::bail!("pool is frozen: address={}", pool_addr);
            }
            AccountState::Uninitialized => {}
        };

        tracing::info!(
            target: "contracts",
            "[{}] pool is uninitialized: address={}",
            node_id, pool_addr,
        );

        let deploy_amount = match pool.pool_kind() {
            PoolKind::SNP => auto.pool.snp,
            PoolKind::TONCore => auto.pool.ton_core,
        };

        let master_balance =
            self.master_wallet.balance().await.context("get master wallet balance")?;
        if master_balance < deploy_amount + WALLET_GAS {
            return Err(Self::insufficient_master_balance_error(
                master_balance,
                deploy_amount + WALLET_GAS,
            ));
        }

        if pool.state_init().is_none() {
            tracing::warn!(target: "contracts", "[{}] pool has no state_init configured, sending funds only", node_id);
        }

        tracing::info!(target: "contracts", "[{}] deploy pool: amount={:.4} TON",
                node_id, deploy_amount as f64 / 1e9);
        let msg = self
            .master_wallet
            .build_message(
                pool_addr,
                deploy_amount,
                Cell::default(),
                false,
                Some(u32::try_from(seqno)?),
                None,
                pool.state_init(),
            )
            .await?;

        self.broadcast(&msg).await?;
        Ok(false)
    }

    /// Step 4: Top up active wallets whose balance is below the minimum threshold.
    ///
    /// Step 5 (`ensure_pool_validator_sets_updated`) depends on the pools being
    /// deployed and wallets funded, so this step runs first.
    async fn ensure_wallet_balances(
        &self,
        auto: &ContractsAutomationConfig,
        seqno: &mut i64,
    ) -> anyhow::Result<bool> {
        if !auto.auto_topup {
            return Ok(true);
        }
        let threshold = auto.wallet.threshold;
        let topup_amount = auto.wallet.topup;
        let mut all_topped_up = true;
        let mut processed_wallets = HashSet::new();
        for (node_id, wallet) in self.wallets.iter() {
            let addr = wallet.address().await?;
            let is_new = processed_wallets.insert(addr.clone());
            if !is_new {
                tracing::debug!(
                    target: "contracts",
                    "[{}] skipping wallet top-up: address {} already processed",
                    node_id,
                    addr
                );
                continue;
            }

            let (state, balance) = match self.account_info(&addr).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(target: "contracts", "[{}] get wallet info error: {:#}", node_id, e);
                    continue;
                }
            };

            if state != AccountState::Active {
                continue;
            }

            if balance >= threshold {
                continue;
            }

            all_topped_up = false;
            tracing::info!(
                target: "contracts",
                "[{}] top-up wallet: address={} current_balance={:.4} TON topup_amount={:.4} TON",
                node_id, addr, balance as f64 / 1e9, topup_amount as f64 / 1e9,
            );

            let master_balance =
                self.master_wallet.balance().await.context("get master wallet balance")?;
            if master_balance < topup_amount + WALLET_GAS {
                return Err(Self::insufficient_master_balance_error(
                    master_balance,
                    topup_amount + WALLET_GAS,
                ));
            }

            match self
                .master_wallet
                .build_message(
                    addr,
                    topup_amount,
                    Cell::default(),
                    false,
                    Some(u32::try_from(*seqno)?),
                    None,
                    None,
                )
                .await
            {
                Ok(msg) => {
                    if let Err(e) = self.broadcast(&msg).await {
                        tracing::error!(target: "contracts", "[{}] send top-up error: {:#}", node_id, e);
                    }
                    *seqno += 1;
                }
                Err(e) => {
                    tracing::error!(target: "contracts", "[{}] build top-up message error: {:#}", node_id, e);
                }
            }
        }
        Ok(all_topped_up)
    }

    /// Step 5: Send `update_validator_set` (opcode 6) to TonCore pools
    /// that are in staking state (state == 2) but haven't detected enough validator
    /// set changes for recovery.
    ///
    /// The TonCore pool contract tracks the on-chain validator set hash
    /// (config param 34) and increments an internal counter each time it changes.
    /// Recovery is only allowed once `validator_set_changes_count >= 2`.
    /// Unlike the SNP contract, the TonCore pool does not update this counter
    /// automatically — opcode 6 must be sent explicitly (by anyone).
    ///
    /// The message is sent from the **validator wallet** for the pool's node (`self.wallets[node_id]`),
    /// not the master wallet — by staking time that wallet is deployed and typically topped up.
    async fn ensure_pool_validator_sets_updated(&self) -> anyhow::Result<bool> {
        let provider = contract_provider!(self.rpc_client.clone());
        let mut all_updated = true;
        tracing::info!(
            target: "contracts",
            "ensure_pool_validator_sets_updated: checking {} nodes",
            self.pools.len()
        );
        for (node_id, pool_binding) in
            self.pools.iter().filter(|(_, b)| b.pool_kind() == PoolKind::TONCore)
        {
            let validator_wallet = match self.wallets.get(node_id.as_str()) {
                Some(w) => (*w).clone(),
                None => {
                    tracing::warn!(
                        target: "contracts",
                        "[{}] ensure_pool_validator_sets_updated: no validator wallet in config (skip TonCore pools)",
                        node_id
                    );
                    all_updated = false;
                    continue;
                }
            };

            let wallet_addr = validator_wallet.address().await?;
            let mut seqno: Option<i64> = None;

            for pool in pool_binding.inner_pools() {
                let pool_addr = pool.address().await?;
                let pool_data = match pool.get_pool_data().await {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(
                            target: "contracts",
                            "[{}] get_pool_data error (skipping update_validator_set): pool={} {:#}",
                            node_id, pool_addr, e
                        );
                        continue;
                    }
                };

                tracing::info!(
                    target: "contracts",
                    "[{}] pool={} state={} vsc_count={}",
                    node_id, pool_addr, pool_data.state, pool_data.validator_set_changes_count
                );

                if pool_data.state != 2 || pool_data.validator_set_changes_count >= 2 {
                    continue;
                }

                let current_seqno = match seqno {
                    Some(s) => s,
                    None => provider
                        .get_method(wallet_addr.to_string(), "seqno", vec![])
                        .await?
                        .i64(0)?,
                };

                tracing::info!(
                    target: "contracts",
                    "[{}] update_validator_set: pool={}, state={}, vsc_count={}, from_wallet={}",
                    node_id,
                    pool_addr,
                    pool_data.state,
                    pool_data.validator_set_changes_count,
                    wallet_addr,
                );

                let body = tc_messages::update_validator_set(0)?;
                let msg = validator_wallet
                    .build_message(
                        pool_addr,
                        POOL_OP_GAS,
                        body,
                        true,
                        Some(u32::try_from(current_seqno)?),
                        None,
                        None,
                    )
                    .await?;
                self.broadcast(&msg).await?;
                seqno = Some(current_seqno + 1);
                all_updated = false;
            }
        }
        Ok(all_updated)
    }
}

#[cfg(test)]
mod tests {
    use super::ContractsMonitor;
    use crate::runtime_config::RuntimeConfig;
    use axum::{Json, Router, extract::State, routing::post};
    use common::app_config::{AppConfig, ContractsAutomationConfig, HttpConfig, TonHttpApiConfig};
    use contracts::{NominatorWrapper, SmartContract, TonWallet};
    use secrets_vault::vault::SecretVault;
    use std::{
        collections::HashMap,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use ton_block::{Cell, MsgAddressInt, StateInit};
    use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

    /// Minimal [`RuntimeConfig`] for unit tests that only call `get()`.
    struct CfgRuntime(Arc<AppConfig>);

    impl RuntimeConfig for CfgRuntime {
        fn get(&self) -> Arc<AppConfig> {
            self.0.clone()
        }

        fn master_wallet(&self) -> Arc<dyn TonWallet> {
            unimplemented!("CfgRuntime: use ContractsMonitor.master_wallet")
        }

        fn pools(&self) -> Arc<HashMap<String, Arc<dyn NominatorWrapper>>> {
            unimplemented!("CfgRuntime: use ContractsMonitor.pools")
        }

        fn wallets(&self) -> Arc<HashMap<String, Arc<dyn TonWallet>>> {
            unimplemented!("CfgRuntime: use ContractsMonitor.wallets")
        }

        fn rpc_client(&self) -> Arc<ClientJsonRpc> {
            unimplemented!("CfgRuntime: use ContractsMonitor.rpc_client")
        }

        fn vault(&self) -> Option<Arc<SecretVault>> {
            None
        }

        fn update_and_save(
            &self,
            _f: Box<dyn FnOnce(&mut AppConfig) + Send>,
        ) -> anyhow::Result<()> {
            unimplemented!("CfgRuntime: read-only")
        }
    }

    fn test_app_config() -> Arc<AppConfig> {
        Arc::new(AppConfig {
            nodes: HashMap::new(),
            wallets: HashMap::new(),
            pools: HashMap::new(),
            bindings: HashMap::new(),
            ton_http_api: TonHttpApiConfig::default(),
            elections: None,
            voting: None,
            http: HttpConfig { auth: None, ..Default::default() },
            master_wallet: None,
            tick_interval: 30,
            automation: Default::default(),
            log: None,
        })
    }

    #[derive(Clone)]
    struct MockRpcState {
        account_state: &'static str,
        account_balance: u64,
        send_boc_calls: Arc<AtomicUsize>,
    }

    struct MockRpcServer {
        url: String,
        send_boc_calls: Arc<AtomicUsize>,
        shutdown_tx: tokio::sync::oneshot::Sender<()>,
        join: tokio::task::JoinHandle<()>,
    }

    impl MockRpcServer {
        async fn start(account_state: &'static str, account_balance: u64) -> Self {
            let send_boc_calls = Arc::new(AtomicUsize::new(0));
            let state = MockRpcState {
                account_state,
                account_balance,
                send_boc_calls: send_boc_calls.clone(),
            };
            let app = Router::new().route("/jsonRPC", post(mock_jsonrpc)).with_state(state);

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
            let join = tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .unwrap();
            });

            Self { url: format!("http://{addr}/"), send_boc_calls, shutdown_tx, join }
        }

        async fn shutdown(self) {
            let _ = self.shutdown_tx.send(());
            let _ = self.join.await;
        }
    }

    async fn mock_jsonrpc(
        State(state): State<MockRpcState>,
        Json(request): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        let method = request.get("method").and_then(|m| m.as_str()).unwrap_or_default();
        let id = request.get("id").cloned().unwrap_or_else(|| serde_json::json!("1"));

        let response = match method {
            "getAddressInformation" => serde_json::json!({
                "ok": true,
                "result": {
                    "@type": "raw.fullAccountState",
                    "balance": state.account_balance,
                    "extra_currencies": [],
                    "code": "",
                    "data": "",
                    "last_transaction_id": {
                        "@type": "internal.transactionId",
                        "lt": "0",
                        "hash": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
                    },
                    "block_id": {
                        "@type": "ton.blockIdExt",
                        "workchain": -1,
                        "shard": "-9223372036854775808",
                        "seqno": 1,
                        "root_hash": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                        "file_hash": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
                    },
                    "frozen_hash": "",
                    "sync_utime": 0,
                    "state": state.account_state
                },
                "jsonrpc": "2.0",
                "id": id
            }),
            "sendBoc" => {
                state.send_boc_calls.fetch_add(1, Ordering::Relaxed);
                serde_json::json!({
                    "ok": true,
                    "result": { "@type": "ok" },
                    "jsonrpc": "2.0",
                    "id": id
                })
            }
            _ => serde_json::json!({
                "ok": false,
                "error": format!("unsupported method {method}"),
                "code": 400,
                "jsonrpc": "2.0",
                "id": id
            }),
        };

        Json(response)
    }

    #[derive(Clone)]
    struct DummyWallet {
        addr: MsgAddressInt,
        state_init: Option<StateInit>,
    }

    #[async_trait::async_trait]
    impl SmartContract for DummyWallet {
        async fn balance(&self) -> anyhow::Result<u64> {
            Ok(u64::MAX)
        }

        async fn address(&self) -> anyhow::Result<MsgAddressInt> {
            Ok(self.addr.clone())
        }
    }

    #[async_trait::async_trait]
    impl TonWallet for DummyWallet {
        async fn message(
            &self,
            _dest: MsgAddressInt,
            _value: u64,
            _payload: Cell,
        ) -> anyhow::Result<Cell> {
            Ok(Cell::default())
        }

        async fn deploy_message(&self, _value: u64, _payload: Cell) -> anyhow::Result<Cell> {
            Ok(Cell::default())
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
            Ok(Cell::default())
        }

        async fn state_init(&self) -> anyhow::Result<StateInit> {
            self.state_init.clone().ok_or_else(|| anyhow::anyhow!("state_init is not set"))
        }
    }

    fn addr(byte: u8) -> MsgAddressInt {
        MsgAddressInt::with_standart(None, -1, [byte; 32].into()).unwrap()
    }

    fn build_monitor(
        rpc_url: String,
        master_wallet: Arc<dyn TonWallet>,
        wallets: Arc<HashMap<String, Arc<dyn TonWallet>>>,
    ) -> ContractsMonitor {
        let rpc_client = Arc::new(
            ClientJsonRpc::connect_many(
                vec![(rpc_url, None)],
                None,
                common::app_config::EndpointTimeouts::default(),
                common::app_config::FreshnessConfig::disabled(),
            )
            .unwrap(),
        );
        ContractsMonitor {
            master_wallet,
            pools: Arc::<HashMap<String, Arc<dyn NominatorWrapper>>>::default(),
            wallets,
            rpc_client,
            runtime_cfg: Arc::new(CfgRuntime(test_app_config())) as Arc<dyn RuntimeConfig>,
        }
    }

    #[tokio::test]
    async fn ensure_wallets_deployed_sends_once_for_shared_wallet() {
        let server = MockRpcServer::start("uninitialized", 0).await;

        let shared_wallet: Arc<dyn TonWallet> =
            Arc::new(DummyWallet { addr: addr(1), state_init: Some(StateInit::default()) });
        let wallets: Arc<HashMap<String, Arc<dyn TonWallet>>> = Arc::new(HashMap::from([
            ("node-a".to_string(), shared_wallet.clone()),
            ("node-b".to_string(), shared_wallet),
        ]));
        let master_wallet: Arc<dyn TonWallet> =
            Arc::new(DummyWallet { addr: addr(9), state_init: Some(StateInit::default()) });

        let monitor = build_monitor(server.url.clone(), master_wallet, wallets);
        let mut seqno = 1;
        let auto = ContractsAutomationConfig::default();
        let all_deployed = monitor.ensure_wallets_deployed(&auto, &mut seqno).await.unwrap();

        assert!(!all_deployed);
        assert_eq!(seqno, 2);
        assert_eq!(server.send_boc_calls.load(Ordering::Relaxed), 1);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn ensure_wallet_balances_sends_once_for_shared_wallet() {
        let server = MockRpcServer::start("active", 0).await;

        let shared_wallet: Arc<dyn TonWallet> =
            Arc::new(DummyWallet { addr: addr(2), state_init: Some(StateInit::default()) });
        let wallets: Arc<HashMap<String, Arc<dyn TonWallet>>> = Arc::new(HashMap::from([
            ("node-a".to_string(), shared_wallet.clone()),
            ("node-b".to_string(), shared_wallet),
        ]));
        let master_wallet: Arc<dyn TonWallet> =
            Arc::new(DummyWallet { addr: addr(9), state_init: Some(StateInit::default()) });

        let monitor = build_monitor(server.url.clone(), master_wallet, wallets);
        let mut seqno = 10;
        let auto = ContractsAutomationConfig::default();
        let all_topped_up = monitor.ensure_wallet_balances(&auto, &mut seqno).await.unwrap();

        assert!(!all_topped_up);
        assert_eq!(seqno, 11);
        assert_eq!(server.send_boc_calls.load(Ordering::Relaxed), 1);

        server.shutdown().await;
    }
}
