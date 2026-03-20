/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::client::{AdnlClient, AdnlClientConfig, AdnlClientConfigJson};
use node::config::TonNodeConfig;
use std::{
    convert::TryInto, env, fs::File, io::BufReader, net::SocketAddr, str::FromStr, time::Duration,
};
use tokio::io::AsyncReadExt;
use ton_api::{
    serialize_boxed,
    ton::{
        self,
        accountaddress::AccountAddress,
        engine::validator::{onestat::OneStat, ControlQueryError},
        raw::ShardAccountState,
        rpc::engine::validator::ControlQuery,
        Bool,
    },
    AnyBoxedSerialize, TLObject,
};
use ton_block::{
    base64_decode, base64_encode, ed25519_create_private_key, error, fail, read_single_root_boc,
    write_boc, AccountStatus, BlockIdExt, BuilderData, Deserializable, Ed25519KeyOption,
    ExternalInboundMessageHeader, IBitstring, Message, MsgAddressInt, Result, Serializable,
    ShardAccount, ShardIdent, SliceData, UInt256, UnixTime,
};
use ton_block_json::{parse_config, parse_state};

#[cfg(test)]
const SECRET_CONFIG_KEY: &str = "vXUACWlse0AB1GmVEMzZrm/eftCfRYATDp+OUAGve84=";
#[cfg(test)]
const PUBLIC_CONFIG_KEY: &str = "y+54dF9ZIQoIRQuH2+wE10fF5SV4GWrEv9m0lXHEmLU=";

trait SendReceive<Q> {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject>;
    fn receive(
        answer: TLObject,
        _params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        downcast::<ton_api::ton::engine::validator::Success>(answer)?;
        Ok(("success".to_string(), vec![]))
    }
}

/*
trait ConsoleCommand: SendReceive {
    fn _name() -> &'static str;
    fn help() -> &'static str;
}
*/

macro_rules! commands {
    ($($command: ident, $name: literal, $help: literal)*) => {
        $(
            struct $command;
        )*
        fn command_help(name: &str) -> Result<&str> {
            match name {
                $($name => Ok($help), )*
                _ => fail!("command {} not supported", name)
            }
        }
        fn command_send<Q: ToString>(
            name: &str,
            params: &mut impl Iterator<Item = Q>
        ) -> Result<TLObject> {
            match name {
                $($name => $command::send(params), )*
                _ => fail!("command {} not supported", name)
            }
        }
        fn command_receive<Q: ToString>(
            name: &str,
            answer: TLObject,
            params: &mut impl Iterator<Item = Q>
        ) -> Result<(String, Vec<u8>)> {
            match name {
                $($name => $command::receive(answer, params), )*
                _ => fail!("an error occured while receiving a response (command: {})", name)
            }
        }
    };
}

commands! {
    AddAdnlAddr, "addadnl",
        "addadnl <keyhash> <category>\tuse key as ADNL addr"
    AddValidatorAdnlAddr, "addvalidatoraddr",
        "addvalidatoraddr <permkeyhash> <keyhash> <expireat>\tadd validator ADNL addr"
    AddValidatorPermKey, "addpermkey",
        "addpermkey <keyhash> <election-date> <expire-at>\tadd validator permanent key"
    AddValidatorTempKey, "addtempkey",
        "addtempkey <permkeyhash> <keyhash> <expire-at>\tadd validator temp key"
    Bundle, "bundle",
        "bundle <block_id>\tprepare bundle"
    ExportPub, "exportpub",
        "exportpub <keyhash>\texports public key by key hash"
    FutureBundle, "future_bundle",
        "future_bundle <block_id>\tprepare future bundle"
    GetAccount, "getaccount",
        "getaccount <account id> <Option<file name>> <Option<block_id>>\tget account info"
    GetAccountState, "getaccountstate",
        "getaccountstate <account id> <file name>\tsave accountstate to the file"
    GetAccountByBlock, "getaccountstate_byblock",
        "getaccountstate_byblock <block root hash or masterchain block seq no> <account id> \
        <Option<file name>>\tsave masterchain accountstate from old block to the file"
    GetBlock, "getblock",
        "getblock <block id or root hash of block or masterchain seq no> <file name>\t\
         find block by masterchain seq_no or block id or root hash of block then save it to the file"
    GetBlockchainConfig, "getblockchainconfig",
        "getblockchainconfig\tget current config from masterchain state"
    GetConfigParam, "get_config_param",
        "getconfig <param_number>\tget current config param from masterchain state"
    GetSessionStats, "getconsensusstats",
        "getconsensusstats\tget consensus statistics for the node"
    GetSelectedStats, "getstatsnew",
        "getstatsnew\tget status full node or validator in new format"
    GetStats, "getstats",
        "getstats\tget status full node or validator"
    NewKeypair, "newkey",
        "newkey\tgenerates new key pair on server"
    ImportPrivateKey, "importprivatekey",
        "importprivatekey <keytype> <key>\timports private key on server"
    GetValidatorConfig, "get_validator_config",
        "get_validator_config\tget config from node or validator"
    SendMessage, "sendmessage",
        "sendmessage <filename>\tload a serialized message from <filename> and send it to server"
    Sign, "sign",
        "sign <keyhash> <data>\tsigns bytestring with privkey"
    AddCustomOverlay, "add_custom_overlay",
        "addcustomoverlay <config file name>\tadd custom overlay"
    DelCustomOverlay, "del_custom_overlay",
        "delcustomoverlay <custom overlay name>\tdelete custom overlay"
    ShowCustomOverlays, "show_custom_overlays",
        "showcustomoverlays\tshow custom overlays"
    // TODO: Simplex consensus commands (requires feature = "simplex")
    // GetSimplexStats, "getsimplexstats",
    //     "getsimplexstats\tget simplex consensus statistics"
}

fn parse_any<A, Q: ToString>(
    param_opt: Option<Q>,
    name: &str,
    parse_value: impl FnOnce(&str) -> Result<A>,
) -> Result<A> {
    let value = param_opt.ok_or_else(|| error!("insufficient parameters"))?.to_string();
    parse_value(value.trim_matches('\"')).map_err(|err| error!("you must give {}: {}", name, err))
}

fn downcast<T: ton_api::AnyBoxedSerialize>(data: TLObject) -> Result<T> {
    match data.downcast::<T>() {
        Ok(result) => Ok(result),
        Err(obj) => fail!("Wrong downcast {:?} to {}", obj, std::any::type_name::<T>()),
    }
}

fn parse_data<Q: ToString>(param_opt: Option<Q>, name: &str) -> Result<ton::bytes> {
    parse_any(param_opt, &format!("{} in hex format", name), |value| Ok(hex::decode(value)?))
}

fn parse_int256<Q: ToString>(param_opt: Option<Q>, name: &str) -> Result<UInt256> {
    parse_any(param_opt, &format!("{} in hex or base64 format", name), |value| {
        let value = match value.len() {
            44 => base64_decode(value)?,
            64 => hex::decode(value)?,
            length => fail!("wrong hash: {} with length: {}", value, length),
        };
        Ok(UInt256::with_array(value.as_slice().try_into()?))
    })
}

fn parse_int<Q: ToString>(param_opt: Option<Q>, name: &str) -> Result<ton::int> {
    parse_any(param_opt, name, |value| Ok(ton::int::from_str(value)?))
}

fn parse_blockid<Q: ToString>(param_opt: Option<Q>, name: &str) -> Result<BlockIdExt> {
    parse_any(param_opt, name, BlockIdExt::from_str)
}

fn stats_to_json<'a>(stats: impl IntoIterator<Item = &'a OneStat>) -> serde_json::Value {
    let map = stats
        .into_iter()
        .map(|stat| {
            let value = if stat.value.is_empty() {
                "null".into()
            } else if let Ok(value) = stat.value.parse::<i64>() {
                value.into()
            } else if let Ok(value) = serde_json::from_str(&stat.value) {
                value
            } else {
                stat.value.trim_matches('\"').into()
            };
            (stat.key.clone(), value)
        })
        .collect::<serde_json::Map<_, _>>();
    map.into()
}

impl<Q: ToString> SendReceive<Q> for GetStats {
    fn send(_params: &mut impl Iterator) -> Result<TLObject> {
        Ok(ton::rpc::engine::validator::GetStats.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let data = serialize_boxed(&answer)?;
        let stats = downcast::<ton_api::ton::engine::validator::Stats>(answer)?;
        let description = stats_to_json(stats.stats().iter());
        let description = format!("{:#}", description);
        Ok((description, data))
    }
}

impl<Q: ToString> SendReceive<Q> for GetSelectedStats {
    fn send(_params: &mut impl Iterator) -> Result<TLObject> {
        let req = ton::rpc::engine::validator::GetSelectedStats { filter: "*".to_string() };
        Ok(req.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let data = serialize_boxed(&answer)?;
        let stats = downcast::<ton_api::ton::engine::validator::Stats>(answer)?;
        let description = stats_to_json(stats.stats().iter());
        let description = format!("{:#}", description);
        Ok((description, data))
    }
}

impl<Q: ToString> SendReceive<Q> for GetSessionStats {
    fn send(_params: &mut impl Iterator) -> Result<TLObject> {
        Ok(ton::rpc::engine::validator::GetSessionStats.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let data = serialize_boxed(&answer)?;
        let stats = downcast::<ton_api::ton::engine::validator::SessionStats>(answer)?;
        let description = stats
            .stats()
            .iter()
            .map(|session_stat| {
                (session_stat.session_id.clone(), stats_to_json(session_stat.stats.iter()))
            })
            .collect::<serde_json::Map<_, _>>();
        let description = format!("{:#}", serde_json::Value::from(description));
        Ok((description, data))
    }
}

impl<Q: ToString> SendReceive<Q> for NewKeypair {
    fn send(_params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        Ok(ton::rpc::engine::validator::GenerateKeyPair.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let answer = downcast::<ton_api::ton::engine::validator::KeyHash>(answer)?;
        let key_hash = answer.key_hash().as_slice().to_vec();
        let msg = format!(
            "received public key hash: {} {}",
            hex::encode(&key_hash),
            base64_encode(&key_hash)
        );
        Ok((msg, key_hash))
    }
}

impl<Q: ToString> SendReceive<Q> for ImportPrivateKey {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let key_type = params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string();
        let pvt_key = parse_int256(params.next(), "key")?;

        if key_type != "ed25519" {
            fail!("Unsupported key type: {}", key_type);
        }

