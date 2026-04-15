/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    ContractProvider, SmartContract,
    nominator::{
        NominatorRoles, NominatorWrapper, PoolConfig, PoolData, PoolKind, TONCORE_STORAGE_RESERVE,
    },
};
use anyhow::Context;
use common::app_config::{
    TonCoreInitParams,
};
use std::sync::Arc;
use ton_block::{
    BuilderData, Coins, IBitstring, MsgAddressInt, Serializable, StateInit, read_single_root_boc,
};

/// Compiled code of the nominator-pool contract.
///
/// Obtain by compiling the FunC source with `func` + `fift`:
/// <https://github.com/ton-blockchain/nominator-pool/tree/main/func>
const CODE: &str = "b5ee9c7201023a010009c2000114ff00f4a413f4bcf2c80b0102016202030202ce0405020120131402012006070065421d749ab02705203aa008e23aa0303f00114a002a45301ba8e1323d74ac0019c5b01d430d020d749ab021270dede02e46c218047f3e09dbc400b434c0fe900c083e9100dc6c23c88c4cccc835d2708fe3c5200835c874c7cc2084139cdd12ee80b6cf2c38c02497c0f8b800f4c7f6cf1584b0002021081f09004f34c1c069b40830bffcb852483042b729be4830bffcb8524830443729b80830bfc870442c3cb852600330db3c5610c00193705711de104c103b4a98db3c085533db3c1f0c12042ce30f5540db3c105c104b103a497810561045103440330a0b0c0d03a257121110d30721c07922c06eb122c06423c077b121b1f2e04020b39e21d15616c000f2bd56152ebdf2bede22c064e30022c077925717e30d11168e1330041115040311140302111302571157115f03e30d0e0f1003341111d33f56165616db3ce30f0b11100b10bf10be10bd10bc10ab2122230028c88101001026cf0113cb0fcb0f01fa0201fa02c90104db3c1202d8810100561652a2f40e6fa120b3951112a41112de56122ebbf2e04182103b9aca0001111b01a120c200f2e042111a8e82db3c93307020e25613c0009401561aa094561aa001e25301a02cbef2e0432ad765755614b603aa00b609b9f2e04401db3c81010012561740bbf443082f2503a45611c0008f2156150410391028011118011111db3c015618a18212540be400be8e845613db3cde8ea3571781010056155292f40e6fa131f2e045c88101001256164099f4435613db3c4f0702e24f1f50770629303002fe5614c0ff56142dbab0b38e9d1114c000f2e07981010056135272f40e6fa1f2e07adb3c30c200f2e07b925714e211148020f00201d11113c079561356118307f40e6fa120b38e1982103b9aca005613d76595800f7aa984e401111801bef2e07b925717e2561695f404d31f3094306df823e25614228307f40e6fa131f2d07c2f11016cf82303c8ca0013cb1f021114018307f443c8f40001111201cb1f02011112010f8307f44311128e830ddb3c913de20c11100c10bf10bc30004a0cc8cb071bcb0f5009fa025007fa0215cc13f400f400cb1fcbffcb07cb1fcb1ff400c9ed540201201516020120191a0109bbf19db3c81f02016217180175af3bed9e2b882f87b6acc183fa0737d0f97042fa02183fc70fc0808029107a3e37d2904f816900698f98112cb781a802378101c8997100d9f32dc01f0109ac8b6d9e403302016e1b1c015dbbd05db3c57105f0f6d7f8e1f228307f47c6fa5208e1002f40431d31f3052106f0250036f02029132e201b3e6303181f0201201d1e0117ae3eed9e0837af8798b759c01f0276aa39db3c5f06509a5f096d7f8ea98101005230f47c6fa5208e9802db3c810100546380f40e6fa1312355206f0450036f02029132e201b3e6135f031f2f0244ab59db3c5f06509a5f098101002359f40e6fa1f2e056db3c8101004430f40e6fa1311f2f0154ed44d0d307d30ffa00fa00d401d0db3c05f404f404d31fd3ffd307d31fd31ff4043010bc10ab109a108920001c810100d701d30fd30ffa00fa0030001e01c0ff71f833d0810100d70358bab001e85b5712571257125712f8008210f96f732452e0ba8eb93b11117009a15380c1019a5088a020c100923727de8e16305305a8812710a9045301bc923020de5188a008a107e25077db3c270a11110a080a925712e22ac0018e198210ee6f454c52d0ba92703bde8210f374484c1dba92723ade913ce22404b85613c2005614c108b0821047657424561501bab182104e73744b561501bab1f2e0465613c001305613c0028f24d3071039102856180201111201db3c5619a18212540be400be8e845614db3cde11104870de5613c003e3005613c0062630272803ba707f8e988101005230f47c6fa5208e8702db3c3013a0029132e201b3e6306d7f8f378101005240f47c6fa5208f2602db3c25c2009f547715a98412a020c100923070de01dea070db3c8101005412015055f443029132e201b3e6145f042f2f25000ec858fa0201fa020172707f218eb0810100542270f47c6fa532218e9c3254411348705266db3c5217ba05a45304be927f36de103847634550de01b322b112e65f0401290268810100d7018101005462a0f40e6fa131f2e0474930185618011112db3c015619a18212540be400be8e845614db3cde1110487012293004d68f2024c103f2e071db3c6c21f9005360bd99343503a44413f823039130e25614db3cde5613c0078eb7f8237f8e2c56148307f47c6fa5208e1c02f40431d31f305230a18208278d00bc9a2011168307f45b301115de9132e201b3e65b5614db3cde821047657424561401ba3430302a03b2810100546550f40e6fa1f2bcdb3ca08212540be4005230a15210bc93306c14e0810100544666f45b30810100544655f45b3001a55124a182103b9aca005250be8f11705006db3c6d80101023102670db3c1023923434e243302f393804e08f3024c201f2e06f24c202f82325a124a63cbcb1f2e070821047657424c8cb1f5220cb3fc9db3c708018804010341023db3cde5613c0048e235616c0ff56162fbab0f2e04982103b9aca0001111901a120c200f2e04a51eea00e1118de5613c005925714e30d82104e73744b561301ba37382b2c04a85611c000f2e04a5616c0ff56162fbab0f2e04bfa0021c200f2e04e29db3c8212540be400561a01a101a15220bbf2e04c51f1a120c100923070de7f2fdb3c6d8010245970db3c561858a15619a18212540be400be2d39382e014e8e173005111605041115040311140302111302571157115f04e30d0f11100f10ef10de10cd10bc31013e707f8e988101005230f47c6fa5208e8702db3ca013a0029132e201b3e630312f011c8e841114db3c925714e20d11130d30000afa00fa00300114706d8010804072a0db3c3804d63e5f050fc0ff51e6ba1eb0f2e04e08c000f2e04f25f2e05082103b9aca001fbef2e05609fa0020db3c82103b9aca005230a18218746a5288005240bef2e0518212540be40001111001a15230bbf2e052535fbef2e0532edb3c5260bef2e0542d6ef2e05571db3c31f9007032333435001cd3ff31d31fd31f31d3ff31d431d100848028f833206e985b8218178411b200e0d0d30731fa00d31fd30fd30fd30f31d30f31d30fd30f305053a8ab075033a8ab075023a8ab0759a8ab075220a9b41fa0b60800268022f83320d0d30701c012f289d31fd31f3058035cdb3cdb3c1110c8cb1f1ccb3f5006cf16c9801871041110041038db3c0e11100e1f103e102d10bc107b50990743133637380022800ff833d0d31f31d31f31d31f31d70b1f011a71f833d0810100d7037f01db3c390048226eb32091719170e203c8cb055006cf165004fa02cb6a039358cc019130e201c901fb00001c74c8cb0212ca07810100cf01c9d0";

