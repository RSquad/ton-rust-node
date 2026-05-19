/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::consensus::{
    BlockHash, BlockPayloadPtr, ConsensusNode, PublicKey, PublicKeyHash, SessionNode,
};
use crate::{engine_traits::EngineOperations, shard_state::ShardStateStuff};
use secrets_vault::vault_block::get_key_option_factory;
use std::{
    fmt::{Debug, Display, Formatter, Write},
    hash::Hash,
    sync::Arc,
};
use ton_api::ton::engine::validator::validator::groupmember::GroupMember;
use ton_block::{
    error, fail, BlockIdExt, BlockSignatures, BlockSignaturesPure, BuilderData, ConfigParams,
    CryptoSignature, CryptoSignaturePair, KeyId, Result, Serializable, Sha256, ShardIdent,
    SigPubKey, UInt256, ValidatorBaseInfo, ValidatorDescr, ValidatorSet, WorkchainDescr,
    Workchains,
};

pub fn sigpubkey_to_publickey(k: &SigPubKey) -> PublicKey {
    get_key_option_factory().from_public_key(k.key_bytes())
}

pub fn make_cryptosig(s: BlockPayloadPtr) -> Result<CryptoSignature> {
    return CryptoSignature::from_bytes(s.data().as_slice());
}

pub fn make_cryptosig_pair(pair: (PublicKeyHash, BlockPayloadPtr)) -> Result<CryptoSignaturePair> {
    let csig = make_cryptosig(pair.1)?;
    return Ok(CryptoSignaturePair::with_params(pair.0.data().into(), csig));
}

pub fn pairvec_to_cryptopair_vec(
    vec: Vec<(PublicKeyHash, BlockPayloadPtr)>,
) -> Result<Vec<CryptoSignaturePair>> {
    vec.into_iter().map(make_cryptosig_pair).collect()
}

#[allow(dead_code)]
pub fn pairvec_to_puresigs(
    pvec: Vec<(PublicKeyHash, BlockPayloadPtr)>,
) -> Result<BlockSignaturesPure> {
    let mut pure_sigs = BlockSignaturesPure::new();
    for p in pvec {
        let pair = make_cryptosig_pair(p)?;
        pure_sigs.add_sigpair(pair);
    }
    Ok(pure_sigs)
}

#[allow(dead_code)]
pub fn pairvec_val_to_sigs(
    pvec: Vec<(PublicKeyHash, BlockPayloadPtr)>,
    vset: &ValidatorSet,
) -> Result<BlockSignatures> {
    let pure_sigs = pairvec_to_puresigs(pvec)?;
    let vset_catchain_seqno = vset.catchain_seqno();
    let vset_hash = ValidatorSet::calc_subset_hash_short(vset.list(), vset_catchain_seqno)?;
    let vset_info = ValidatorBaseInfo::with_params(vset_hash, vset_catchain_seqno);
    Ok(BlockSignatures::with_params(vset_info, pure_sigs))
}

pub fn check_crypto_signatures(
    signatures: &BlockSignaturesPure,
    validators_list: &[ValidatorDescr],
    data: &[u8],
) -> Result<u64> {
    // Delegate to canonical implementation from `ton_block`, which:
    // - verifies Ed25519 signatures against `data`
    // - rejects duplicated signatures from the same node_id_short
    // - sums validator weights for matching validators
    signatures.check_signatures(validators_list, data)
}

pub fn validatordescr_to_consensus_node(descr: &ValidatorDescr) -> ConsensusNode {
    ConsensusNode {
        adnl_id: get_adnl_id(descr),
        public_key: sigpubkey_to_publickey(&descr.public_key),
    }
}

pub fn validatordescr_to_session_node(descr: &ValidatorDescr) -> Result<SessionNode> {
    Ok(SessionNode {
        adnl_id: get_adnl_id(descr),
        public_key: sigpubkey_to_publickey(&descr.public_key),
        weight: descr.weight,
    })
}

