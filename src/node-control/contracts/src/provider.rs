/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use common::tvm_stack_parser::TvmStackParser;
use std::sync::Arc;
use ton_api::ton::tvm::StackEntry;
use ton_block::MsgAddressInt;
use ton_http_api_client::v2::{
    RPCStackEntry, client_json_rpc::ClientJsonRpc, data_models::RunGetMethodParams,
};

/// Creates a new `ContractProvider` instance.
///
/// # Example
/// ```ignore
/// let rpc_client = Arc::new(ClientJsonRpc::new(...));
/// let provider = contract_provider!(rpc_client);
/// ```
#[macro_export]
macro_rules! contract_provider {
    ($rpc_client:expr) => {
        std::sync::Arc::new($crate::provider::ContractProviderImpl::new($rpc_client))
            as std::sync::Arc<dyn $crate::provider::ContractProvider>
    };
}

#[async_trait::async_trait]
pub trait ContractProvider: Send + Sync {
    async fn get_method(
        &self,
        address: String,
        method: &str,
        stack: Vec<StackEntry>,
    ) -> anyhow::Result<TvmStackParser>;
    async fn balance(&self, address: &MsgAddressInt) -> anyhow::Result<u64>;
}

pub struct ContractProviderImpl {
    rpc_client: Arc<ClientJsonRpc>,
}

impl ContractProviderImpl {
    pub fn new(rpc_client: Arc<ClientJsonRpc>) -> Self {
        Self { rpc_client }
    }
}

#[async_trait::async_trait]
impl ContractProvider for ContractProviderImpl {
    async fn get_method(
        &self,
        address: String,
        method: &str,
        stack: Vec<StackEntry>,
    ) -> anyhow::Result<TvmStackParser> {
        let result = self
            .rpc_client
            .run_get_method(&RunGetMethodParams {
                address,
                method_id: method.to_owned(),
                stack: Some(stack.into_iter().map(RPCStackEntry::from).collect::<Vec<_>>()),
                seqno: None,
            })
            .await
            .map_err(|e| anyhow::anyhow!("get-method {} error: {}", method, e))?;
        if result.exit_code != 0 {
            anyhow::bail!("get-method {} error: exit_code={}", method, result.exit_code);
        }
        Ok(TvmStackParser::new(result.stack.into_iter().map(Into::into).collect::<Vec<_>>()))
    }
    async fn balance(&self, address: &MsgAddressInt) -> anyhow::Result<u64> {
        let info = self
            .rpc_client
            .get_address_information(&address)
            .await
            .context("Failed to get account info")?;
        Ok(info.balance)
    }
}
