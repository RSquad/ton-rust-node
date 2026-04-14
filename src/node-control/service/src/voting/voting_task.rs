/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    runtime_config::RuntimeConfig,
    voting::{
        VotingProviderImpl,
        voting_provider::{ValidatorEntry, VotingProvider},
    },
};
use anyhow::Context;
use common::{app_config::AppConfig, task_cancellation::CancellationCtx};
use contracts::{
    ConfigContractImpl, ConfigContractWrapper, ConfigProposal, TonWallet, config_contract,
    contract_provider,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use ton_block::{ValidatorSet, write_boc};

const SEND_VOTE_AMOUNT: u64 = 1_000_000_000;

struct Node {
    api: Box<dyn VotingProvider>,
    wallet: Arc<dyn TonWallet>,
}

struct VotingRunner {
    nodes: HashMap<String, Node>,
    config_contract: Arc<dyn ConfigContractWrapper>,
    tracked_proposals: Vec<[u8; 32]>,
}

pub(crate) async fn run(
    cancellation_ctx: CancellationCtx,
    app_config: Arc<AppConfig>,
    runtime_cfg: Arc<dyn RuntimeConfig>,
) -> anyhow::Result<()> {
    let Some(config) = app_config.voting.as_ref() else {
        anyhow::bail!("voting config is empty");
    };
    let adnl_configs = app_config.nodes.clone();
    let mut set = tokio::task::JoinSet::new();
    let mut nodes: Vec<_> = adnl_configs.into_iter().collect();
    nodes.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (node_id, config) in nodes.into_iter() {
        let vault = runtime_cfg.vault();
        set.spawn(async move { (node_id, config.to_node_adnl_config(vault).await) });
    }

    let providers: HashMap<String, Box<dyn VotingProvider>> = set
        .join_all()
        .await
        .into_iter()
        .filter_map(|(node_id, config)| match config {
            Ok(config) => {
                let provider: Box<dyn VotingProvider> = Box::new(VotingProviderImpl::new(config));
                tracing::info!(target: "voting", "node [{}] voting provider created", node_id);
                Some((node_id, provider))
            }
            Err(e) => {
                tracing::error!(target: "voting", "node [{}] has wrong ADNL config: {}", node_id, e);
                None
            }
        })
        .collect();

    if providers.len() != app_config.nodes.len() {
        anyhow::bail!("cannot proceed: some nodes have invalid configs");
    }

    let config_contract =
        Arc::new(ConfigContractImpl::new(contract_provider!(runtime_cfg.rpc_client())));

    let wallets = runtime_cfg.wallets();
    let nodes = providers
        .into_iter()
        .filter_map(|(node_id, provider)| match wallets.get(&node_id) {
            Some(wallet) => Some((node_id, Node { api: provider, wallet: wallet.clone() })),
            None => {
                tracing::error!(target: "voting", "[{}] skipped: wallet not found", node_id);
                None
            }
        })
        .collect::<HashMap<String, Node>>();

    let proposals = config
        .proposals
        .iter()
        .filter_map(|proposal_hash| {
            let mut out = [0; 32];
            hex::decode_to_slice(proposal_hash, &mut out)
                .map_err(|e| tracing::error!(target: "voting", "invalid proposal hash: {}", e))
                .ok()
                .map(|_| out)
        })
        .collect::<Vec<_>>();

    let mut runner = VotingRunner::new(nodes, config_contract, proposals);
    runner
        .run_loop(Duration::from_secs(config.tick_interval), cancellation_ctx)
        .await
        .context("voting loop error")
}

impl VotingRunner {
    fn new(
        nodes: HashMap<String, Node>,
        config_contract: Arc<dyn ConfigContractWrapper>,
        tracked_proposals: Vec<[u8; 32]>,
    ) -> Self {
        Self { nodes, config_contract, tracked_proposals }
    }

    async fn run_loop(
        &mut self,
        tick_interval: Duration,
        cancellation_ctx: CancellationCtx,
    ) -> anyhow::Result<()> {
        let mut cancellation_rx = cancellation_ctx.subscribe();
        tracing::info!(target: "voting", "tick interval: {} seconds", tick_interval.as_secs());
        let mut interval = tokio::time::interval(tick_interval);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    tracing::info!(target: "voting", "TICK");
                    if let Err(e) = &self.run().await {
                        tracing::error!(target: "voting", "run error: {}", e);
                    }
                    tracing::info!(target: "voting","SLEEP");
                }
                _ = cancellation_rx.changed() => {
                    tracing::info!(target: "voting", "cancel received");
                    if let Err(e) = self.shutdown().await {
                        tracing::error!(target: "voting", "runner failed to shutdown: {}", e);
                    }
                    return Ok(());
                }
            }
        }
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        let proposals = self
            .config_contract
            .list_proposals()
            .await
            .map_err(|e| anyhow::anyhow!("get proposals error: {}", e))?;
        tracing::info!(target: "voting", "active proposals: {} {}", proposals.len(), proposals.iter().map(|p| hex::encode(p.hash)).collect::<Vec<_>>().join(", "));
        let proposals: Vec<_> = proposals
            .into_iter()
            .filter(|proposal| self.tracked_proposals.contains(&proposal.hash))
            .collect();

        if proposals.is_empty() {
            tracing::warn!(target: "voting", "no tracked proposals found");
            return Ok(());
        }

        let mut nodes = self.nodes.keys().cloned().collect::<Vec<_>>();
        nodes.sort();

        let vset = self.get_current_vset().await?;
        for proposal in proposals {
            for node_id in &nodes {
                if let Err(e) = self.vote_for_proposal(node_id, &proposal, &vset).await {
                    tracing::error!(target: "voting", "node [{}] vote error: {}", node_id, e);
                }
            }
        }

        Ok(())
    }

    async fn vote_for_proposal(
        &mut self,
        node_id: &str,
        proposal: &ConfigProposal,
        vset: &ValidatorSet,
    ) -> anyhow::Result<()> {
        let node = self.nodes.get_mut(node_id).expect("node not found");
        let Some((validator_idx, validator_entry)) = Self::find_validator_entry(node, vset).await?
        else {
            tracing::warn!(target: "voting", "node [{}] voting skipped: not a validator", node_id);
            return Ok(());
        };

        if proposal.voters.contains(&validator_idx) {
            tracing::info!(target: "voting",
                "node [{}] already voted for proposal: hash={} validator_idx={}",
                node_id,
                hex::encode(proposal.hash),
                validator_idx
            );
            return Ok(());
        }

        tracing::info!(
            target: "voting",
            "node [{}] voting for proposal: hash={}, validator_idx={}",
            node_id,
            hex::encode(proposal.hash),
            validator_idx
        );

        // Build the data to sign
        let signing_data = config_contract::messages::unsigned_vote(validator_idx, &proposal.hash)?;

        // Sign the data with the validator key
        let signature = node
            .api
            .sign(signing_data.data(), validator_entry.key_id.clone())
            .await
            .map_err(|e| anyhow::anyhow!("sign error: {}", e))?;

        // Build the external message
        let config_addr = self.config_contract.address().await;
        let query_id = 0;
        let vote = config_contract::messages::signed_vote(query_id, &signing_data, &signature)?;
        let msg_cell = node
            .wallet
            .message(config_addr, SEND_VOTE_AMOUNT, vote)
            .await
            .map_err(|e| anyhow::anyhow!("send vote error: {}", e))?;
        let boc = write_boc(&msg_cell).context("write_boc")?;

        tracing::info!(
            target: "voting",
            "node [{}] send vote",
            node_id,
        );

        // Send the message to the network
        node.api.send_boc(&boc).await.map_err(|e| anyhow::anyhow!("send_boc error: {}", e))?;
        Ok(())
    }

    async fn get_current_vset(&mut self) -> anyhow::Result<ValidatorSet> {
        for (node_id, node) in self.nodes.iter_mut() {
            match node.api.get_current_vset().await {
                Ok(vset) => return Ok(vset),
                Err(e) => {
                    tracing::error!(target: "voting", "node [{}] get_current_vset error: {}", node_id, e)
                }
            }
        }
        anyhow::bail!("get_current_vset: all nodes failed");
    }

    async fn find_validator_entry(
        node: &mut Node,
        vset: &ValidatorSet,
    ) -> anyhow::Result<Option<(u16, ValidatorEntry)>> {
        let config = node
            .api
            .validator_config()
            .await
            .map_err(|e| anyhow::anyhow!("get validator config error: {}", e))?;

        // The validator config can have many epoch keys. To reduce the time spent checking each key
        // (as each check requires exporting the public key and searching for it in the vset),
        // we only check the last three election IDs.
        // We cannot check only the last election ID, because if new elections have started,
        // the last election ID will reference a new key for the next epoch.

        let mut election_ids = config.keys.keys().cloned().collect::<Vec<_>>();
        election_ids.sort();

        for election_id in &election_ids[election_ids.len().saturating_sub(3)..] {
            let entry = config
                .keys
                .get(election_id)
                .ok_or_else(|| anyhow::anyhow!("validator entry not found"))?;
            let public_key = node.api.export_public_key(&entry.key_id).await?;
            let mut key = [0u8; 32];
            key.copy_from_slice(&public_key);
            // Search validator public key in current vset
            let vset_entry = vset
                .list()
                .iter()
                .position(|item| item.public_key.as_slice() == &key)
                .map(|idx| (idx as u16, entry.clone()));
            if let Some((idx, entry)) = vset_entry {
                return Ok(Some((idx, entry)));
            }
        }
        Ok(None)
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        for (node_id, node) in self.nodes.iter_mut() {
            tracing::info!(target: "voting", "node [{}] shutdown provider", node_id);
            if let Err(e) = node.api.shutdown().await {
                tracing::error!(target: "voting", "node [{}] shutdown error: {}", node_id, e);
            }
        }
        Ok(())
    }
}
