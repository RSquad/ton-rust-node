/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{
    providers::{DefaultElectionsProvider, ElectionsProvider},
    runner::{ElectionRunner, PersistStaticAdnls},
};
use crate::runtime_config::RuntimeConfig;
use anyhow::Context;
use common::{
    app_config::{AppConfig, BindingStatus, ElectionsConfig},
    snapshot::SnapshotStore,
    task_cancellation::CancellationCtx,
};
use contracts::{ElectorWrapperImpl, NominatorWrapper, TonWallet, contract_provider};
use secrets_vault::vault::SecretVault;
use std::{collections::HashMap, sync::Arc, time::Duration};
use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

/// Callback invoked after each tick with updated binding statuses.
pub type BindingStatusCallback = Arc<dyn Fn(HashMap<String, BindingStatus>) + Send + Sync>;

pub async fn run(
    cancellation_ctx: CancellationCtx,
    app_config: Arc<AppConfig>,
    rpc_client: Arc<ClientJsonRpc>,
    wallets: Arc<HashMap<String, Arc<dyn TonWallet>>>,
    pools: Arc<HashMap<String, Arc<dyn NominatorWrapper>>>,
    store: Arc<SnapshotStore>,
    vault: Option<Arc<SecretVault>>,
    on_status_change: Option<BindingStatusCallback>,
    runtime_cfg: Arc<dyn RuntimeConfig>,
) -> anyhow::Result<()> {
    let Some(config) = app_config.elections.as_ref() else {
        anyhow::bail!("elections config is empty");
    };

    let adnl_configs = app_config
        .nodes
        .iter()
        .map(|(node_name, cfg)| (node_name.clone(), cfg.clone()))
        .collect::<HashMap<_, _>>();

    let mut set = tokio::task::JoinSet::new();
    let mut sorted_nodes: Vec<_> = adnl_configs.into_iter().collect();
    sorted_nodes.sort_by(|(a, _), (b, _)| a.cmp(b));

    for (node_id, config) in sorted_nodes.into_iter() {
        let vault = vault.clone();
        set.spawn(async move { (node_id, config.to_node_adnl_config(vault).await) });
    }

    let providers: HashMap<String, Box<dyn ElectionsProvider>> = set
        .join_all()
        .await
        .into_iter()
        .filter_map(|(node_id, config)| match config {
            Ok(config) => {
                let provider: Box<dyn ElectionsProvider> =
                    Box::new(DefaultElectionsProvider::new(config));
                tracing::info!("node [{}] elections provider created", node_id);
                Some((node_id, provider))
            }
            Err(e) => {
                tracing::error!("node [{}] has wrong ADNL config: {}", node_id, e);
                None
            }
        })
        .collect();

    if providers.len() != app_config.nodes.len() {
        anyhow::bail!("cannot proceed: some nodes have invalid configs");
    }

    let elector = Arc::new(ElectorWrapperImpl::new(contract_provider!(rpc_client)));

    let persist_static_adnls: PersistStaticAdnls = {
        let runtime_cfg = runtime_cfg.clone();
        Arc::new(move |generated: HashMap<String, String>| {
            runtime_cfg.update_and_save(Box::new(move |cfg| {
                let elections = cfg.elections.get_or_insert_with(ElectionsConfig::default);
                for (node_id, b64) in generated {
                    elections.static_adnls.insert(node_id, b64);
                }
            }))
        })
    };

    let mut runner = ElectionRunner::new(
        config,
        &app_config.bindings,
        elector,
        providers,
        wallets,
        pools,
        Some(persist_static_adnls),
    );
    runner
        .run_loop(
            Duration::from_secs(config.tick_interval),
            cancellation_ctx,
            store,
            on_status_change,
        )
        .await
        .context("elections loop error")
}
