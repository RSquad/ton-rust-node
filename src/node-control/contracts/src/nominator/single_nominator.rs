/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{NominatorRoles, NominatorWrapper, PoolConfig, PoolData};
use crate::{ContractProvider, SmartContract};
use anyhow::Context;
use std::sync::Arc;
use ton_block::{
    BuilderData, Deserializable, MsgAddressInt, Serializable, SliceData, StateInit,
    read_single_root_boc,
};

/// Code for single-nominator contract v1.1
const CODE_V1_1: &'static str = "b5ee9c7241020d010001f0000114ff00f4a413f4bcf2c80b01020162020a02bcd0ed44d0fa40fa40d122c700925f06e003d0d3030171b0925f06e0fa403002d31f7022c000228b1778c705b022d74ac000b08e136c21830bc85376a182103b9aca00a1fa02c9d09430d33f12e25343c7059133e30d5235c705925f06e30d030401c421830bba8ea0fa005387a182103b9aca00a112b60881200421c200f2f452406d80188040db3cde21811001ba9efa405044c858cf1601cf16c9ed549133e220817702ba9802d307d402fb0002de2082009903ba9d02d4812002226ef2f201fb0402de0904f22382104e73744bba8fe102fa4430f828fa443081200302c0ff12f2f4830c01c0fff2f481200122f2f481200524821047868c00bef2f4fa0020db3c300581200405a182103b9aca00a15210bb14f2f4db3c82104e73744bc8cb1f5220cb3f5005cf16c9443080188040db3c9410356c41e201821047657424ba05070906001cd3ff31d31fd31f31d3ff31d431d102368f16821047657424c8cb1fcb3fc9db3c705880188040db3c9130e20709011671f833d0d70bff7f01db3c08001674c8cb0212ca07cbffc9d00048226eb32091719170e203c8cb055006cf165004fa02cb6a039358cc019130e201c901fb000201200b0c0027bdf8cb938b82a38002a380036b6aa39152988b6c0015bfe5076a2687d207d2068c5fb766c2";
pub const NOMINATOR_POOL_WORKCHAIN: i32 = -1;
/// Implementation of the single-nominator contract wrapper
///
/// See: https://github.com/ton-blockchain/single-nominator
pub struct NominatorWrapperImpl {
    provider: Arc<dyn ContractProvider>,
    nominator_addr: MsgAddressInt,
    state_init: Option<StateInit>,
}

impl NominatorWrapperImpl {
    pub fn new(provider: Arc<dyn ContractProvider>, nominator_addr: MsgAddressInt) -> Self {
        Self { provider, nominator_addr, state_init: None }
    }

    pub fn from_init_data(
        provider: Arc<dyn ContractProvider>,
        owner_address: &MsgAddressInt,
        validator_address: &MsgAddressInt,
        workchain: i32,
    ) -> anyhow::Result<Self> {
        let (nominator_addr, state_init) =
            Self::calculate_address_with_state_init(workchain, owner_address, validator_address)?;
        Ok(Self { provider, nominator_addr, state_init: Some(state_init) })
    }

    pub fn calculate_address(
        wc: i32,
        owner_address: &MsgAddressInt,
        validator_address: &MsgAddressInt,
    ) -> anyhow::Result<MsgAddressInt> {
        Self::calculate_address_with_state_init(wc, owner_address, validator_address)
            .map(|(addr, _)| addr)
    }

    /// Calculate both the pool address and `StateInit` in a single pass.
    pub fn calculate_address_with_state_init(
        wc: i32,
        owner_address: &MsgAddressInt,
        validator_address: &MsgAddressInt,
    ) -> anyhow::Result<(MsgAddressInt, StateInit)> {
        let state_init = Self::build_state_init(owner_address, validator_address)?;
        let cell = state_init.write_to_new_cell()?.into_cell()?;
        let addr = MsgAddressInt::with_params(wc, cell.hash(0))?;
        Ok((addr, state_init))
    }

    pub fn build_state_init(
        owner_address: &MsgAddressInt,
        validator_address: &MsgAddressInt,
    ) -> anyhow::Result<StateInit> {
        let mut data = BuilderData::new();
        owner_address.write_to(&mut data)?;
        validator_address.write_to(&mut data)?;
        let code =
            read_single_root_boc(hex::decode(CODE_V1_1).expect("CODE_V1_1 code hex is invalid"))?;
        let state_init = StateInit::with_code_and_data(code, data.into_cell()?);

        Ok(state_init)
    }
}

#[async_trait::async_trait]
impl SmartContract for NominatorWrapperImpl {
    async fn balance(&self) -> anyhow::Result<u64> {
        self.provider.balance(&self.nominator_addr).await
    }

    fn address(&self) -> MsgAddressInt {
        self.nominator_addr.clone()
    }
}

#[async_trait::async_trait]
impl NominatorWrapper for NominatorWrapperImpl {
    fn state_init(&self) -> Option<StateInit> {
        self.state_init.clone()
    }