/// Pool is always deployed in the masterchain.
pub const POOL_WORKCHAIN: i32 = -1;

fn toncore_pool_address_from_state_init(state_init: &StateInit) -> anyhow::Result<MsgAddressInt> {
    let cell = state_init.write_to_new_cell()?.into_cell()?;
    let addr = MsgAddressInt::with_params(POOL_WORKCHAIN, cell.hash(0))?;
    Ok(addr)
}

/// Build the `StateInit` for deploying a new TONCore nominator pool.
///
/// Data layout follows `save_data` / `load_data` in pool.fc:
/// ```text
/// state:8 nominators_count:16 stake_amount_sent:coins validator_amount:coins
/// config:^Cell nominators:dict withdraw_requests:dict
/// stake_at:32 saved_validator_set_hash:256 validator_set_changes_count:8
/// validator_set_change_time:32 stake_held_for:32 config_proposal_votings:dict
/// ```
fn build_toncore_pool_state_init(
    validator_address: &MsgAddressInt,
    params: &TonCoreInitParams,
) -> anyhow::Result<StateInit> {
    let mut config = BuilderData::new();
    let validator_hash = validator_address.address().get_bytestring(0);
    anyhow::ensure!(validator_hash.len() == 32, "validator address must be 256 bits");
    config.append_raw(&validator_hash, 256)?;
    config.append_raw(&params.validator_share.to_be_bytes(), 16)?;
    config.append_raw(&params.max_nominators.to_be_bytes(), 16)?;
    Coins::new(params.min_validator_stake).write_to(&mut config)?;
    Coins::new(params.min_nominator_stake).write_to(&mut config)?;
    let config_cell = config.into_cell()?;

    let mut data = BuilderData::new();
    data.append_u8(0)?;
    data.append_raw(&0u16.to_be_bytes(), 16)?;
    Coins::new(0u64).write_to(&mut data)?;
    Coins::new(0u64).write_to(&mut data)?;
    data.checked_append_reference(config_cell)?;
    data.append_bit_zero()?;
    data.append_bit_zero()?;
    data.append_u32(0)?;
    data.append_raw(&[0u8; 32], 256)?;
    data.append_u8(0)?;
    data.append_u32(0)?;
    data.append_u32(0)?;
    data.append_bit_zero()?;

    let code =
        read_single_root_boc(hex::decode(CODE).expect("nominator pool code hex is invalid"))?;
    Ok(StateInit::with_code_and_data(code, data.into_cell()?))
}