pub fn validator_query_candidate_to_validator_block_candidate(
    source: PublicKey,
    candidate: super::BlockCandidate,
) -> Arc<validator_session::ValidatorBlockCandidate> {
    Arc::new(validator_session::ValidatorBlockCandidate {
        public_key: source,
        id: candidate.block_id,
        collated_file_hash: candidate.collated_file_hash,
        data: catchain::CatchainFactory::create_block_payload(candidate.data),
        collated_data: catchain::CatchainFactory::create_block_payload(candidate.collated_data),
    })
}

pub fn validatorset_to_string(vs: &ValidatorSet) -> String {
    let mut res = string_builder::Builder::default();
    let vs_list = vs.list();
    res.append(format!("val_set.cc_seqno = {} ", vs.cc_seqno()));
    for i in 0..vs_list.len() {
        if let Some(x) = vs_list.get(i) {
            let adnl =
                x.adnl_addr.clone().map_or("** no-addr **".to_string(), |x| x.to_hex_string());
            res.append(format!(
                "val_set.{}.pk = {} val_set.{}.weight = {} val_set.{}.adnl = {} ",
                i,
                hex::encode(x.public_key.key_bytes()),
                i,
                x.weight,
                i,
                adnl
            ));
        }
    }
    res.string().unwrap_or_default()
}

// returns adnl_id of validator or calc it by the
pub fn get_adnl_id(validator: &ValidatorDescr) -> Arc<KeyId> {
    if let Some(addr) = &validator.adnl_addr {
        KeyId::from_data(*addr.as_slice())
    } else {
        KeyId::from_data(validator.compute_node_id_short().inner())
    }
}

pub type ValidatorListHash = UInt256;

/// compute sha256 for hashes of public keys of all validators
pub fn compute_validator_list_id(
    list: &[ValidatorDescr],
    session_data: Option<(u32, u32, &ShardIdent)>,
) -> Result<Option<ValidatorListHash>> {
    if !list.is_empty() {
        let mut hasher = Sha256::new();
        if let Some((cc, master_cc, shard)) = session_data {
            hasher.update(cc.to_be_bytes());
            hasher.update(master_cc.to_be_bytes());
            let mut serialized = BuilderData::new();
            shard.write_to(&mut serialized)?;
            hasher.update(serialized.data());
        }
        for x in list {
            hasher.update(x.compute_node_id_short().as_slice());
        }
        let hash: [u8; 32] = hasher.finalize();
        Ok(Some(hash.into()))
    } else {
        Ok(None)
    }
}

// pub fn get_validator_key_idx_in_validator_set(key: &PublicKey, set: &ValidatorSet) -> Result<u32> {
//     let mut idx = 0;
//     for validator in set.list() {
//         let validator_key = sigpubkey_to_publickey(&validator.public_key);
//         if key.id() == validator_key.id() {
//             return Ok(idx);
//         }
//         idx += 1;
//     }
//     fail!("Key {} not found in validator set {:?}", key.id(), set)
// }

pub fn compute_validator_set_cc(
    mc_state: &ShardStateStuff,
    shard: &ShardIdent,
    seq_no: u32,
    cc_seqno: u32,
    cc_seqno_delta: &mut u32,
) -> Result<Vec<ValidatorDescr>> {
    let config = mc_state.config_params()?;
    let vset = config.validator_set()?;
    if (*cc_seqno_delta & 0xfffffffe) != 0 {
        fail!("seqno_delta>1 is not implemented yet");
    }
    *cc_seqno_delta += cc_seqno;
    let workchain_info = if shard.is_masterchain() {
        calc_subset_for_masterchain(&vset, config, *cc_seqno_delta)?
    } else {
        {
            let _ = seq_no;
            calc_subset_for_workchain_standard(&vset, config, shard, *cc_seqno_delta)?
        }
    };

    Ok(workchain_info.validators)
}

