/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::client_api::{
    Account, AddAdnlAddressRq, AddValidatorAdnlAddrRq, AddValidatorPermKeyRq,
    AddValidatorTempKeyRq, BlockchainConfigInfo, ClientAPI, EngineValidatorConfig,
    ShardAccountState, Shutdown, SignRq,
};
use adnl::client::{AdnlClient, AdnlClientConfig, AdnlClientConfigJson};
use anyhow::Context;
use ton_api::{
    AnyBoxedSerialize, TLObject, serialize_boxed,
    ton::{
        self, engine::validator::ControlQueryError as TonControlQueryError,
        raw::ShardAccountState as TonShardAccountState,
        rpc::engine::validator::ControlQuery as TonControlQuery,
    },
};
use ton_block::{BlockIdExt, Deserializable, ShardAccount, UInt256, UnixTime, write_boc};

pub trait ToFromTL {
    type Rq;
    type Rs;

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject>;
    fn deserialize(answer: TLObject) -> anyhow::Result<Self::Rs>;
}

fn downcast<T: ton_api::AnyBoxedSerialize>(data: TLObject) -> anyhow::Result<T> {
    match data.downcast::<T>() {
        Ok(result) => Ok(result),
        Err(obj) => anyhow::bail!("Wrong downcast {:?} to {}", obj, std::any::type_name::<T>()),
    }
}

struct GetAccountRqRs {}

impl ToFromTL for GetAccountRqRs {
    type Rq = String;
    type Rs = Account;

    fn serialize(address: &Self::Rq) -> anyhow::Result<TLObject> {
        Ok(ton::rpc::raw::GetShardAccountState {
            account_address: ton_api::ton::accountaddress::AccountAddress {
                account_address: address.to_owned(),
            },
        }
        .into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        Ok(match downcast::<TonShardAccountState>(rs)? {
            TonShardAccountState::Raw_ShardAccountNone => Account::Nonexist,
            TonShardAccountState::Raw_ShardAccountState(account_state) => {
                let shard_account =
                    ShardAccount::construct_from_bytes(&account_state.shard_account)?;
                let account = shard_account.read_account()?;
                let sas = ShardAccountState {
                    status: account.status(),
                    balance: account.balance().map_or(0, |val| val.coins.as_u128()),
                    last_paid: account.last_paid(),
                    last_trans: shard_account.last_trans_lt(),
                    data: write_boc(&shard_account.account_cell())?,
                };

                Account::ShardAccountState(sas)
            }
        })
    }
}

struct GetBlockchainConfigRqRs {}

impl ToFromTL for GetBlockchainConfigRqRs {
    type Rq = ();
    type Rs = BlockchainConfigInfo;

    fn serialize(_: &Self::Rq) -> anyhow::Result<TLObject> {
        Ok(ton::rpc::lite_server::GetConfigAll::default().into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        let config_info = downcast::<ton_api::ton::lite_server::ConfigInfo>(rs)?;

        Ok(BlockchainConfigInfo {
            state_proof: config_info.state_proof().clone(),
            config_proof: config_info.config_proof().clone(),
        })
    }
}

struct GetValidatorConfigRqRs {}

impl ToFromTL for GetValidatorConfigRqRs {
    type Rq = ();
    type Rs = EngineValidatorConfig;

    fn serialize(_: &Self::Rq) -> anyhow::Result<TLObject> {
        Ok(ton::rpc::engine::validator::GetConfig.into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        let config = downcast::<ton_api::ton::engine::validator::JsonConfig>(rs)?;
        let engine_validator_config = serde_json::from_str::<EngineValidatorConfig>(config.data())?;
        Ok(engine_validator_config)
    }
}

struct GetConfigParamRqRs {}

impl ToFromTL for GetConfigParamRqRs {
    type Rq = u32;
    type Rs = Vec<u8>;

    fn serialize(id: &Self::Rq) -> anyhow::Result<TLObject> {
        let param_id =
            i32::try_from(*id).map_err(|_| anyhow::anyhow!("id value does not fit into i32"))?;
        let param_list = vec![param_id];
        Ok(ton::rpc::lite_server::GetConfigParams {
            mode: 0,
            id: BlockIdExt::default(),
            param_list,
        }
        .into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        let config_info = downcast::<ton_api::ton::lite_server::ConfigInfo>(rs)?;
        Ok(config_info.only().config_proof)
    }
}

struct SignRqRs {}

impl ToFromTL for SignRqRs {
    type Rq = SignRq;
    type Rs = Vec<u8>;

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject> {
        let key_hash = UInt256::from_raw(rq.key_hash.clone(), 256);
        let ret = ton::rpc::engine::validator::Sign { key_hash, data: rq.data.clone() };
        Ok(ret.into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        let answer = downcast::<ton_api::ton::engine::validator::Signature>(rs)?;
        let signature = answer.signature().clone();

        Ok(signature)
    }
}

struct GenerateKeyPairRqRs {}

impl ToFromTL for GenerateKeyPairRqRs {
    type Rq = ();
    type Rs = Vec<u8>;