/// Derive masterchain pool address and `StateInit` from deployment parameters.
pub(crate) fn toncore_pool_address_and_state(
    ton_core_init_params: &TonCoreInitParams,
    validator_address: &MsgAddressInt,
) -> anyhow::Result<(MsgAddressInt, StateInit)> {
    let state_init = build_toncore_pool_state_init(validator_address, ton_core_init_params)?;
    let addr = toncore_pool_address_from_state_init(&state_init)?;
    Ok((addr, state_init))
}

/// Resolved pool address and `StateInit` for a TONCore config.
pub struct ResolvedTonCorePool {
    pub params: TonCoreInitParams,
    pub address: MsgAddressInt,
    pub state_init: StateInit,
}

/// Validate and resolve the pool address from TONCore config fields.
pub fn resolve_toncore_pool(
    validator_addr: &MsgAddressInt,
    pool_address: Option<&str>,
    params: TonCoreInitParams,
) -> anyhow::Result<ResolvedTonCorePool> {
    let (address, state_init) = toncore_pool_address_and_state(&params, validator_addr)?;
    if let Some(addr) = pool_address {
        let explicit = addr
            .parse::<MsgAddressInt>()
            .context(format!("invalid TONCore pool address: {addr}"))?;
        anyhow::ensure!(
            explicit == address,
            "TONCore pool address ({explicit}) does not match derived address ({address})"
        );
    }

    Ok(ResolvedTonCorePool { params, address, state_init })
}

pub struct TonCoreNominatorWrapper {
    provider: Arc<dyn ContractProvider>,
    pool_addr: MsgAddressInt,
    state_init: Option<StateInit>,
}

impl TonCoreNominatorWrapper {
    pub fn new(provider: Arc<dyn ContractProvider>, pool_addr: MsgAddressInt) -> Self {
        Self { provider, pool_addr, state_init: None }
    }

    pub fn new_with_state_init(
        provider: Arc<dyn ContractProvider>,
        pool_addr: MsgAddressInt,
        state_init: StateInit,
    ) -> Self {
        Self { provider, pool_addr, state_init: Some(state_init) }
    }

    pub fn from_init_data(
        provider: Arc<dyn ContractProvider>,
        validator_address: &MsgAddressInt,
        init_params: &TonCoreInitParams,
    ) -> anyhow::Result<Self> {
        let (pool_addr, state_init) =
            toncore_pool_address_and_state(&init_params, validator_address)?;
        Ok(Self { provider, pool_addr, state_init: Some(state_init) })
    }