    async fn get_roles(&self) -> anyhow::Result<NominatorRoles> {
        let stack =
            self.provider.get_method(self.nominator_addr.to_string(), "get_roles", vec![]).await?;
        let owner_address =
            MsgAddressInt::construct_from(&mut SliceData::load_cell(stack.cell(0)?)?)
                .map_err(|e| anyhow::anyhow!("parse owner address error: {}", e))?;
        let validator_address =
            MsgAddressInt::construct_from(&mut SliceData::load_cell(stack.cell(1)?)?)
                .map_err(|e| anyhow::anyhow!("parse validator address error: {}", e))?;

        Ok(NominatorRoles { owner_address, validator_address })
    }

    async fn get_pool_data(&self) -> anyhow::Result<PoolData> {
        let stack = self
            .provider
            .get_method(self.nominator_addr.to_string(), "get_pool_data", vec![])
            .await?;
        let state = stack.i64(0).context("parse state")? as i32;
        let nominators_count = stack.i64(1).context("parse nominators_count")? as u32;
        let stake_amount_sent = stack.i64(2).context("parse stake_amount_sent")? as u64;
        let validator_amount = stack.i64(3).context("parse validator_amount")? as u64;
        // Parse pool config
        let validator_addr = {
            let mut array = [0u8; 32];
            array.copy_from_slice(&stack.number_bytes(4, 32).context("parse validator_addr")?);
            array
        };
        let validator_reward_share = stack.i64(5).context("parse validator_reward_share")? as u16;
        let max_nominators_count = stack.i64(6).context("parse max_nominators_count")? as u16;
        let min_validator_stake = stack.i64(7).context("parse min_validator_stake")? as u64;
        let nominator_stake_threshold = stack.i64(8).context("parse nominator_stake_threshold")? as u64;
        // skip indices 9-10 (nominators, withdraw_requests)
        let stake_at = stack.i64(11).context("parse stake_at")? as u32;
        let saved_validator_set_hash = {
            let bytes = stack.number_bytes(12, 32).context("parse saved_validator_set_hash")?;
            let mut array = [0u8; 32];
            array.copy_from_slice(&bytes);
            array
        };
        let validator_set_changes_count =
            stack.i64(13).context("parse validator_set_changes_count")? as i32;
        let validator_set_change_time =
            stack.i64(14).context("parse validator_set_change_time")? as u64;
        let stake_held_for = stack.i64(11).context("parse stake_held_for")? as u64;

        Ok(PoolData {
            state,
            nominators_count,
            stake_amount_sent,
            validator_amount,
            pool_config: PoolConfig {
                validator_addr,
                validator_reward_share,
                max_nominators_count,
                min_validator_stake,
                nominator_stake_threshold,
            },
            stake_at,
            saved_validator_set_hash,
            validator_set_changes_count,
            validator_set_change_time,
            stake_held_for,
        })
    }
}

// TON HTTP API integration tests for single nominator contract
#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract_provider;
    use std::str::FromStr;
    use ton_block::MsgAddressInt;
    use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

    fn open_nominator() -> Option<NominatorWrapperImpl> {
        let nominator_addr =
            MsgAddressInt::from_str("kf-d42Dwn_dzfdwlV_aEeX7WWnJ-bBU_eZp6CfKoMb4vQ3t0")
                .expect("Failed to parse nominator address");
        let url = match std::env::var("TON_HTTP_API_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping test: TON_HTTP_API_URL env variable not set");
                return None;
            }
        };

        let client = ClientJsonRpc::connect(url, None).expect("Failed to connect to Ton network");
        Some(NominatorWrapperImpl::new(contract_provider!(Arc::new(client)), nominator_addr))
    }

    #[tokio::test]
    async fn test_get_roles() {
        // Enable logging to see debug logs from ClientJsonRpc
        //let _ = env_logger::init();
        let Some(nominator) = open_nominator() else {
            return;
        };
        let roles = nominator.get_roles().await.expect("Failed to get roles");
        assert_eq!(
            roles.owner_address,
            MsgAddressInt::from_str(
                "0:0f06dd725549dd60a6ca3743ac532db52789cb15af1f943fe96adbb4c1a86155"
            )
            .unwrap()
        );
        assert_eq!(
            roles.validator_address,
            MsgAddressInt::from_str(
                "0:308f9a48e5bf1c61bddcbd65dca41376877f845536f698eacfd5c7844d07c324"
            )
            .unwrap()
        );
    }

    #[tokio::test]
    async fn test_get_pool_data() {
        // Enable logging to see debug logs from ClientJsonRpc
        //let _ = env_logger::init();
        let Some(nominator) = open_nominator() else {
            return;
        };
        let pool_data = nominator.get_pool_data().await.expect("Failed to get pool data");
        let expected = PoolData {
            state: 2,
            nominators_count: 1,
            validator_set_changes_count: 2,
            ..Default::default()
        };
        assert_eq!(pool_data, expected);
    }
}