#[derive(Clone, Debug)]
pub struct ValidatorSubsetInfo {
    pub validators: Vec<ValidatorDescr>,
    pub short_hash: u32,
}

impl ValidatorSubsetInfo {
    pub fn compute_validator_set(&self, cc_seqno: u32) -> Result<ValidatorSet> {
        ValidatorSet::with_cc_seqno(0, 0, 0, cc_seqno, self.validators.clone())
    }

    /*
           if self.collator_range.len() > 1 {
               fail!("{} has too many collators: [{}]",
                   self.proof_for(),
                   subset.collator_range.iter().map(|c| format!("{} ", c)).collect::<String>()
               )
           }

           let range = subset.collator_range.get(0).ok_or_else(
               || error!("{} has no collator range in val. set", self.proof_for())
           )?;
    */
}

pub fn try_calc_subset_for_workchain_standard(
    vset: &ValidatorSet,
    config: &ConfigParams,
    shard_id: &ShardIdent,
    cc_seqno: u32,
) -> Result<Option<ValidatorSubsetInfo>> {
    let cc_config = config.catchain_config()?;
    let workchain_id = shard_id.workchain_id();
    let shard_pfx = shard_id.shard_prefix_with_tag();

    let (ws, hash) = vset.calc_subset(&cc_config, shard_pfx, workchain_id, cc_seqno)?;
    Ok(Some(ValidatorSubsetInfo { validators: ws, short_hash: hash }))
}

lazy_static::lazy_static! {
    static ref SINGLE_WORKCHAIN: Workchains = {
        let mut workchains = Workchains::default();
        workchains.set(&0, &WorkchainDescr::default()).unwrap();
        workchains
    };
}

pub fn try_calc_subset_for_workchain(
    vset: &ValidatorSet,
    mc_state: &ShardStateStuff,
    shard_id: &ShardIdent,
    cc_seqno: u32,
    block_seqno: u32,
) -> Result<Option<ValidatorSubsetInfo>> {
    let config = mc_state.config_params()?;

    let _ = block_seqno;

    try_calc_subset_for_workchain_standard(vset, config, shard_id, cc_seqno)
}

pub fn calc_subset_for_masterchain(
    vset: &ValidatorSet,
    config: &ConfigParams,
    cc_seqno: u32,
) -> Result<ValidatorSubsetInfo> {
    match try_calc_subset_for_workchain_standard(
        vset,
        config,
        &ShardIdent::masterchain(),
        cc_seqno,
    )? {
        Some(x) => Ok(x),
        None => fail!(
            "Not enough validators from total {} for masterchain cc_seqno: {}",
            vset.list().len(),
            cc_seqno
        ),
    }
}

pub fn calc_subset_for_workchain_standard(
    vset: &ValidatorSet,
    config: &ConfigParams,
    shard_id: &ShardIdent,
    cc_seqno: u32,
) -> Result<ValidatorSubsetInfo> {
    if shard_id.is_masterchain() {
        fail!(
            "calc_subset_for_workchain_standard must be called for shardchain only, but called for {}",
            shard_id
        );
    }

    match try_calc_subset_for_workchain_standard(vset, config, shard_id, cc_seqno)? {
        Some(x) => Ok(x),
        None => fail!(
            "Not enough validators from total {} for workchain {} cc_seqno: {}",
            vset.list().len(),
            shard_id,
            cc_seqno
        ),
    }
}