        Ok(ton::rpc::engine::validator::ImportPrivateKey {
            key: ton_api::ton::PrivateKey::Pk_Ed25519(ton::pk::privatekey::Ed25519 {
                key: pvt_key,
            }),
        }
        .into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let answer = downcast::<ton_api::ton::engine::validator::KeyHash>(answer)?;
        let key_hash = answer.key_hash().as_slice().to_vec();
        let msg = format!(
            "received public key hash: {} {}",
            hex::encode(&key_hash),
            base64_encode(&key_hash)
        );
        Ok((msg, key_hash))
    }
}

impl<Q: ToString> SendReceive<Q> for GetValidatorConfig {
    fn send(_params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        Ok(ton::rpc::engine::validator::GetConfig.into_tl_object())
    }
    fn receive(
        answer: TLObject,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let json_name =
            params.next().ok_or_else(|| error!("insufficient output json filename"))?.to_string();
        let answer = downcast::<ton_api::ton::engine::validator::JsonConfig>(answer)?;
        std::fs::write(&json_name, answer.data())
            .map_err(|err| error!("cannot write config to file {json_name}: {err}"))?;
        Ok((answer.only().data, Vec::new()))
    }
}

impl<Q: ToString> SendReceive<Q> for ExportPub {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let ret = ton::rpc::engine::validator::ExportPublicKey {
            key_hash: parse_int256(params.next(), "key_hash")?,
        };
        Ok(ret.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let answer = downcast::<ton_api::ton::PublicKey>(answer)?;
        let pub_key = match answer.key() {
            Some(key) => key.clone().into_vec(),
            None => fail!("Public key not found in answer!"),
        };
        let msg = format!("imported key: {} {}", hex::encode(&pub_key), base64_encode(&pub_key));
        Ok((msg, pub_key))
    }
}

impl<Q: ToString> SendReceive<Q> for Sign {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let key_hash = parse_int256(params.next(), "key_hash")?;
        let data = parse_data(params.next(), "data")?;
        let ret = ton::rpc::engine::validator::Sign { key_hash, data };
        Ok(ret.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let answer = downcast::<ton_api::ton::engine::validator::Signature>(answer)?;
        let signature = answer.signature().clone();
        let msg =
            format!("got signature: {} {}", hex::encode(&signature), base64_encode(&signature));
        Ok((msg, signature))
    }
}

impl<Q: ToString> SendReceive<Q> for AddValidatorPermKey {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let key_hash = parse_int256(params.next(), "key_hash")?;
        let election_date = parse_int(params.next(), "election_date")?;
        let ttl = parse_int(params.next(), "expire_at")? - election_date;
        let ret =
            ton::rpc::engine::validator::AddValidatorPermanentKey { key_hash, election_date, ttl };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for AddValidatorTempKey {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let permanent_key_hash = parse_int256(params.next(), "permanent_key_hash")?;
        let key_hash = parse_int256(params.next(), "key_hash")?;
        let ttl = parse_int(params.next(), "expire_at")? - UnixTime::now() as i32;
        let ret =
            ton::rpc::engine::validator::AddValidatorTempKey { permanent_key_hash, key_hash, ttl };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for AddValidatorAdnlAddr {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let permanent_key_hash = parse_int256(params.next(), "permanent_key_hash")?;
        let key_hash = parse_int256(params.next(), "key_hash")?;
        let ttl = parse_int(params.next(), "expire_at")? - UnixTime::now() as i32;
        let ret = ton::rpc::engine::validator::AddValidatorAdnlAddress {
            permanent_key_hash,
            key_hash,
            ttl,
        };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for AddAdnlAddr {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let key_hash = parse_int256(params.next(), "key_hash")?;
        let category = parse_int(params.next(), "category")?;
        if !(0..=15).contains(&category) {
            fail!("category must be not negative and less than 16")
        }
        let ret = ton::rpc::engine::validator::AddAdnlId { key_hash, category };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for Bundle {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let ret = ton::rpc::engine::validator::GetBundle {
            block_id: parse_blockid(params.next(), "block_id")?,
        };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for FutureBundle {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let mut prev_block_ids = vec![parse_blockid(params.next(), "block_id")?];
        if let Ok(block_id) = parse_blockid(params.next(), "block_id") {
            prev_block_ids.push(block_id);
        }
        let ret = ton::rpc::engine::validator::GetFutureBundle { prev_block_ids };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for SendMessage {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let filename = params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string();
        let body = std::fs::read(&filename)
            .map_err(|e| error!("Can't read file {} with message: {}", filename, e))?;
        let ret = ton::rpc::lite_server::SendMessage { body };
        Ok(ret.into_tl_object())
    }
}

struct SendMessageBinary;
impl<Q: AsRef<Vec<u8>>> SendReceive<Q> for SendMessageBinary {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let mut body = Vec::new();
        body.extend_from_slice(
            &params.next().ok_or_else(|| error!("No binary message body"))?.as_ref()[..],
        );
        let ret = ton::rpc::lite_server::SendMessage { body };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for GetBlock {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let id = params.next().ok_or_else(|| error!("you need to set block_id"))?.to_string();
        let id = if let Ok(seq_no) = id.parse() {
            BlockIdExt { shard_id: ShardIdent::masterchain(), seq_no, ..Default::default() }
        } else {
            id.parse().map_err(|err| error!("Can`t parse block_id {}: {}", id, err))?
        };
        let ret = ton::rpc::lite_server::GetBlock { id };
        Ok(ret.into_tl_object())
    }

    fn receive(
        answer: TLObject,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let data = downcast::<ton::lite_server::BlockData>(answer)?;

        if data.data().is_empty() {
            fail!("block not found!")
        }

        // skip block_id
        params.next();

        let boc_name = params.next().map_or_else(
            || format!("block_boc_{:x}.boc", data.id().root_hash()),
            |s| s.to_string(),
        );

        std::fs::write(&boc_name, data.data())
            .map_err(|err| error!("Can`t create file {}: {}", boc_name, err))?;

        Ok((
            format!("{} {}", hex::encode(data.data()), base64_encode(data.data())),
            data.only().data,
        ))
    }
}

impl<Q: ToString> SendReceive<Q> for GetBlockchainConfig {
    fn send(_params: &mut impl Iterator) -> Result<TLObject> {
        Ok(ton::rpc::lite_server::GetConfigAll::default().into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let config_info = downcast::<ton_api::ton::lite_server::ConfigInfo>(answer)?;

        // We use config_proof because we use standard struct ConfigInfo from ton-tl and
        // ConfigInfo doesn`t contain more suitable fields
        let config_param = hex::encode(config_info.config_proof().clone());
        Ok((config_param.to_string(), config_info.config_proof().clone()))
    }
}

impl<Q: ToString> SendReceive<Q> for GetConfigParam {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let param_number = parse_int(params.next(), "paramnumber")?;
        let param_list = vec![param_number];
        let ret = ton::rpc::lite_server::GetConfigParams {
            mode: 0,
            id: BlockIdExt::default(),
            param_list,
        };
        Ok(ret.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let config_info = downcast::<ton_api::ton::lite_server::ConfigInfo>(answer)?;
        let config_proof = config_info.only().config_proof;
        let config_param = String::from_utf8(config_proof.clone())?;
        Ok((config_param, config_proof))
    }
}

fn account_address<Q: ToString>(params: &mut impl Iterator<Item = Q>) -> Result<AccountAddress> {
    let account_address =
        params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string();
    Ok(AccountAddress { account_address })
}

fn receive_and_save_shard_account_state<Q: ToString>(
    answer: TLObject,
    params: &mut impl Iterator<Item = Q>,
) -> Result<(String, Vec<u8>)> {
    let shard_account_state = downcast::<ShardAccountState>(answer)?;

    params.next(); // account address
    let boc_name =
        params.next().ok_or_else(|| error!("bad params (boc name not found)!"))?.to_string();

    let shard_account_state =
        shard_account_state.shard_account().ok_or_else(|| error!("account not found!"))?;

    let shard_account = ShardAccount::construct_from_bytes(shard_account_state)?;
    let account_state = write_boc(&shard_account.account_cell())?;
    std::fs::write(&boc_name, &account_state)
        .map_err(|err| error!("Can`t create file {boc_name}: {err}"))?;

    Ok((base64_encode(&account_state), account_state))
}

impl<Q: ToString> SendReceive<Q> for GetAccount {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let ret = ton::rpc::raw::GetShardAccountState { account_address: account_address(params)? };
        Ok(ret.into_tl_object())
    }

    fn receive(
        answer: TLObject,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let account_info = match downcast::<ShardAccountState>(answer)? {
            ShardAccountState::Raw_ShardAccountNone => {
                serde_json::json!({
                    "acc_type": "Nonexist"
                })
            }
            ShardAccountState::Raw_ShardAccountState(account_state) => {
                let shard_account =
                    ShardAccount::construct_from_bytes(&account_state.shard_account)?;
                let account = shard_account.read_account()?;

                let account_type = match account.status() {
                    AccountStatus::AccStateUninit => "Uninit",
                    AccountStatus::AccStateFrozen => "Frozen",
                    AccountStatus::AccStateActive => "Active",
                    AccountStatus::AccStateNonexist => "Nonexist",
                };
                serde_json::json!({
                    "acc_type": account_type,
                    "balance": account.balance().map_or(0, |val| val.coins.as_u128()),
                    "last_paid": account.last_paid(),
                    "last_trans": format!("{:#x}", shard_account.last_trans_lt()),
                    "data(boc)": hex::encode(write_boc(&shard_account.account_cell())?)
                })
            }
        };

        params.next();
        let account_info = serde_json::to_string_pretty(&account_info)?;
        let account_data = account_info.as_bytes().to_vec();
        if let Some(boc_name) = params.next() {
            let boc_name = boc_name.to_string();
            std::fs::write(&boc_name, &account_info)
                .map_err(|err| error!("Can`t create file {}: {}", boc_name, err))?;
        }

        Ok((account_info, account_data))
    }
}

impl<Q: ToString> SendReceive<Q> for GetAccountState {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let ret = ton::rpc::raw::GetShardAccountState { account_address: account_address(params)? };
        Ok(ret.into_tl_object())
    }

    fn receive(
        answer: TLObject,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        receive_and_save_shard_account_state(answer, params)
    }
}

impl<Q: ToString> SendReceive<Q> for GetAccountByBlock {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let block_root_hash =
            params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string().parse()?;
        let account_id =
            params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string().parse()?;
        let ret = ton::rpc::raw::GetAccountByBlock { block_root_hash, account_id };
        Ok(ret.into_tl_object())
    }

    fn receive(
        answer: TLObject,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        params.next(); // block_id
        receive_and_save_shard_account_state(answer, params)
    }
}

impl<Q: ToString> SendReceive<Q> for AddCustomOverlay {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let config_file_name =
            params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string();
        let file = File::open(config_file_name)?;
        let reader = BufReader::new(file);
        let json_value: serde_json::Value = serde_json::from_reader(reader)?;
        let overlay = TonNodeConfig::parse_custom_overlay_json(&json_value)?;
        let ret = ton::rpc::engine::validator::AddCustomOverlay { overlay };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for DelCustomOverlay {
    fn send(params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let name = params.next().ok_or_else(|| error!("insufficient parameters"))?.to_string();
        let ret = ton::rpc::engine::validator::DelCustomOverlay { name };
        Ok(ret.into_tl_object())
    }
}

impl<Q: ToString> SendReceive<Q> for ShowCustomOverlays {
    fn send(_params: &mut impl Iterator<Item = Q>) -> Result<TLObject> {
        let ret = ton::rpc::engine::validator::ShowCustomOverlays;
        Ok(ret.into_tl_object())
    }
    fn receive(answer: TLObject, _params: &mut impl Iterator) -> Result<(String, Vec<u8>)> {
        let Ok(overlays) =
            downcast::<ton_api::ton::engine::validator::CustomOverlaysConfig>(answer)
        else {
            return Ok(("received incorrect answer".to_string(), vec![]));
        };
        // Description is same as the cpp node
        let mut description = format!("{} custom overlays:\n\n", overlays.overlays().len());
        for overlay in overlays.overlays() {
            description.push_str(&format!(
                "Overlay \"{}\": {} nodes\n",
                overlay.name,
                overlay.nodes.len()
            ));
            for node in &overlay.nodes {
                description.push_str(&format!("  {}", base64_encode(node.adnl_id.as_slice())));
                if matches!(node.msg_sender, Bool::BoolTrue) {
                    description.push_str(&format!(" (msg sender, p={})", node.msg_sender_priority));
                }
                if matches!(node.block_sender, Bool::BoolTrue) {
                    description.push_str(" (block sender)");
                }
                description.push_str("\n");
            }
            description.push_str(" Sender shards:\n");
            for shard in &overlay.sender_shards {
                description
                    .push_str(&format!("  ({},{:016x})\n", shard.workchain, shard.shard as u64));
            }
            if matches!(overlay.skip_public_msg_send, Bool::BoolTrue) {
                description.push_str("Don't send external messages to public overlays\n");
            }
            description.push_str("\n");
        }
        let data = serialize_boxed(&overlays)?;
        Ok((description, data))
    }
}

/// ControlClient
struct ControlClient {
    config: AdnlConsoleConfigJson,
    adnl: AdnlClient,
}

impl ControlClient {
    /// Connect to server
    async fn connect(mut config: AdnlConsoleConfigJson) -> Result<Self> {
        let client_config =
            config.config.take().ok_or_else(|| error!("config must contain \"config\" section"))?;
        let (_, adnl_config) = AdnlClientConfig::from_json_config(&client_config)?;
        Ok(Self { config, adnl: AdnlClient::connect(&adnl_config).await? })
    }

    /// Shutdown client
    async fn shutdown(self) -> Result<()> {
        self.adnl.shutdown().await
    }

    async fn command(&mut self, cmd: &str) -> Result<(String, Vec<u8>)> {
        let result = shell_words::split(cmd)?;
        let mut params = result.iter();
        match params.next().expect("takes_value set for COMMANDS").as_str() {
            "config_param" | "cparam" => self.process_config_param(&mut params).await,
            "set_config_param" | "setcparam" => self.process_set_config_param(&mut params).await,
            "ebid" | "election-bid" | "election_bid" => {
                self.process_election_bid(&mut params).await
            }
            "ext_msg_proxy" => self.process_ext_msg_proxy(&mut params).await,
            "recover_stake" => self.process_recover_stake(&mut params).await,
            name => self.process_command(name, &mut params).await,
        }
    }

    async fn process_any_type_command(
        &mut self,
        name: &str,
        prepare: impl FnOnce(&str) -> Result<TLObject>,
        process: impl FnOnce(&str, TLObject) -> Result<(String, Vec<u8>)>,
    ) -> Result<(String, Vec<u8>)> {
        let query = match prepare(name) {
            Ok(query) => query,
            Err(err) => {
                let help =
                    command_help(name).map_or_else(|err| err.to_string(), |help| help.to_string());
                println!("{}\n{}", err, help);
                return Err(err);
            }
        };
        let boxed = ControlQuery { data: serialize_boxed(&query)? }.into_tl_object().into();
        let answer = self
            .adnl
            .query(&boxed)
            .await
            .map_err(|err| error!("Error receiving answer: {}", err))?;
        match answer.downcast::<ControlQueryError>() {
            Err(answer) => match process(name, answer) {
                Err(answer) => fail!("Wrong response to {:?}: {:?}", query, answer),
                Ok(result) => Ok(result),
            },
            Ok(error) => fail!("Error response to {:?}: {:?}", query, error),
        }
    }

    async fn process_binary_command<Q: AsRef<Vec<u8>>>(
        &mut self,
        name: &str,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        if name != "sendmessage_binary" {
            fail!("Wrong command name {}", name)
        }
        self.process_any_type_command(
            name,
            |_| SendMessageBinary::send(params),
            |_, answer| SendMessageBinary::receive(answer, &mut Vec::<Vec<u8>>::new().iter()),
        )
        .await
    }

    async fn process_command<Q: ToString>(
        &mut self,
        name: &str,
        params: &mut (impl Iterator<Item = Q> + Clone),
    ) -> Result<(String, Vec<u8>)> {
        let mut params_clone = params.clone();
        self.process_any_type_command(
            name,
            |name| command_send(name, params),
            |name, answer| command_receive(name, answer, &mut params_clone),
        )
        .await
    }

    async fn process_recover_stake<Q: ToString>(
        &mut self,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let query_id = UnixTime::now() as u64;
        // recover-stake.fif
        let mut data = 0x47657424u32.to_be_bytes().to_vec();
        data.extend_from_slice(&query_id.to_be_bytes());
        let len = data.len() * 8;
        let body = BuilderData::with_raw(data, len)?;
        let body = body.into_cell()?;
        log::trace!("message body {}", body);
        let data = write_boc(&body)?;
        let path =
            params.next().map(|path| path.to_string()).unwrap_or("recover-query.boc".to_string());
        std::fs::write(&path, &data)?;
        Ok((format!("Message body is {} saved to path {}", base64_encode(&data), path), data))
    }

    // @input elect_time expire_time <validator-query.boc>
    // @output validator-query.boc
    async fn process_election_bid<Q: ToString>(
        &mut self,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let wallet_id =
            parse_any(self.config.wallet_id.as_ref(), "wallet_id", |value| {
                match value.strip_prefix("-1:") {
                    Some(stripped) => Ok(hex::decode(stripped)?),
                    None => fail!("use masterchain wallet"),
                }
            })?;
        let elect_time = parse_int(params.next(), "elect_time")?;
        if elect_time <= 0 {
            fail!("<elect-utime> must be a positive integer")
        }
        let elect_time_str = elect_time.to_string();
        let expire_time = parse_int(params.next(), "expire_time")?;
        if expire_time <= elect_time {
            fail!("<expire-utime> must be a grater than elect_time")
        }
        let expire_time_str = expire_time.to_string();
        let max_factor =
            self.config.max_factor.ok_or_else(|| error!("you must give max_factor as real"))?;
        if !(1.0..=100.0).contains(&max_factor) {
            fail!("<max-factor> must be a real number 1..100")
        }
        let max_factor = (max_factor * 65536.0) as u32;

        let (s, perm) = self.process_command("newkey", &mut Vec::<String>::new().iter()).await?;
        log::trace!("{}", s);
        let perm_str = hex::encode_upper(&perm);

        let (s, pub_key) = self.process_command("exportpub", &mut [&perm_str].iter()).await?;
        log::trace!("{}", s);

        let (s, _) = self
            .process_command(
                "addpermkey",
                &mut [&perm_str, &elect_time_str, &expire_time_str].iter(),
            )
            .await?;
        log::trace!("{}", s);

        let (s, _) = self
            .process_command("addtempkey", &mut [&perm_str, &perm_str, &expire_time_str].iter())
            .await?;
        log::trace!("{}", s);

        let (s, adnl) = self.process_command("newkey", &mut Vec::<String>::new().iter()).await?;
        log::trace!("{}", s);
        let adnl_str = hex::encode_upper(&adnl);

        let (s, _) = self.process_command("addadnl", &mut [&adnl_str, "0"].iter()).await?;
        log::trace!("{}", s);

        let (s, _) = self
            .process_command(
                "addvalidatoraddr",
                &mut [&perm_str, &adnl_str, &elect_time_str].iter(),
            )
            .await?;
        log::trace!("{}", s);

        // validator-elect-req.fif
        let mut data = 0x654C5074u32.to_be_bytes().to_vec();
        data.extend_from_slice(&elect_time.to_be_bytes());
        data.extend_from_slice(&max_factor.to_be_bytes());
        data.extend_from_slice(&wallet_id);
        data.extend_from_slice(&adnl);
        let data_str = hex::encode_upper(&data);
        log::trace!("data to sign {}", data_str);
        let (s, signature) =
            self.process_command("sign", &mut [&perm_str, &data_str].iter()).await?;
        log::trace!("{}", s);
        Ed25519KeyOption::from_public_key(&pub_key[..].try_into()?).verify(&data, &signature)?;

        let query_id = UnixTime::now() as u64;
        // validator-elect-signed.fif
        let mut data = 0x4E73744Bu32.to_be_bytes().to_vec();
        data.extend_from_slice(&query_id.to_be_bytes());
        data.extend_from_slice(&pub_key);
        data.extend_from_slice(&elect_time.to_be_bytes());
        data.extend_from_slice(&max_factor.to_be_bytes());
        data.extend_from_slice(&adnl);
        let len = data.len() * 8;
        let mut body = BuilderData::with_raw(data, len)?;
        let len = signature.len() * 8;
        body.checked_append_reference(BuilderData::with_raw(signature, len)?.into_cell()?)?;
        let body = body.into_cell()?;

        log::trace!("message body {}", body);
        let data = write_boc(&body)?;
        let path =
            params.next().map(|path| path.to_string()).unwrap_or("validator-query.boc".to_string());

        std::fs::write(&path, &data)?;
        Ok((format!("Message body is {} saved to path {}", base64_encode(&data), path), data))
    }

    // @input <tcp-port-to-listen-to>
    // @output <number-of-proxied-messages>
    async fn process_ext_msg_proxy<Q: ToString>(
        &mut self,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        async fn read_stream(
            stream: &mut tokio::net::TcpStream,
            token: &tokio_util::sync::CancellationToken,
        ) -> Result<Vec<u8>> {
            let mut buf = [0u8; 2];
            tokio::select! {
                len = stream.read_exact(&mut buf) => len?,
                _ = token.cancelled() => fail!("Proxy cancelled")
            };
            let len = ((buf[0] as usize) << 8) | (buf[1] as usize);
            let mut buf = vec![0u8; len];
            tokio::select! {
                len = stream.read_exact(&mut buf) => len?,
                _ = token.cancelled() => fail!("Proxy cancelled")
            };
            Ok(buf)
        }

        let Some(port) = params.next() else { fail!("No TCP port specified") };
        let token = tokio_util::sync::CancellationToken::new();
        let addr = SocketAddr::new([0, 0, 0, 0].into(), port.to_string().parse()?);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let token_clone = token.clone();

        tokio::spawn(async move {
            loop {
                let mut stream = tokio::select! {
                    x = listener.accept() => match x {
                        Err(e) => {
                            log::warn!("Error in proxy listener: {}", e);
                            break
                        }
                        Ok((stream, _)) => stream
                    },
                    _ = token_clone.cancelled() => break
                };
                let token = token_clone.clone();
                let sender = sender.clone();
                tokio::spawn(async move {
                    loop {
                        let msg = match read_stream(&mut stream, &token).await {
                            Err(e) => {
                                log::warn!("Proxy error: {}", e);
                                break;
                            }
                            Ok(msg) => msg,
                        };
                        if &msg[..] == b"close_proxy" {
                            log::warn!("Close proxy signal received");
                            token.cancel();
                            break;
                        }
                        if let Err(e) = sender.send(msg) {
                            log::warn!("Proxy error: {}", e);
                            break;
                        }
                    }
                });
            }
        });

        let mut count: u32 = 0;
        loop {
            let res = tokio::select! {
                x = receiver.recv() => match x {
                    None => break,
                    Some(msg) => self.process_binary_command(
                        "sendmessage_binary",
                         &mut [msg].iter()
                    ).await
                },
                _ = token.cancelled() => break
            };
            if let Err(e) = res {
                log::warn!("External message proxy error: {}", e);
            }
            count += 1;
        }

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&count.to_be_bytes());
        Ok((format!("{} external messages proxied", count), bytes))
    }

    // @input index zerostate.json <config-param.boc>
    // @output config-param.boc
    async fn process_config_param<Q: ToString>(
        &mut self,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let index = parse_int(params.next(), "index")?;
        if index < 0 {
            fail!("<index> must not be a negative integer")
        }
        let zerostate = parse_any(params.next(), "zerostate.json", |value| Ok(value.to_string()))?;
        let path =
            params.next().map(|path| path.to_string()).unwrap_or("config-param.boc".to_string());

        let zerostate = std::fs::read_to_string(&zerostate)
            .map_err(|err| error!("Can't read zerostate json file {} : {}", zerostate, err))?;
        let zerostate =
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&zerostate)
                .map_err(|err| error!("Can't parse read zerostate json file: {}", err))?;
        let zerostate = parse_state(&zerostate)
            .map_err(|err| error!("Can't parse read zerostate json file: {}", err))?;

        let key = SliceData::load_builder(index.write_to_new_cell()?)?;
        let config_param_cell = zerostate
            .read_custom()
            .map_err(|err| error!("Can't read McStateExtra from zerostate: {}", err))?
            .ok_or_else(|| error!("Can't find McStateExtra in zerostate"))?
            .config()
            .config_params
            .get(key)
            .map_err(|err| error!("Can't read config param {} from zerostate: {}", index, err))?
            .ok_or_else(|| error!("Can't find config param {} in zerostate", index))?
            .reference_opt(0)
            .ok_or_else(|| {
                error!("Can't parse config param {}: wrong format - no reference", index)
            })?;

        let data = write_boc(&config_param_cell)
            .map_err(|err| error!("Can't serialize config param {}: {}", index, err))?;

        std::fs::write(&path, &data).map_err(|err| {
            error!("Can't write config param {} to file {}: {}", index, path, err)
        })?;

        Ok((format!("Config param {} saved to path {}", index, path), data))
    }

    // @input <config-param-json-path> <key-bin-path> <output-message-boc-path>
    // @output base64_boc and boc
    // prepare message to set config param valid for 30 seconds
    async fn process_set_config_param<Q: ToString>(
        &mut self,
        params: &mut impl Iterator<Item = Q>,
    ) -> Result<(String, Vec<u8>)> {
        let config = parse_any(params.next(), "config.json", |path| Ok(path.to_string()))?;
        let config = std::fs::read_to_string(&config)
            .map_err(|err| error!("Can't read config file {config}: {err}"))?;
        let config = serde_json::from_str(&config)
            .map_err(|err| error!("Can't parse json config file: {err}"))?;
        let config = parse_config(&config)
            .map_err(|err| error!("Can't parse config param from json file: {err}"))?;
        let (config_cell, config_index) = config
            .serialize_single_param()
            .map_err(|err| error!("Can't serialize config param: {err}"))?;

        let secret_key = parse_any(params.next(), "secret_key.bin", |path| Ok(path.to_string()))?;
        let secret_key = std::fs::read(&secret_key)
            .map_err(|err| error!("Can't read secret key file {secret_key}: {err}"))?;
        let secret_key = ed25519_create_private_key(&secret_key).map_err(|err| {
            error!("Can't parse secret key file {}: {err}", base64_encode(&secret_key))
        })?;

        let output_path = parse_any(params.next(), "output.bin", |path| Ok(path.to_string()))?;

        let params = [
            "-1:5555555555555555555555555555555555555555555555555555555555555555".to_string(),
            output_path.clone(),
        ];
        let (_, config_boc) = self
            .process_command("getaccountstate", &mut params.into_iter())
            .await
            .map_err(|err| error!("Can't get config account state: {err}"))?;
        let config_smc_cell = read_single_root_boc(&config_boc)
            .map_err(|err| error!("Can't parse config account state boc: {err}"))?;
        let data = config_smc_cell
            .reference(1)
            .map_err(|err| error!("Can't get config account data cell: {err}"))?;
        let mut slice = SliceData::load_cell(data)
            .map_err(|err| error!("Can't load config account data cell: {err}"))?;
        let seqno = slice.get_next_u32()?;
        // println!("seqno: {seqno}");
        let public_key = slice.get_next_u256()?;
        if public_key != secret_key.verifying_key() {
            fail!("public key from config account doesn't match secret key")
        }
        let valid_until = UnixTime::now() as u32 + 30; // valid for 30 seconds

        let mut builder = BuilderData::new();
        builder.append_u32(0x43665021)?; // change one configuration parameter
        builder.append_u32(seqno)?;
        builder.append_u32(valid_until)?;
        builder.append_u32(config_index)?;
        builder.checked_append_reference(config_cell)?;

        assert_eq!(builder.bits_used(), 32 + 32 + 32 + 32);
        assert_eq!(builder.references_used(), 1);

        let data = builder.clone().into_cell()?.repr_hash();
        let signature = secret_key.sign(data.as_slice());
        let mut body = BuilderData::with_bytes(&signature)?;
        body.append_builder(&builder)?;

        assert_eq!(body.bits_used(), 512 + 32 + 32 + 32 + 32);
        assert_eq!(body.references_used(), 1);

        let body = body.into_cell()?;

        let h = ExternalInboundMessageHeader::new(
            Default::default(),
            MsgAddressInt::standard(-1, [0x55; 32]),
        );
        let msg = Message::with_ext_in_header_and_body(h, SliceData::load_cell(body)?);
        let msg_cell = msg.serialize()?;
        println!("msg hash: {:x}", msg_cell.repr_hash());
        let msg_data = write_boc(&msg_cell)?;
        std::fs::write(&output_path, &msg_data)?;

        // let mut config_smc = ton_block::Account::construct_from_cell(config_smc_cell)
        //     .map_err(|err| error!("Can't construct config account from cell: {err}"))?;
        // let block_lt = config_smc.last_tr_time().unwrap() + 1;
        // let params = ton_executor::ExecuteParams {
        //     block_unixtime: UnixTime::now() as u32,
        //     block_lt,
        //     last_tr_lt: block_lt + 1,
        //     debug: true,
        //     ..Default::default()
        // };
        // let config = config_smc.get_config()?;
        // let config = ton_executor::BlockchainConfig::with_config(config)?;
        // let executor = ton_executor::OrdinaryTransactionExecutor::new(config);
        // let in_msg_cell = msg.serialize()?;
        // let _trans = ton_executor::TransactionExecutor::execute_with_params(
        //     &executor,
        //     Some(in_msg_cell),
        //     &mut config_smc,
        //     params,
        // )?;
        // let mut slice = SliceData::load_cell_ref(config_smc.data().unwrap())?;
        // let new_seqno = slice.get_next_u32()?;
        // assert_eq!(new_seqno, seqno + 1);
        // println!("transaction checked, new seqno {new_seqno}");

        Ok((base64_encode(&msg_data), msg_data))
    }
}

#[derive(serde::Deserialize)]
struct AdnlConsoleConfigJson {
    config: Option<AdnlClientConfigJson>,
    wallet_id: Option<String>,
    max_factor: Option<f32>,
}

#[tokio::main]
async fn main() {
    // init_test_log();
    let args = clap::Command::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            clap::Arg::new("CONFIG")
                .short('C')
                .long("config")
                .help("config for console")
                .required(true)
                .default_value("console.json")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("COMMANDS")
                .allow_hyphen_values(true)
                .short('c')
                .long("cmd")
                .help("schedule command")
                .action(clap::ArgAction::Append)
                .num_args(1..),
        )
        .arg(
            clap::Arg::new("TIMEOUT")
                .short('t')
                .long("timeout")
                .help("timeout in batch mode")
                .num_args(1),
        )
        .arg(
            clap::Arg::new("VERBOSE")
                .long("verbose")
                .help("verbose regim")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            clap::Arg::new("JSON")
                .short('j')
                .long("json")
                .help("output in json format")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    if !args.get_flag("JSON") {
        println!(
            "TON Rust node console {}\nCOMMIT_ID: {}\nBUILD_DATE: {}\nCOMMIT_DATE: {}\nGIT_BRANCH: {}",
            env!("CARGO_PKG_VERSION"),
            env!("BUILD_GIT_COMMIT"),
            env!("BUILD_TIME"),
            env!("BUILD_GIT_DATE"),
            env!("BUILD_GIT_BRANCH")
        );
    }

    if args.get_flag("VERBOSE") {
        let encoder_boxed = Box::new(log4rs::encode::pattern::PatternEncoder::new("{m}{n}"));
        let console =
            log4rs::append::console::ConsoleAppender::builder().encoder(encoder_boxed).build();
        let config = log4rs::config::Config::builder()
            .appender(log4rs::config::Appender::builder().build("console", Box::new(console)))
            .build(
                log4rs::config::Root::builder().appender("console").build(log::LevelFilter::Trace),
            )
            .unwrap();
        log4rs::init_config(config).unwrap();
    }

    let config = args.get_one::<String>("CONFIG").expect("required set for config");
    let config = std::fs::read_to_string(config)
        .unwrap_or_else(|_| panic!("Can't read config file {}", config));
    let config = serde_json::from_str(&config).expect("Can't parse config");
    let timeout = match args.get_one::<String>("TIMEOUT") {
        Some(timeout) => u64::from_str(timeout).expect("timeout must be set in microseconds"),
        None => 0,
    };
    let timeout = Duration::from_micros(timeout);
    let mut client = ControlClient::connect(config).await.expect("Can't create client");
    if let Some(commands) = args.get_many::<String>("COMMANDS") {
        // batch mode - call commands and exit
        for command in commands {
            match client.command(command.trim_matches('\"')).await {
                Ok((result, _)) => println!("{}", result),
                Err(err) => panic!("Error executing command: {}", err),
            }
            tokio::time::sleep(timeout).await;
        }
    } else {
        // interactive mode
        loop {
            let mut line = String::default();
            std::io::stdin().read_line(&mut line).expect("Can't read line from stdin");
            match line.trim_end() {
                "" => continue,
                "quit" => break,
                command => match client.command(command).await {
                    Ok((result, _)) => println!("{}", result),
                    Err(err) => println!("{}", err),
                },
            }
        }
    }
    client.shutdown().await.ok();
}

#[cfg(test)]
mod test {

    use super::*;
    use node::{
        collator_test_bundle::create_engine_allocated,
        config::TonNodeConfig,
        engine_traits::{EngineAlloc, EngineOperations},
        internal_db::{state_gc_resolver::AllowStateGcSmartResolver, InternalDb, InternalDbConfig},
        network::{
            control::{ControlServer, DataSource},
            node_network::NodeNetwork,
        },
        shard_state::ShardStateStuff,
        shard_states_keeper::PinnedShardStateGuard,
        validator::validator_manager::ValidationStatus,
    };
    #[cfg(feature = "telemetry")]
    use node::{collator_test_bundle::create_engine_telemetry, engine_traits::EngineTelemetry};
    use rand::{Rng, SeedableRng};
    use std::{
        cmp::min,
        collections::HashMap,
        fs,
        path::Path,
        sync::{
            atomic::{AtomicU16, AtomicU64, AtomicU8, Ordering},
            Arc,
        },
        thread,
        time::{Duration, Instant},
    };
    use storage::block_handle_db::BlockHandle;
    use tokio::io::AsyncWriteExt;
    use ton_api::deserialize_boxed;
    use ton_block::{
        Account, AccountId, AccountIdPrefixFull, AccountStorage, Block, BlockInfo, BlockLimits,
        Cell, CurrencyCollection, HashmapAugType, Message, MsgAddressInt, ParamLimits,
        ShardAccounts, ShardIdent, ShardStateUnsplit, StateInit, StorageInfo, TickTock,
    };

    include!("../../block/src/tests/test_utils.rs");

    const CFG_DIR: &str = "../target";
    const CFG_NODE_FILE: &str = "light_node.json";
    const CFG_GLOB_FILE: &str = "light_global.json";
    const DB_PATH: &str = "../target/console_test";

    struct TestEngine {
        counter: Option<Arc<AtomicU64>>,
        db: InternalDb,
        index: u16,
        states: HashMap<BlockIdExt, Arc<ShardStateStuff>>,
        blocks: HashMap<BlockIdExt, Vec<u8>>,
        last_mc_state: Arc<ShardStateStuff>,
        last_validation_time: lockfree::map::Map<ShardIdent, u64>,
        #[cfg(feature = "telemetry")]
        telemetry: Arc<EngineTelemetry>,
        allocated: Arc<EngineAlloc>,
    }

    impl TestEngine {
        async fn new(
            index: u16,
            counter: Option<Arc<AtomicU64>>,
            #[cfg(feature = "telemetry")] telemetry: Arc<EngineTelemetry>,
            allocated: Arc<EngineAlloc>,
        ) -> Self {
            let mut db = Err(error!("Unreachable"));
            let db_path = format!("{}{}", DB_PATH, index);
            for _ in 0..10 {
                fs::remove_dir_all(&db_path).ok();
                let db_config =
                    InternalDbConfig { db_directory: db_path.clone(), ..Default::default() };
                db = InternalDb::with_update(
                    db_config,
                    false,
                    false,
                    false,
                    &|| Ok(()),
                    None,
                    Arc::new(AtomicU8::new(0)),
                    None,
                    #[cfg(feature = "telemetry")]
                    telemetry.clone(),
                    allocated.clone(),
                )
                .await;
                if db.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
            let db = db.unwrap_or_else(|err| panic!("Can't create db: {}", err));

            let mut states = HashMap::new();

            let mut ss = ShardStateUnsplit::with_ident(ShardIdent::full(0));
            let account = generate_test_account(true, AccountTestOptions::with_default_setup(true));
            let account_id = account.get_id().unwrap();
            ss.insert_account(
                &account_id,
                &ShardAccount::with_params(&account, UInt256::default(), 0).unwrap(),
            )
            .unwrap();
            let cell = ss.serialize().unwrap();
            let bytes = write_boc(&cell).unwrap();
            let block_id = BlockIdExt::with_params(
                ShardIdent::full(0),
                0,
                cell.repr_hash(),
                UInt256::calc_file_hash(&bytes),
            );
            let shard_state = ShardStateStuff::deserialize_zerostate(
                block_id.clone(),
                &bytes,
                #[cfg(feature = "telemetry")]
                &telemetry,
                &allocated,
            )
            .unwrap();
            states.insert(block_id, shard_state.clone());

            let ss = Self::prepare_zero_state(shard_state.block_id()).unwrap();
            let cell = ss.serialize().unwrap();
            let bytes = write_boc(&cell).unwrap();
            let block_id = BlockIdExt::with_params(
                ShardIdent::masterchain(),
                ss.seq_no(),
                cell.repr_hash(),
                UInt256::calc_file_hash(&bytes),
            );

            let last_mc_state = ShardStateStuff::deserialize_zerostate(
                block_id,
                &bytes,
                #[cfg(feature = "telemetry")]
                &telemetry,
                &allocated,
            )
            .unwrap();

            states.insert(last_mc_state.block_id().clone(), last_mc_state.clone());
            db.create_or_load_block_handle(last_mc_state.block_id(), None, Some(1), None)
                .unwrap()
                ._to_created()
                .unwrap();

            Self {
                counter,
                db,
                index,
                states,
                blocks: HashMap::new(),
                last_mc_state,
                last_validation_time: lockfree::map::Map::new(),
                telemetry,
                allocated,
            }
        }

        fn prepare_zero_state(wc_zero_state_id: &BlockIdExt) -> Result<ShardStateUnsplit> {
            let config_addr = AccountId::from([0x55; 32]);

            let json = serde_json::from_str(SAMPLE_ZERO_STATE)?;
            let mut ss = ton_block_json::parse_state(&json)?;
            //  ShardStateUnsplit::with_ident(ShardIdent::masterchain());
            let mut ms = ss.read_custom()?.unwrap();
            ms.shards.add_workchain(
                0,
                0,
                wc_zero_state_id.root_hash().clone(),
                wc_zero_state_id.file_hash().clone(),
            )?;
            ss.write_custom(Some(&ms))?;
            let address = MsgAddressInt::standard(-1, config_addr.clone());
            let public_key = base64_decode(PUBLIC_CONFIG_KEY).unwrap();
            let mut builder = 7u32.write_to_new_cell()?; // seqno
            builder.append_raw(&public_key, 256)?;
            builder.append_bit_zero()?;
            builder.checked_append_reference(Default::default())?;
            let code = Default::default(); // empty code
            let data = builder.into_cell()?;
            let mut state_init = StateInit::with_code_and_data(code, data);
            state_init.special = Some(TickTock::with_values(true, true));
            let balance = CurrencyCollection::with_coins(1_000_000_000);
            let mut config_smc = Account::active(
                address,
                balance,
                0,
                0,
                state_init,
                ms.config().size_limits_config()?.acc_state_cells_for_storage_dict,
            )?;
            config_smc.update_config_smc(ms.config())?;
            // let cell = config_smc.serialize()?;
            // panic!("{cell:#.10}");
            let mut shard_accounts = ShardAccounts::new();
            let account = ShardAccount::with_params(&config_smc, UInt256::default(), 0)?;
            shard_accounts.set_augmentable(&config_addr, &account)?;
            // panic!("{:#.10}", shard_accounts.serialize()?);
            ss.write_accounts(&shard_accounts)?;

            Ok(ss)
        }

        fn init_internal(&mut self) -> Result<()> {
            let mut ss = self.last_mc_state.state()?.clone();
            self.update_mc_state(&mut ss, 17).unwrap();
            self.last_mc_state = self.update_mc_state(&mut ss, 19).unwrap();
            Ok(())
        }

        fn update_mc_state(
            &mut self,
            ss: &mut ShardStateUnsplit,
            seq_no: u32,
        ) -> Result<Arc<ShardStateStuff>> {
            assert!(seq_no < 100);
            let config_addr = AccountId::from([0x55; 32]);
            let mut shard_accounts = ss.read_accounts()?;
            let mut account = shard_accounts.get(&config_addr)?.unwrap_or_default();
            let mut config_smc = account.read_account()?;
            config_smc.set_balance(CurrencyCollection::with_coins(seq_no as u64 * 1_000_000_000));
            config_smc.set_code(((seq_no * 100 + seq_no) * 100 + seq_no).serialize()?);
            account.write_account(&config_smc)?;
            let account = ShardAccount::with_params(&config_smc, UInt256::default(), 0).unwrap();
            shard_accounts.set_augmentable(&config_addr, &account).unwrap();
            ss.write_accounts(&shard_accounts).unwrap();

            ss.set_seq_no(seq_no);
            let mut block = Block::default();
            let mut info = BlockInfo::new();
            info.set_seq_no(seq_no)?;
            info.set_gen_utime(seq_no.into());
            block.write_info(&info)?;
            block.global_id = 42;
            let bytes = block.write_to_bytes()?;
            let file_hash = UInt256::calc_file_hash(&bytes);
            let block_id = BlockIdExt::with_params(
                ShardIdent::masterchain(),
                ss.seq_no(),
                [seq_no as u8; 32].into(),
                file_hash,
            );
            self.blocks.insert(block_id.clone(), bytes);
            let state = ShardStateStuff::from_state(
                block_id.clone(),
                ss.clone(),
                #[cfg(feature = "telemetry")]
                &self.telemetry,
                &self.allocated,
            )?;
            self.states.insert(block_id, state.clone());
            let handle = self
                .db
                .create_or_load_block_handle(state.block_id(), Some(&block), Some(seq_no), None)
                .unwrap()
                ._to_created()
                .unwrap();
            handle.set_data();
            handle.set_state();
            handle.set_block_applied();
            Ok(state)
        }

        async fn stop(&self) {
            self.db.stop_states_db().await
        }
    }

    impl Drop for TestEngine {
        fn drop(&mut self) {
            let cfg_glob_file = format!("{}{}", CFG_GLOB_FILE, self.index);
            let cfg_node_file = format!("{}{}", CFG_NODE_FILE, self.index);
            fs::remove_file(Path::new(CFG_DIR).join(cfg_node_file)).ok();
            fs::remove_file(Path::new(CFG_DIR).join(cfg_glob_file)).ok();
        }
    }

    #[async_trait::async_trait]
    impl EngineOperations for TestEngine {
        fn calc_tps(&self, _period: u64) -> Result<u32> {
            Ok(0)
        }
        fn validation_status(&self) -> ValidationStatus {
            ValidationStatus::Active
        }
        fn last_validation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
            &self.last_validation_time
        }
        fn last_collation_time(&self) -> &lockfree::map::Map<ShardIdent, u64> {
            &self.last_validation_time
        }
        fn get_sync_status(&self) -> u32 {
            0
        }
        fn load_block_handle(&self, id: &BlockIdExt) -> Result<Option<Arc<BlockHandle>>> {
            self.db.load_block_handle(id)
        }
        fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.last_mc_state.block_id().clone())))
        }
        async fn load_last_applied_mc_state(&self) -> Result<Arc<ShardStateStuff>> {
            Ok(self.last_mc_state.clone())
        }
        fn load_shard_client_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
            Ok(Some(Arc::new(self.last_mc_state.block_id().clone())))
        }
        async fn load_state(&self, block_id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
            self.states
                .get(block_id)
                .cloned()
                .ok_or_else(|| error!("Can't find state {}", block_id))
        }
        async fn load_and_pin_state(&self, block_id: &BlockIdExt) -> Result<PinnedShardStateGuard> {
            let state = self
                .states
                .get(block_id)
                .cloned()
                .ok_or_else(|| error!("Wrong block ID {}", block_id))?;
            PinnedShardStateGuard::new(state, Arc::new(AllowStateGcSmartResolver::new(10)))
        }
        async fn load_block_raw(&self, handle: &BlockHandle) -> Result<Vec<u8>> {
            if handle.has_data() {
                if let Some(data) = self.blocks.get(handle.id()) {
                    return Ok(data.clone());
                }
            }
            fail!("no block with id {}", handle.id())
        }
        fn find_full_block_id(&self, root_hash: &UInt256) -> Result<Option<BlockIdExt>> {
            Ok(self.states.iter().find_map(|(id, _)| {
                if id.root_hash() == root_hash {
                    Some(id.clone())
                } else {
                    None
                }
            }))
        }
        async fn lookup_block_by_seqno(
            &self,
            prefix: &AccountIdPrefixFull,
            seqno: u32,
        ) -> Result<Option<(BlockIdExt, Vec<u8>)>> {
            if !prefix.is_masterchain() {
                fail!("Only masterchain is supported");
            }
            for (id, data) in self.blocks.iter() {
                if id.shard().is_masterchain() && id.seq_no() == seqno {
                    return Ok(Some((id.clone(), data.clone())));
                }
            }
            fail!("Wrong seqno {}", seqno)
        }
        async fn redirect_external_message(&self, _message: &[u8]) -> Result<()> {
            if let Some(counter) = &self.counter {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
    }

    const ADNL_SERVER_CONFIG: &str = r#"{
        "ton_global_config_name": "light_global.json",
        "adnl_node": {
            "ip_address": "127.0.0.1:node_port",
            "keys": [
                {
                    "tag": 1,
                    "data": {
                        "type_id": 1209251014,
                        "pvt_key": "x3osWUUfcdybRLcmH7mJer4S0NM7TDpEw4SCftKD7bY="
                    }
                },
                {
                    "tag": 2,
                    "data": {
                        "type_id": 1209251014,
                        "pvt_key": "K56upUuRmuZrXhuExmmgr3DQt5Ae12XXQDXUXeGCQxg="
                    }
                }
            ]
        },
        "control_server": {
            "address": "127.0.0.1:control_port",
            "server_key": {
                "type_id": 1209251014,
                "pvt_key": "cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk="
            },
            "clients": {
                "list": [
                    {
                        "type_id": 1209251014,
                        "pub_key": "RYokIiD5AFkzfTBgC6NhtAGFKm0+gwhN4suTzaW0Sjw="
                    }
                ]
            }
        }
    }"#;

