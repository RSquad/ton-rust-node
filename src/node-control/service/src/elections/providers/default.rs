/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::traits::{Account, ElectionsProvider, ValidatorConfig, ValidatorEntry};
use adnl::client::AdnlClientConfig;
use anyhow::Context;
use control_client::{
    client_adnl::ControlClientAdnl,
    client_api::{
        AddAdnlAddressRq, AddValidatorAdnlAddrRq, AddValidatorPermKeyRq, AddValidatorTempKeyRq,
        ClientAPI, SignRq,
    },
    config_params::{
        parse_config_param_15, parse_config_param_16, parse_config_param_17, parse_config_param_34,
        parse_config_param_36,
    },
};
use std::collections::HashMap;
use ton_block::{
    ConfigParam15, ValidatorSet,
    config_params::{ConfigParam16, ConfigParam17},
};

pub struct DefaultElectionsProvider {
    client: ControlClientAdnl,
}

impl DefaultElectionsProvider {
    pub fn new(config: AdnlClientConfig) -> Self {
        Self { client: ControlClientAdnl::new(config, 4) }
    }
}

#[async_trait::async_trait]
impl ElectionsProvider for DefaultElectionsProvider {
    async fn setup(&self) -> anyhow::Result<()> {
        Ok(())
    }
    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.client.shutdown().await
    }
    async fn new_validator_key(
        &mut self,
        since: u64,
        until: u64,
    ) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let key_id = self.client.generate_key_pair().await?;
        self.client
            .add_validator_perm_key(&AddValidatorPermKeyRq {
                key_hash: key_id.clone(),
                election_date: since as i32,
                expire_at: until as i32,
            })
            .await
            .context("add_validator_perm_key")?;
        self.client
            .add_validator_temp_key(&AddValidatorTempKeyRq {
                perm_key_hash: key_id.clone(),
                key_hash: key_id.clone(),
                expire_at: until as i32,
            })
            .await
            .context("add_validator_temp_key")?;
        let pub_key = self.client.export_key_pub(&key_id).await.context("export_key_pub")?;
        Ok((key_id, pub_key))
    }
    async fn new_adnl_addr(
        &mut self,
        validator_key_id: Vec<u8>,
        until: u64,
    ) -> anyhow::Result<Vec<u8>> {
        let key_id = self.client.generate_key_pair().await?;
        self.client
            .add_adnl_address(&AddAdnlAddressRq { key_hash: key_id.clone(), category: 0 })
            .await
            .context("add_adnl_address")?;
        self.client
            .add_validator_adnl_addr(&AddValidatorAdnlAddrRq {
                perm_key_hash: validator_key_id,
                key_hash: key_id.clone(),
                expire_at: until as i32,
            })
            .await
            .context("add_validator_adnl_addr")?;
        Ok(key_id)
    }
    async fn validator_config(&mut self) -> anyhow::Result<ValidatorConfig> {
        let remote = self.client.get_validator_config().await?;
        let keys = remote
            .validators
            .into_iter()
            .map(|validator| {
                let adnl_addrs: Vec<(Vec<u8>, u64)> = validator
                    .adnl_addrs
                    .iter()
                    .map(|entry| (entry.id.clone(), entry.expire_at as u64))
                    .collect();
                (
                    validator.election_date as u64,
                    ValidatorEntry {
                        key_id: validator.id,
                        public_key: vec![],
                        adnl_addrs,
                        expired_at: validator.expire_at as u64,
                    },
                )
            })
            .collect::<HashMap<u64, ValidatorEntry>>();
        Ok(ValidatorConfig { keys })
    }
    async fn election_parameters(&mut self) -> anyhow::Result<ConfigParam15> {
        let bytes = self.client.get_config_param(15).await?;
        parse_config_param_15(&bytes)
    }
    async fn send_boc(&mut self, msg_boc: &[u8]) -> anyhow::Result<()> {
        self.client.send_boc(msg_boc).await
    }
    async fn sign(&mut self, key_id: Vec<u8>, data: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        self.client.sign(&SignRq { key_hash: key_id, data }).await
    }
    async fn account(&mut self, address: &str) -> anyhow::Result<Account> {
        let account = self.client.get_account_state(address).await?;
        Ok(Account::new(account))
    }
    async fn export_public_key(&mut self, key_id: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.client.export_key_pub(key_id).await
    }
    async fn get_current_vset(&mut self) -> anyhow::Result<ValidatorSet> {
        let bytes = self.client.get_config_param(34).await?;
        parse_config_param_34(&bytes)
    }
    async fn config_param_16(&mut self) -> anyhow::Result<ConfigParam16> {
        let bytes = self.client.get_config_param(16).await?;
        parse_config_param_16(&bytes)
    }
    async fn config_param_17(&mut self) -> anyhow::Result<ConfigParam17> {
        let bytes = self.client.get_config_param(17).await?;
        parse_config_param_17(&bytes)
    }
    async fn get_next_vset(&mut self) -> anyhow::Result<Option<ValidatorSet>> {
        match self.client.get_config_param(36).await {
            Ok(bytes) => Ok(Some(parse_config_param_36(&bytes)?)),
            Err(e) => {
                tracing::trace!("get_next_vset: config param 36 not available: {e:?}");
                Ok(None)
            }
        }
    }
    async fn generate_adnl_addr(&mut self) -> anyhow::Result<Vec<u8>> {
        let key_id = self.client.generate_key_pair().await?;
        self.client
            .add_adnl_address(&AddAdnlAddressRq { key_hash: key_id.clone(), category: 0 })
            .await
            .context("add_adnl_address")?;
        Ok(key_id)
    }
    async fn register_adnl_addr(
        &mut self,
        adnl_key_id: Vec<u8>,
        validator_key_id: Vec<u8>,
        until: u64,
    ) -> anyhow::Result<()> {
        self.client
            .add_validator_adnl_addr(&AddValidatorAdnlAddrRq {
                perm_key_hash: validator_key_id,
                key_hash: adnl_key_id,
                expire_at: until as i32,
            })
            .await
            .context("add_validator_adnl_addr")
    }
}