    fn serialize(_: &Self::Rq) -> anyhow::Result<TLObject> {
        Ok(ton::rpc::engine::validator::GenerateKeyPair.into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        let ton_key_hash = downcast::<ton_api::ton::engine::validator::KeyHash>(rs)?;
        let key_hash = ton_key_hash.key_hash().as_slice().to_vec();

        Ok(key_hash)
    }
}

struct ExportKeyPubRqRs {}

impl ToFromTL for ExportKeyPubRqRs {
    type Rq = Vec<u8>;
    type Rs = Vec<u8>;

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject> {
        let key_hash = UInt256::from_raw(rq.to_owned(), 256);

        let ret = ton::rpc::engine::validator::ExportPublicKey { key_hash };
        Ok(ret.into_tl_object())
    }

    fn deserialize(rs: TLObject) -> anyhow::Result<Self::Rs> {
        let answer = downcast::<ton_api::ton::PublicKey>(rs)?;
        let pub_key = match answer.key() {
            Some(key) => key.clone().into_vec(),
            None => anyhow::bail!("Public key not found in answer!"),
        };

        Ok(pub_key)
    }
}

struct AddValidatorPermKeyRqRs {}

impl ToFromTL for AddValidatorPermKeyRqRs {
    type Rq = AddValidatorPermKeyRq;
    type Rs = ();

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject> {
        let key_hash = UInt256::from_raw(rq.key_hash.clone(), 256);
        let election_date = rq.election_date;
        let ttl = rq.expire_at - election_date;

        let ret =
            ton::rpc::engine::validator::AddValidatorPermanentKey { key_hash, election_date, ttl };
        Ok(ret.into_tl_object())
    }

    fn deserialize(_: TLObject) -> anyhow::Result<Self::Rs> {
        Ok(())
    }
}

struct AddValidatorTempKeyRqRs {}

impl ToFromTL for AddValidatorTempKeyRqRs {
    type Rq = AddValidatorTempKeyRq;
    type Rs = ();

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject> {
        let perm_key_hash = UInt256::from_raw(rq.perm_key_hash.clone(), 256);
        let key_hash = UInt256::from_raw(rq.key_hash.clone(), 256);
        let expire_at = rq.expire_at;
        let ttl = expire_at - UnixTime::now() as i32;

        let ret = ton::rpc::engine::validator::AddValidatorTempKey {
            permanent_key_hash: perm_key_hash,
            key_hash,
            ttl,
        };

        Ok(ret.into_tl_object())
    }

    fn deserialize(_: TLObject) -> anyhow::Result<Self::Rs> {
        Ok(())
    }
}

struct AddAdnlAddressRqRs {}

impl ToFromTL for AddAdnlAddressRqRs {
    type Rq = AddAdnlAddressRq;
    type Rs = ();

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject> {
        let key_hash = UInt256::from_raw(rq.key_hash.clone(), 256);
        let category = rq.category;

        if !(0..=15).contains(&category) {
            anyhow::bail!("category must be not negative and less than 16")
        }
        let ret = ton::rpc::engine::validator::AddAdnlId { key_hash, category };

        Ok(ret.into_tl_object())
    }

    fn deserialize(_: TLObject) -> anyhow::Result<Self::Rs> {
        Ok(())
    }
}

struct AddValidatorAdnlAddrRqRs {}

impl ToFromTL for AddValidatorAdnlAddrRqRs {
    type Rq = AddValidatorAdnlAddrRq;
    type Rs = ();

    fn serialize(rq: &Self::Rq) -> anyhow::Result<TLObject> {
        let perm_key_hash = UInt256::from_raw(rq.perm_key_hash.clone(), 256);
        let key_hash = UInt256::from_raw(rq.key_hash.clone(), 256);
        let expire_at = rq.expire_at;
        let ttl = expire_at - UnixTime::now() as i32;

        let ret = ton::rpc::engine::validator::AddValidatorAdnlAddress {
            permanent_key_hash: perm_key_hash,
            key_hash,
            ttl,
        };

        Ok(ret.into_tl_object())
    }