    const ADNL_CLIENT_CONFIG: &str = r#"{
        "config": {
            "server_address": "127.0.0.1:control_port",
            "server_key": {
                "type_id": 1209251014,
                "pub_key": "cujCRU4rQbSw48yHVHxQtRPhUlbo+BuZggFTQSu04Y8="
            },
            "client_key": {
                "type_id": 1209251014,
                "pvt_key": "oEivbTDjSOSCgooUM0DAS2z2hIdnLw/PT82A/OFLDmA="
            }
        },
        "wallet_id": "-1:af17db43f40b6aa24e7203a9f8c8652310c88c125062d1129fe883eaa1bd6763",
        "max_factor": 2.7
    }"#;

    const BASE_CONTROL_PORT: u16 = 4291;
    const BASE_NODE_PORT: u16 = 4191;

    const GLOBAL_CONFIG: &str = r#"{
        "validator": {
            "zero_state": {
                "workchain": -1,
                "shard": -9223372036854775808,
                "seqno": 0,
                "root_hash": "6Jb3GskUbO+HxaCnKQPEDPSxCkck7zHL//xPwAmBql0=",
                "file_hash": "Ti5G+UTHu6MTe2IVmMZ7VcQz+dBpET+NApZm+0B4kfc="
            }
        }
    }"#;