/// Creates group members from validator descriptors for session ID calculation.
///
/// IMPORTANT: When `adnl_addr` is `None` (format 0x53 in TLB), we use zeros
/// to match C++ behavior for session ID calculation. C++ represents "no ADNL"
/// as zero bytes in ValidatorDescr, so session ID must use zeros too.
///
/// Note: For runtime communication (not session ID), C++ uses `hash(pubkey)`
/// as ADNL fallback - but that's handled separately in `validatordescr_to_session_node`.
pub fn get_group_members_by_validator_descrs(
    iterator: &[ValidatorDescr],
    dst: &mut Vec<GroupMember>,
) {
    for descr in iterator.iter() {
        let node_id = descr.compute_node_id_short();
        // Use zeros when adnl_addr is None to match C++ session ID calculation
        let adnl_id = descr.adnl_addr.clone().unwrap_or_default();
        dst.push(ton_api::ton::engine::validator::validator::groupmember::GroupMember {
            public_key_hash: node_id,
            adnl: adnl_id,
            weight: descr.weight as i64,
        });
    }
}

pub async fn get_masterchain_seqno(
    engine: Arc<dyn EngineOperations>,
    mc_state: &ShardStateStuff,
) -> Result<u32> {
    let mc_state_extra = mc_state.shard_state_extra()?;
    let master_cc_seqno = mc_state_extra.validator_info.catchain_seqno;

    // Just paranoidal check
    let block_id = mc_state.block_id();
    if block_id.seq_no > 0 {
        let handle =
            engine.load_block_handle(block_id)?.ok_or_else(|| error!("No block {}", block_id))?;
        let block = engine.load_block(&handle).await?;
        let gen_catchain_seqno = block.block()?.read_info()?.gen_catchain_seqno();
        let nx_increment = mc_state_extra.validator_info.nx_cc_updated as u32;
        if gen_catchain_seqno + nx_increment != master_cc_seqno {
            fail!(
                "get_masterchain_seqno: different cc_seqno: {} + {} /= {}",
                gen_catchain_seqno,
                nx_increment,
                master_cc_seqno
            )
        }
    }

    Ok(master_cc_seqno)
}

#[derive(Clone)]
pub struct PrevBlockHistory {
    shard: ShardIdent,
    prev: Vec<BlockIdExt>,
    next_seqno: Option<u32>,
}

pub fn fmt_next_block_descr_from_next_seqno(
    shard_ident: &ShardIdent,
    next_seqno_opt: Option<u32>,
    root_hash: Option<&BlockHash>,
) -> String {
    match (next_seqno_opt, root_hash) {
        (None, _) => shard_ident.to_string(),
        (Some(no), None) => format!("{}, {}", shard_ident, no),
        (Some(no), Some(rh)) => format!("{}, {}, {:x}", shard_ident, no, rh),
    }
}

impl PrevBlockHistory {
    pub fn with_shard(shard: &ShardIdent) -> Self {
        Self { shard: shard.clone(), prev: vec![], next_seqno: None }
    }

    pub fn with_id(id: BlockIdExt) -> Self {
        Self { shard: id.shard().clone(), next_seqno: Some(id.seq_no() + 1), prev: vec![id] }
    }

    pub fn with_prevs(shard: &ShardIdent, prev: Vec<BlockIdExt>) -> Self {
        let next_seqno = get_first_block_seqno_after_prevs(&prev);
        Self { shard: shard.clone(), prev, next_seqno }
    }

    pub fn update_prev(&mut self, prev: Vec<BlockIdExt>) {
        self.prev = prev;
        self.next_seqno = get_first_block_seqno_after_prevs(&self.prev);
    }

    pub fn get_next_seqno(&self) -> Option<u32> {
        self.next_seqno
    }

    pub fn get_next_block_descr(&self, rh: Option<&BlockHash>) -> String {
        fmt_next_block_descr_from_next_seqno(&self.shard, self.next_seqno, rh)
    }

    pub fn get_prevs(&self) -> &[BlockIdExt] {
        &self.prev
    }
    pub fn get_prev(&self, index: usize) -> Option<&BlockIdExt> {
        self.prev.get(index)
    }

    pub fn same_prevs(&self, other: &PrevBlockHistory) -> bool {
        self.shard == other.shard && self.prev == other.prev
    }

    pub fn is_next_block_new(&self, root_hash: &BlockHash, file_hash: &BlockHash) -> bool {
        self.prev.iter().all(|x| x.root_hash != *root_hash && x.file_hash != *file_hash)
    }