    pub fn calculate_address(
        validator_address: &MsgAddressInt,
        params: &TonCoreInitParams,
    ) -> anyhow::Result<MsgAddressInt> {
        toncore_pool_address_and_state(&params, validator_address).map(|(addr, _)| addr)
    }
}

#[async_trait::async_trait]
impl SmartContract for TonCoreNominatorWrapper {
    async fn balance(&self) -> anyhow::Result<u64> {
        self.provider.balance(&self.pool_addr).await
    }

    async fn address(&self) -> anyhow::Result<MsgAddressInt> {
        Ok(self.pool_addr.clone())
    }
}

#[async_trait::async_trait]
impl NominatorWrapper for TonCoreNominatorWrapper {
    fn pool_kind(&self) -> PoolKind {
        PoolKind::TONCore
    }

    fn storage_reserve(&self) -> u64 {
        TONCORE_STORAGE_RESERVE
    }

    fn state_init(&self) -> Option<StateInit> {
        self.state_init.clone()
    }

    fn inner_pools(&self) -> Vec<Arc<dyn NominatorWrapper>> {
        vec![]
    }

    async fn get_roles(&self) -> anyhow::Result<NominatorRoles> {
        let pool_data = self.get_pool_data().await?;
        let validator_address = MsgAddressInt::with_standart(
            None,
            POOL_WORKCHAIN as i8,
            pool_data.pool_config.validator_addr.into(),
        )?;
        Ok(NominatorRoles { owner_address: validator_address.clone(), validator_address })
    }

    async fn get_pool_data(&self) -> anyhow::Result<PoolData> {
        let stack =
            self.provider.get_method(self.pool_addr.to_string(), "get_pool_data", vec![]).await?;

        let state = stack.i64(0).context("parse state")? as i32;
        let nominators_count = stack.i64(1).context("parse nominators_count")? as u32;
        let stake_amount_sent = stack.i64(2).context("parse stake_amount_sent")? as u64;
        let validator_amount = stack.i64(3).context("parse validator_amount")? as u64;

        let validator_addr = {
            let mut array = [0u8; 32];
            array.copy_from_slice(&stack.number_bytes(4, 32).context("parse validator_addr")?);
            array
        };
        let validator_reward_share = stack.i64(5).context("parse validator_reward_share")? as u16;
        let max_nominators_count = stack.i64(6).context("parse max_nominators_count")? as u16;
        let min_validator_stake = stack.i64(7).context("parse min_validator_stake")? as u64;
        let min_nominator_stake = stack.i64(8).context("parse min_nominator_stake")? as u64;
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
        let stake_held_for = stack.i64(15).context("parse stake_held_for")? as u64;

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
                nominator_stake_threshold: min_nominator_stake,
            },
            stake_at,
            saved_validator_set_hash,
            validator_set_changes_count,
            validator_set_change_time,
            stake_held_for,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract_provider;
    use std::str::FromStr;
    use ton_block::MsgAddressInt;
    use ton_http_api_client::v2::client_json_rpc::ClientJsonRpc;

    fn open_pool() -> Option<TonCoreNominatorWrapper> {
        let pool_addr = MsgAddressInt::from_str("kf-d42Dwn_dzfdwlV_aEeX7WWnJ-bBU_eZp6CfKoMb4vQ3t0")
            .expect("Failed to parse pool address");
        let url = match std::env::var("TON_HTTP_API_URL") {
            Ok(url) => url,
            Err(_) => {
                eprintln!("Skipping test: TON_HTTP_API_URL env variable not set");
                return None;
            }
        };

        let client = ClientJsonRpc::connect(url, None).expect("Failed to connect to TON network");
        Some(TonCoreNominatorWrapper::new(contract_provider!(Arc::new(client)), pool_addr))
    }

    #[tokio::test]
    async fn test_get_pool_data() {
        let Some(pool) = open_pool() else {
            return;
        };
        let data = pool.get_pool_data().await.expect("Failed to get pool data");
        assert!(data.pool_config.max_nominators_count <= 40);
    }

    #[tokio::test]
    async fn test_get_roles() {
        let Some(pool) = open_pool() else {
            return;
        };
        let roles = pool.get_roles().await.expect("Failed to get roles");
        assert_eq!(roles.owner_address, roles.validator_address);
    }
}