    const SAMPLE_ZERO_STATE: &str = r#"{
        "id": "-1:8000000000000000",
        "workchain_id": -1,
        "boc": "",
        "global_id": 42,
        "shard": "8000000000000000",
        "seq_no": 0,
        "vert_seq_no": 0,
        "gen_utime": 1600000000,
        "gen_lt": "0",
        "min_ref_mc_seqno": 4294967295,
        "before_split": false,
        "overload_history": "0",
        "underload_history": "0",
        "total_balance": "4993357197000000000",
        "total_validator_fees": "0",
        "master": {
            "config_addr": "5555555555555555555555555555555555555555555555555555555555555555",
            "config": {
            "p0": "5555555555555555555555555555555555555555555555555555555555555555",
            "p1": "3333333333333333333333333333333333333333333333333333333333333333",
            "p2": "0000000000000000000000000000000000000000000000000000000000000000",
            "p7": [],
            "p8": {
                "version": 11,
                "capabilities": "46"
            },
            "p9": [ 0 ],
            "p10": [ 0 ],
            "p11": {
                "normal_params": {
                    "min_tot_rounds": 2,
                    "max_tot_rounds": 3,
                    "min_wins": 2,
                    "max_losses": 2,
                    "min_store_sec": 1000000,
                    "max_store_sec": 10000000,
                    "bit_price": 1,
                    "cell_price": 500
                },
                "critical_params": {
                    "min_tot_rounds": 4,
                    "max_tot_rounds": 7,
                    "min_wins": 4,
                    "max_losses": 2,
                    "min_store_sec": 5000000,
                    "max_store_sec": 20000000,
                    "bit_price": 2,
                    "cell_price": 1000
                }
            },
            "p12": [],
            "p13": {
                "boc": "te6ccgEBAQEADQAAFRpRdIdugAEBIB9I"
            },
            "p14": {
                "masterchain_block_fee": "1700000000",
                "basechain_block_fee": "1000000000"
            },
            "p15": {
                "validators_elected_for": 65536,
                "elections_start_before": 32768,
                "elections_end_before": 8192,
                "stake_held_for": 32768
            },
            "p16": {
                "max_validators": 1000,
                "max_main_validators": 100,
                "min_validators": 5
            },
            "p17": {
                "min_stake": "10000000000000",
                "max_stake": "10000000000000000",
                "min_total_stake": "100000000000000",
                "max_stake_factor": 196608
            },
            "p18": [
                {
                "utime_since": 0,
                "bit_price_ps": "1",
                "cell_price_ps": "500",
                "mc_bit_price_ps": "1000",
                "mc_cell_price_ps": "500000"
                }
            ],
            "p20": {
                "flat_gas_limit": "1000",
                "flat_gas_price": "10000000",
                "gas_price": "655360000",
                "gas_limit": "1000000",
                "special_gas_limit": "100000000",
                "gas_credit": "10000",
                "block_gas_limit": "10000000",
                "freeze_due_limit": "100000000",
                "delete_due_limit": "1000000000"
            },
            "p21": {
                "flat_gas_limit": "1000",
                "flat_gas_price": "1000000",
                "gas_price": "65536000",
                "gas_limit": "1000000",
                "special_gas_limit": "1000000",
                "gas_credit": "10000",
                "block_gas_limit": "10000000",
                "freeze_due_limit": "100000000",
                "delete_due_limit": "1000000000"
            },
            "p22": {
                "bytes": {
                    "underload": 131072,
                    "soft_limit": 524288,
                    "hard_limit": 1048576
                },
                "gas": {
                    "underload": 900000,
                    "soft_limit": 1200000,
                    "hard_limit": 2000000
                },
                "lt_delta": {
                    "underload": 1000,
                    "soft_limit": 5000,
                    "hard_limit": 10000
                }
            },
            "p23": {
                "bytes": {
                    "underload": 131072,
                    "soft_limit": 524288,
                    "hard_limit": 1048576
                },
                "gas": {
                    "underload": 900000,
                    "soft_limit": 1200000,
                    "hard_limit": 2000000
                },
                "lt_delta": {
                    "underload": 1000,
                    "soft_limit": 5000,
                    "hard_limit": 10000
                }
            },
            "p24": {
                "lump_price": "10000000",
                "bit_price": "655360000",
                "cell_price": "65536000000",
                "ihr_price_factor": 98304,
                "first_frac": 21845,
                "next_frac": 21845
            },
            "p25": {
                "lump_price": "1000000",
                "bit_price": "65536000",
                "cell_price": "6553600000",
                "ihr_price_factor": 98304,
                "first_frac": 21845,
                "next_frac": 21845
            },
            "p28": {
                "shuffle_mc_validators": true,
                "mc_catchain_lifetime": 250,
                "shard_catchain_lifetime": 250,
                "shard_validators_lifetime": 1000,
                "shard_validators_num": 7
            },
            "p29": {
                "new_catchain_ids": true,
                "round_candidates": 3,
                "next_candidate_delay_ms": 2000,
                "consensus_timeout_ms": 16000,
                "fast_attempts": 3,
                "attempt_duration": 8,
                "catchain_max_deps": 4,
                "max_block_bytes": 2097152,
                "max_collated_bytes": 2097152
            },
            "p31": [
                "0000000000000000000000000000000000000000000000000000000000000000"
            ],
            "p34": {
                "utime_since": 1600000000,
                "utime_until": 1610000000,
                "total": 1,
                "main": 1,
                "total_weight": "17",
                "list": [
                    {
                        "public_key": "2e7eb5a711ed946605a91e36037c4cb927181eff4bb277b175d891a588d03536",
                        "weight": "17"
                    }
                ]
            }
            },
            "validator_list_hash_short": 871956759,
            "catchain_seqno": 0,
            "nx_cc_updated": true,
            "after_key_block": true,
            "global_balance": "4993357197000000000"
        },
        "accounts": [],
        "libraries": [],
        "out_msg_queue_info": {
            "out_queue": [],
            "proc_info": []
        }
    }"#;

    async fn init_test(
        counter: Option<Arc<AtomicU64>>,
    ) -> (ControlServer, ControlClient, Arc<TestEngine>) {
        // init_test_log();
        static INDEX: AtomicU16 = AtomicU16::new(0);
        let index = INDEX.fetch_add(1, Ordering::Relaxed);
        let cfg_glob_file = format!("{}{}", CFG_GLOB_FILE, index);
        let cfg_node_file = format!("{}{}", CFG_NODE_FILE, index);
        let node_port = format!(":{}", BASE_NODE_PORT + index);
        let control_port = format!(":{}", BASE_CONTROL_PORT + index);
        let adnl_server_config = ADNL_SERVER_CONFIG
            .replace(":node_port", &node_port)
            .replace(":control_port", &control_port)
            .replace(CFG_GLOB_FILE, &cfg_glob_file);
        fs::write(Path::new(CFG_DIR).join(&cfg_node_file), &adnl_server_config).unwrap();
        fs::write(Path::new(CFG_DIR).join(&cfg_glob_file), GLOBAL_CONFIG).unwrap();
        let node_config =
            TonNodeConfig::from_file(CFG_DIR, &cfg_node_file, None, "", None).unwrap();
        let control_server_config = node_config.control_server().unwrap();
        let config = control_server_config.expect("must have control server setting");
        #[cfg(feature = "telemetry")]
        let telemetry = create_engine_telemetry();
        let allocated = create_engine_allocated();
        let network = NodeNetwork::new(
            node_config,
            tokio_util::sync::CancellationToken::new(),
            #[cfg(feature = "telemetry")]
            telemetry.clone(),
            allocated.clone(),
        )
        .await
        .unwrap();
        let mut engine = TestEngine::new(
            index,
            counter,
            #[cfg(feature = "telemetry")]
            telemetry,
            allocated,
        )
        .await;
        engine.init_internal().unwrap();
        let engine = Arc::new(engine);
        let server = ControlServer::with_params(
            config,
            DataSource::Engine(engine.clone()),
            network.config_handler(),
            network.config_handler(),
            Some(&network),
        )
        .await
        .unwrap();
        let adnl_client_config = ADNL_CLIENT_CONFIG.replace(":control_port", &control_port);
        let config = serde_json::from_str(&adnl_client_config).unwrap();
        let client = ControlClient::connect(config).await.unwrap();
        (server, client, engine)
    }

    async fn done_test(server: ControlServer, client: ControlClient, engine: Arc<TestEngine>) {
        server.shutdown().await;
        client.shutdown().await.ok();
        engine.stop().await;
        let db_path = format!("{}{}", DB_PATH, engine.index);
        drop(engine);
        loop {
            let ok = fs::remove_dir_all(&db_path).is_ok();
            thread::sleep(Duration::from_millis(10));
            if ok {
                break;
            }
        }
    }

    async fn test_one_cmd(cmd: &str, check_result: impl FnOnce(Vec<u8>)) {
        let (server, mut client, engine) = init_test(None).await;
        let (answer, result) = client.command(cmd).await.unwrap();
        println!("{} => {}", cmd, answer);
        check_result(result);
        done_test(server, client, engine).await;
    }

    async fn test_one_cmd_fail(cmd: &str) {
        let (server, mut client, engine) = init_test(None).await;
        let err = client.command(cmd).await.unwrap_err();
        println!("{} => {}", cmd, err);
        done_test(server, client, engine).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_getblock() {
        let cmd = "getblock 17 ../target/17.boc";
        test_one_cmd(cmd, |result| {
            assert_eq!(result.len(), 344);
            let block = Block::construct_from_bytes(&result).unwrap();
            assert_eq!(block.read_info().unwrap().seq_no(), 17);
        })
        .await;
        let cmd = "getblock 19 ../target/19.boc";
        test_one_cmd(cmd, |result| {
            assert_eq!(result.len(), 344);
            let block = Block::construct_from_bytes(&result).unwrap();
            assert_eq!(block.read_info().unwrap().seq_no(), 19);
        })
        .await;
        let cmd = "getblock 15";
        test_one_cmd_fail(cmd).await;
        let cmd = "getblock \"\"";
        test_one_cmd_fail(cmd).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_new_key_one() {
        let cmd = "newkey";
        test_one_cmd(cmd, |result| assert_eq!(result.len(), 32)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_nonexist_account() {
        // let account = "-1:5555555555555555555555555555555555555555555555555555555555555555";
        let account = "-1:7777777777777777777777777777777777777777777777777777777777777777";
        let cmd = format!("getaccount {}", account);
        let etalon_result = "{\n  \"acc_type\": \"Nonexist\"\n}";
        test_one_cmd(&cmd, |result| {
            let result = String::from_utf8(result).unwrap();
            pretty_assertions::assert_eq!(&result, etalon_result)
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_active_account() {
        let account = "0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F";
        let cmd = format!("getaccount {}", account);
        let etalon_result = b"{\n  \"acc_type\": \"Active\",";
        test_one_cmd(&cmd, |result| {
            pretty_assertions::assert_eq!(&result[..etalon_result.len()], etalon_result)
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_account_state() {
        const OUT_FILE: &str = "../target/test_get_account_state_file.boc";
        let account = "0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F";
        let cmd = format!("getaccountstate {} {}", account, OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 253);
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_account_old_state() {
        const OUT_FILE: &str = "../target/test_get_account_old_state_file.boc";
        // config
        let account = "5555555555555555555555555555555555555555555555555555555555555555";
        // // first read from zerostate
        // let cmd = format!("getaccountstate_byblock 0 {} {}", account, OUT_FILE);
        // test_one_cmd(
        //     &cmd,
        //     |result| {
        //         assert_eq!(result.len(), 235);
        //         let account = Account::construct_from_bytes(&result).unwrap();
        //         assert_eq!(account.balance().unwrap().coins.as_u128(), 1_000_000_000);
        //         assert!(account.code().is_none());
        //         fs::remove_file(OUT_FILE).unwrap();
        //     }
        // ).await;

        // read from block 17 (masterchain)
        let id = "1111111111111111111111111111111111111111111111111111111111111111"; // 17
        let cmd = format!("getaccountstate_byblock {} {} {}", id, account, OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 1031);
            let account = Account::construct_from_bytes(&result).unwrap();
            assert_eq!(account.balance().unwrap().coins.as_u128(), 17_000_000_000);
            let code = u32::construct_from_cell(account.code().unwrap().clone()).unwrap();
            assert_eq!(code, 171717);
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;

        // read from block 19 (masterchain)
        let id = "1313131313131313131313131313131313131313131313131313131313131313"; // 19
        let cmd = format!("getaccountstate_byblock {} {} {}", id, account, OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 1031);
            let account = Account::construct_from_bytes(&result).unwrap();
            assert_eq!(account.balance().unwrap().coins.as_u128(), 19_000_000_000);
            let code = u32::construct_from_cell(account.code().unwrap().clone()).unwrap();
            assert_eq!(code, 191919);
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;

        // read from wrong block
        let id = "7777777777777777777777777777777777777777777777777777777777777777";
        let cmd = format!("getaccountstate_byblock {} {} {}", id, account, OUT_FILE);
        test_one_cmd_fail(&cmd).await;

        // read from last block (19) (masterchain)
        let cmd = format!("getaccountstate -1:{} {}", account, OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 1031);
            let account = Account::construct_from_bytes(&result).unwrap();
            assert_eq!(account.balance().unwrap().coins.as_u128(), 19_000_000_000);
            let code = u32::construct_from_cell(account.code().unwrap().clone()).unwrap();
            assert_eq!(code, 191919);
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_stats() {
        fn check(result: Vec<u8>, ethalon: &[(&str, &str)]) {
            let stats = deserialize_boxed(&result).unwrap();
            let stats = downcast::<ton_api::ton::engine::validator::Stats>(stats).unwrap();
            // check if stats in JSON
            let json = format!("{:#}", stats_to_json(stats.stats().iter()));
            serde_json::from_str::<serde_json::Value>(&json)
                .unwrap_or_else(|e| panic!("it must be JSON {e}"));
            for stat in stats.stats().iter() {
                match ethalon.iter().find(|(key, _)| key == &stat.key) {
                    Some((name, value)) => assert_eq!(&stat.value, value, "for {}", name),
                    None => println!("unknown stats: {} => {}", stat.key, stat.value),
                }
            }
        }

        let ethalon_stats = [
            ("in_current_vset_p34", "false"),
            ("in_next_vset_p36", "false"),
            ("validation_stats", "{}"),
            ("collation_stats", "{}"),
            ("masterchainblocknumber", "19"),
            ("masterchainblocktime", "19"),
            ("sync_status", "\"no_set_status\""),
            ("public_overlay_key_id", "\"b52Bb8xv6roWwIDGQnBIdL7hsl3Sy3Ro9ITEV9UlIz8=\""),
            ("tps_10", "0"),
            ("tps_300", "0"),
            ("validation_status", "\"Active\""),
        ];
        test_one_cmd("getstats", |result| check(result, &ethalon_stats)).await;

        let ethalon_stats_new = [
            ("in_current_vset_p34", "false"),
            ("in_next_vset_p36", "false"),
            ("global_id", "42"),
            ("last_validation_ago_sec", "{}"),
            ("last_collation_ago_sec", "{}"),
            ("masterchainblocknumber", "19"),
            ("masterchainblocktime", "19"),
            ("node_status", "\"no_set_status\""),
            ("public_overlay_key_id", "\"b52Bb8xv6roWwIDGQnBIdL7hsl3Sy3Ro9ITEV9UlIz8=\""),
            ("tps_10", "0"),
            ("tps_300", "0"),
            ("validation_status", "\"Active\""),
        ];
        test_one_cmd("getstatsnew", |result| check(result, &ethalon_stats_new)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_election_bid() {
        const OUT_FILE: &str = "../target/test_election_bid_file.boc";
        let now = UnixTime::now() + 86400;
        let cmd = format!("election-bid {} {} \"{}\"", now, now + 10001, OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 164);
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_recover_stake() {
        const OUT_FILE: &str = "../target/test_recover_stake_file.boc";
        let cmd = format!("recover_stake \"{}\"", OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 25);
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_config_param() {
        const IN_FILE: &str = "../target/zerostate.json";
        const OUT_FILE: &str = "../target/test_config_param_file.boc";
        std::fs::write(IN_FILE, SAMPLE_ZERO_STATE).unwrap();
        let cmd = format!("cparam 23 \"{}\" \"{}\"", IN_FILE, OUT_FILE);
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 53);
            let limits = BlockLimits::construct_from_bytes(&result).unwrap();
            assert_eq!(limits.bytes(), &ParamLimits::with_limits(131072, 524288, 1048576).unwrap());
            assert_eq!(limits.gas(), &ParamLimits::with_limits(900000, 1200000, 2000000).unwrap());
            assert_eq!(limits.lt_delta(), &ParamLimits::with_limits(1000, 5000, 10000).unwrap());
            fs::remove_file(IN_FILE).unwrap();
            fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_new_key_with_export() {
        let (server, mut client, engine) = init_test(None).await;
        let (_, result) = client.command("newkey").await.unwrap();
        assert_eq!(result.len(), 32);
        let cmd = format!("exportpub {}", base64_encode(&result));
        let (_, result) = client.command(&cmd).await.unwrap();
        assert_eq!(result.len(), 32);
        done_test(server, client, engine).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_set_config_param() {
        const IN_FILE: &str = "../target/test_set_config_param.json";
        const OUT_FILE: &str = "../target/test_set_config_param.boc";
        const KEY_FILE: &str = "../target/test_set_config_param.key";
        // read from last block (19) (masterchain)
        let cmd = format!("getaccountstate -1:5555555555555555555555555555555555555555555555555555555555555555 {OUT_FILE}");
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 1031);
            let account = Account::construct_from_bytes(&result).unwrap();
            assert_eq!(account.data().unwrap().bit_length(), 32 + 256 + 1);
            assert_eq!(account.balance().unwrap().coins.as_u128(), 19_000_000_000);
            let code = u32::construct_from_cell(account.code().unwrap().clone()).unwrap();
            assert_eq!(code, 191919);
            std::fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
        let _ = std::fs::remove_file(OUT_FILE);
        let json = r#"{
            "p8": {
                "version": 11,
                "capabilities_dec": "46",
                "capabilities": "12e"
            }
        }"#;
        std::fs::write(IN_FILE, &json).unwrap();
        std::fs::write(KEY_FILE, base64_decode(SECRET_CONFIG_KEY).unwrap()).unwrap();
        let cmd = format!("setcparam {IN_FILE} {KEY_FILE} {OUT_FILE}");
        test_one_cmd(&cmd, |result| {
            assert_eq!(result.len(), 144);
            assert_eq!(std::fs::metadata(OUT_FILE).unwrap().len(), 144);
            std::fs::remove_file(OUT_FILE).unwrap();
        })
        .await;
    }

    macro_rules! parse_test {
        ($func:expr, $param:expr) => {
            $func($param.split_whitespace().next(), "test")
        };
    }

    #[test]
    fn test_parse_int() {
        assert_eq!(parse_test!(parse_int, "0").unwrap(), 0);
        assert_eq!(parse_test!(parse_int, "-1").unwrap(), -1);
        assert_eq!(parse_test!(parse_int, "1600000000").unwrap(), 1600000000);
        parse_test!(parse_int, "qwe").expect_err("must generate error");
        parse_int(Option::<&str>::None, "test").expect_err("must generate error");
    }

    #[test]
    fn test_parse_int256() {
        let ethalon = "GfgI79Xf3q7r4q1SPz7wAqBt0W6CjavuADODoz/DQE8=";
        assert_eq!(
            parse_test!(parse_int256, ethalon).unwrap(),
            ethalon.parse::<UInt256>().unwrap()
        );
        assert_eq!(
            parse_test!(
                parse_int256,
                "19F808EFD5DFDEAEEBE2AD523F3EF002A06DD16E828DABEE003383A33FC3404F"
            )
            .unwrap(),
            ethalon.parse::<UInt256>().unwrap()
        );
        parse_test!(parse_int256, "11").expect_err("must generate error");
        parse_int256(Option::<&str>::None, "test").expect_err("must generate error");
    }

    #[test]
    fn test_parse_data() {
        let ethalon = vec![10, 77];
        assert_eq!(parse_test!(parse_data, "0A4D").unwrap(), ethalon);
        parse_test!(parse_data, "QQ").expect_err("must generate error");
        parse_test!(parse_data, "GfgI79Xf3q7r4q1SPz7wAqBt0W6CjavuADODoz/DQE8=")
            .expect_err("must generate error");
        parse_data(Option::<&str>::None, "test").expect_err("must generate error");
    }

    #[test]
    fn test_stats_to_json() {
        let mut stats = [OneStat { key: "key".to_string(), value: String::new() }];
        let value = stats_to_json(stats.iter());
        let ethalon = serde_json::json!({"key": "null"});
        assert_eq!(ethalon, value);

        stats[0].value = "12345".into();
        let value = stats_to_json(stats.iter());
        let ethalon = serde_json::json!({"key": 12345});
        assert_eq!(ethalon, value);

        stats[0].value = "\"Some text\"".into();
        let value = stats_to_json(stats.iter());
        let ethalon = serde_json::json!({"key": "Some text"});
        assert_eq!(ethalon, value);

        stats[0].value = "\"777\"".into();
        let value = stats_to_json(stats.iter());
        let ethalon = serde_json::json!({"key": "777"});
        assert_eq!(ethalon, value);

        stats[0].value = serde_json::json!({"a": 777}).to_string();
        let value = stats_to_json(stats.iter());
        let ethalon = serde_json::json!({"key": {"a": 777}});
        assert_eq!(ethalon, value);
    }

    fn generate_boc(mut size: u32) -> Result<Vec<u8>> {
        // Raw data, withoud BOC format
        // let data: Vec<u8> = (0..size).map(|_| rand::random::<u8>()).collect();
        // Ok(data)

        let mut cells = Vec::new();

        // Time per 100000 iterations
        // 0 ms
        size *= 8;
        while size > 0 {
            let mut builder = BuilderData::new();
            let bits = min(1023, size);
            let data: Vec<u8> = (0..(bits + 7) / 8).map(|_| rand::random::<u8>()).collect();
            // 300 ms
            builder.append_raw(&data, bits as usize)?;
            size -= bits;
            // 330 ms
            if let Some(child) = cells.pop() {
                builder.checked_append_reference(child)?;
            }
            // 330 ms
            let cell = builder.into_cell()?;
            // 520 ms
            cells.push(cell);
            // 550 ms
        }

        let Some(cell) = cells.pop() else { fail!("BOC generation failed") };
        // 550 ms
        write_boc(&cell)
        // 850 ms
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_ext_msg_proxy() {
        let counter = Arc::new(AtomicU64::new(0));
        let counter_clone = counter.clone();
        tokio::spawn(async move {
            let addr: SocketAddr = "127.0.0.1:4001".parse().unwrap();
            let mut stream = loop {
                if let Ok(stream) = tokio::net::TcpStream::connect(addr).await {
                    break stream;
                }
            };
            let mut rng = rand::rngs::StdRng::seed_from_u64(0);
            let start = Instant::now();
            let count = 100000;
            for _ in 0..count {
                let range = rand::distributions::Uniform::new(256, 1024);
                let len = rng.sample(range);
                let msg = generate_boc(len).unwrap();
                let len = (msg.len() as u16).to_be_bytes();
                stream.write_all(&len[..]).await.unwrap();
                stream.write_all(&msg[..]).await.unwrap();
            }
            println!("Generated {} messages during {} ms", count, start.elapsed().as_millis());
            while counter_clone.load(Ordering::Relaxed) < count {
                tokio::time::sleep(Duration::from_millis(1)).await
            }
            println!("Proxied {} messages during {} ms", count, start.elapsed().as_millis());
            let close = b"close_proxy";
            let len = (close.len() as u16).to_be_bytes();
            stream.write_all(&len[..]).await.unwrap();
            stream.write_all(close).await.unwrap();
        });

        let (server, mut client, engine) = init_test(Some(counter.clone())).await;
        let (_, result) = client.command("ext_msg_proxy 4001").await.unwrap();
        let mut count: u64 = 0;
        for i in &result {
            count = (count << 8) | (*i as u64);
        }
        assert_eq!(count, counter.load(Ordering::Relaxed));
        done_test(server, client, engine).await;
    }
}