    pub fn ensure_next_block_new(
        &self,
        root_hash: &BlockHash,
        file_hash: &BlockHash,
    ) -> Result<()> {
        if !self.is_next_block_new(root_hash, file_hash) {
            fail!(
                "Block candidate with rh {:x}, fh {:x} is not unique: prevs {}",
                root_hash,
                file_hash,
                self
            );
        }
        Ok(())
    }

    pub fn get_next_block_id(&self, root_hash: &BlockHash, file_hash: &BlockHash) -> BlockIdExt {
        BlockIdExt {
            shard_id: self.shard.clone(),
            seq_no: self.next_seqno.unwrap_or(1),
            root_hash: root_hash.clone(),
            file_hash: file_hash.clone(),
        }
    }

    pub fn display_prevs(&self) -> String {
        prevs_to_string(&self.prev)
    }
}

pub fn prevs_to_string(prev_block_ids: &[BlockIdExt]) -> String {
    prev_block_ids.iter().fold(String::new(), |mut res, x| {
        let _ = write!(res, "{} ", x);
        res
    })
}

impl Display for PrevBlockHistory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "prev: {}", self.display_prevs())
    }
}

pub fn get_first_block_seqno_after_prevs(prevs: &[BlockIdExt]) -> Option<u32> {
    prevs.iter().map(|blk| blk.seq_no).max().map(|x| x + 1)
}
/// Lock-free map to small set (small set means 1-10 records in one typical map cell)
pub struct LockfreeMapSet<K, V>
where
    V: Ord,
    V: Clone + Debug,
    K: Clone + Hash + Ord + Debug,
{
    map: dashmap::DashMap<K, Vec<V>>,
}

impl<K, V> LockfreeMapSet<K, V>
where
    V: Ord,
    V: Clone + Debug,
    K: Clone + Hash + Ord + Debug,
{
    #[allow(dead_code)]
    fn remove_and_sort(src: &[V], old_to_remove: &V) -> Vec<V> {
        let mut canonized: Vec<V> = src.iter().filter(|x| *x != old_to_remove).cloned().collect();
        canonized.sort();
        canonized
    }

    #[allow(dead_code)]
    pub fn remove_from_set(&self, msg_uid: &K, msg_id: &V) -> Result<()> {
        if let Some(mut t) = self.map.get_mut(msg_uid) {
            *t = Self::remove_and_sort(t.value(), msg_id)
        }
        //self.map.remove_if(msg_uid, |_k,v| v.len() == 0);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_lowest(&self, msg_uid: &K) -> Option<V> {
        self.map.get(msg_uid).and_then(|v| v.value().first().cloned())
    }

    #[allow(dead_code)]
    pub fn contains_in_set(&self, msg_uid: &K, msg_id: &V) -> bool {
        match self.map.get(msg_uid) {
            None => false,
            Some(kv) => {
                let mut lw = 0;
                let mut up = kv.value().len();
                while lw < up {
                    let mid = (lw + up) / 2;
                    if &kv.value()[mid] < msg_id {
                        lw = mid + 1;
                    } else {
                        up = mid;
                    }
                }
                lw < kv.value().len() && &kv.value()[lw] == msg_id
            }
        }
    }
}

impl<K, V> Default for LockfreeMapSet<K, V>
where
    K: Clone + Hash + Ord + Debug,
    V: Clone + Ord + Debug,
{
    fn default() -> Self {
        LockfreeMapSet { map: dashmap::DashMap::new() }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct GeneralSessionInfo {
    pub shard: ShardIdent,
    pub opts_hash: UInt256,
    pub catchain_seqno: u32,
    pub key_seqno: u32,
    pub max_vertical_seqno: u32,
}

impl Display for GeneralSessionInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}, cc {}", self.shard, self.catchain_seqno)
    }
}