    fn deserialize(_: TLObject) -> anyhow::Result<Self::Rs> {
        Ok(())
    }
}

struct SendBocRqRs {}

impl ToFromTL for SendBocRqRs {
    type Rq = Vec<u8>;
    type Rs = ();

    fn serialize(boc: &Self::Rq) -> anyhow::Result<TLObject> {
        let ret = ton::rpc::lite_server::SendMessage { body: boc.to_owned() };
        Ok(ret.into_tl_object())
    }

    fn deserialize(_: TLObject) -> anyhow::Result<Self::Rs> {
        Ok(())
    }
}

pub struct ControlClientAdnl {
    config: AdnlClientConfig,
    adnl: Option<AdnlClient>,
    max_rq_attempts: u32,
}

impl ControlClientAdnl {
    /// Create a new disconnected control client.
    ///
    /// Connection will be established when the first request is made.
    pub fn new(config: AdnlClientConfig, max_rq_attempts: u32) -> Self {
        Self { config, adnl: None, max_rq_attempts }
    }

    /// Create a new disconnected control client from a JSON configuration.
    ///
    /// Connection will be established when the first request is made.
    pub fn new_from_json(config_json: &AdnlClientConfigJson) -> anyhow::Result<Self> {
        let (_, config) = AdnlClientConfig::from_json_config(config_json)?;
        Ok(Self::new(config, 4))
    }

    /// Establish connection to the Control Server via ADNL.
    ///
    /// If connection is already established, do nothing.
    /// It is not necessary to call this method before making requests,
    /// but it can be used to force connection establishment.
    pub async fn connect(&mut self) -> anyhow::Result<()> {
        if self.adnl.is_none() {
            self.adnl = Some(
                AdnlClient::connect(&self.config)
                    .await
                    .context("failed to connect to Control Server")?,
            );
        }
        Ok(())
    }

    /// Shutdown the Control Client.
    ///
    /// If connection is not established, do nothing.
    /// Call this method to ensure the connection is closed.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        if let Some(adnl) = self.adnl.take() {
            adnl.shutdown().await?;
        }
        Ok(())
    }

    pub async fn ping(&mut self) -> anyhow::Result<u64> {
        let adnl = self.adnl.as_mut().context("ADNL client not connected")?;
        adnl.ping().await
    }

    pub async fn reconnect(&mut self) -> anyhow::Result<()> {
        if let Some(adnl) = self.adnl.take() {
            if let Err(e) = adnl.shutdown().await {
                tracing::error!(target: "control-client", "failed to shut down ADNL client: {}", e)
            }
        }

        self.adnl = Some(AdnlClient::connect(&self.config).await?);
        Ok(())
    }

    async fn do_rq<T>(&mut self, rq: &T::Rq) -> anyhow::Result<T::Rs>
    where
        T: ToFromTL,
    {
        let tl_object_rq = T::serialize(rq)?;
        let tl_object_rq_boxed =
            TonControlQuery { data: serialize_boxed(&tl_object_rq)? }.into_tl_object().into();

        // Establish connection if not established yet
        self.connect().await?;

        let mut attempt = 1;

        loop {
            let adnl = self.adnl.as_mut().context("control client not connected")?;
            let res = adnl.query(&tl_object_rq_boxed).await;

            match res {
                Ok(tl_object) => match tl_object.downcast::<TonControlQueryError>() {
                    Err(tl_object_rs) => match T::deserialize(tl_object_rs) {
                        Err(err) => {
                            anyhow::bail!("Wrong response to {:?}: {:?}", tl_object_rq, err)
                        }
                        Ok(result) => return Ok(result),
                    },
                    Ok(error) => anyhow::bail!("Error response to {:?}: {:?}", tl_object_rq, error),
                },
                Err(err) => {
                    tracing::debug!(target: "control-client", "control query error: {}", err);
                    if attempt >= self.max_rq_attempts {
                        tracing::error!(target: "control-client", "max reconnecting attempts reached");
                        anyhow::bail!("control query error: {}", err)
                    }

                    tracing::debug!( target: "control-client",
                        "reconnect and repeat request: attempt {}/{}",
                        attempt,
                        self.max_rq_attempts,
                    );

                    self.reconnect().await?;
                    attempt += 1;
                    continue;
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl ClientAPI for ControlClientAdnl {
    async fn get_account_state(&mut self, address: &str) -> anyhow::Result<Account> {
        self.do_rq::<GetAccountRqRs>(&address.to_string()).await
    }

    async fn get_blockchain_config(&mut self) -> anyhow::Result<BlockchainConfigInfo> {
        self.do_rq::<GetBlockchainConfigRqRs>(&()).await
    }

    async fn get_validator_config(&mut self) -> anyhow::Result<EngineValidatorConfig> {
        self.do_rq::<GetValidatorConfigRqRs>(&()).await
    }

    async fn get_config_param(&mut self, id: u32) -> anyhow::Result<Vec<u8>> {
        self.do_rq::<GetConfigParamRqRs>(&id).await
    }

    async fn sign(&mut self, rq: &SignRq) -> anyhow::Result<Vec<u8>> {
        self.do_rq::<SignRqRs>(rq).await
    }

    async fn generate_key_pair(&mut self) -> anyhow::Result<Vec<u8>> {
        self.do_rq::<GenerateKeyPairRqRs>(&()).await
    }

    async fn export_key_pub(&mut self, key_hash: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.do_rq::<ExportKeyPubRqRs>(&key_hash.to_vec()).await
    }

    async fn add_validator_perm_key(&mut self, rq: &AddValidatorPermKeyRq) -> anyhow::Result<()> {
        self.do_rq::<AddValidatorPermKeyRqRs>(rq).await
    }

    async fn add_validator_temp_key(&mut self, rq: &AddValidatorTempKeyRq) -> anyhow::Result<()> {
        self.do_rq::<AddValidatorTempKeyRqRs>(rq).await
    }

    async fn add_adnl_address(&mut self, rq: &AddAdnlAddressRq) -> anyhow::Result<()> {
        self.do_rq::<AddAdnlAddressRqRs>(rq).await
    }

    async fn add_validator_adnl_addr(&mut self, rq: &AddValidatorAdnlAddrRq) -> anyhow::Result<()> {
        self.do_rq::<AddValidatorAdnlAddrRqRs>(rq).await
    }

    async fn send_boc(&mut self, boc: &[u8]) -> anyhow::Result<()> {
        self.do_rq::<SendBocRqRs>(&boc.to_vec()).await
    }
}

#[async_trait::async_trait]
impl Shutdown for ControlClientAdnl {
    async fn shutdown(&mut self) -> anyhow::Result<()> {
        ControlClientAdnl::shutdown(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adnl::common::Timeouts;
    use std::time::{Duration, Instant};
    use ton_block::{Ed25519KeyOption, ZeroizingBytes};

    /// Verifies that connecting to an unreachable peer does not park the
    /// tokio worker thread, by running another async task concurrently on a
    /// single-worker runtime and asserting that the other task makes progress
    /// before the ADNL connect timeout elapses.
    ///
    /// With the legacy synchronous `AdnlClient::connect` this test would fail:
    /// the worker would be parked inside `socket2::Socket::connect_timeout`
    /// for the entire timeout window, the timer would not fire, and the probe
    /// would finish only after the connect attempt returned.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn connect_to_unreachable_peer_does_not_starve_runtime() {
        // RFC 5737 TEST-NET-1: guaranteed unreachable.
        let blackhole = "192.0.2.1:12345".parse().unwrap();
        let timeouts = Timeouts::with_duration(Duration::from_millis(800));
        let server_key = Ed25519KeyOption::<ZeroizingBytes>::generate().unwrap();
        let config = AdnlClientConfig::new(None, blackhole, server_key, timeouts);
        let mut client = ControlClientAdnl::new(config, 1);

        let start = Instant::now();

        let probe = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            Instant::now()
        };
        let try_connect = async {
            let res = client.connect().await;
            assert!(res.is_err(), "connect to blackhole must fail");
            Instant::now()
        };

        let (probe_done, connect_done) = tokio::join!(probe, try_connect);

        let probe_elapsed = probe_done.duration_since(start);
        let connect_elapsed = connect_done.duration_since(start);

        // If the environment returns a near-instant connect error (e.g. a CI
        // sandbox without a default route, giving EHOSTUNREACH), the slow
        // path is not exercised and the starvation check is meaningless.
        // Skip rather than fail in that case.
        if connect_elapsed < Duration::from_millis(100) {
            eprintln!(
                "skipping starvation check: connect to {} returned in {:?} \
                 (environment does not block the route)",
                blackhole, connect_elapsed,
            );
            return;
        }

        assert!(
            probe_elapsed < Duration::from_millis(500),
            "probe took {probe_elapsed:?}; runtime appears starved by ADNL connect"
        );
        assert!(
            connect_elapsed >= Duration::from_millis(700),
            "connect returned in {connect_elapsed:?}, expected ~800ms timeout"
        );
    }
}
