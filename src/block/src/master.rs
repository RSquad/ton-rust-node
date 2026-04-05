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
use crate::{
    bintree::{BinTree, BinTreeType},
    blocks::{Block, BlockIdExt, ExtBlkRef},
    config_params::ConfigParams,
    define_HashmapAugE, define_HashmapE,
    dictionary::hashmapaug::{Augmentable, HashmapAugType, TraverseNextStep},
    error,
    error::BlockError,
    fail,
    inbound_messages::InMsg,
    shard::{AccountIdPrefixFull, ShardIdent, SHARD_FULL},
    signature::CryptoSignaturePair,
    types::{ChildCell, CurrencyCollection, InRefValue},
    validators::ValidatorInfo,
    AccountId, Augmentation, BuilderData, Cell, Deserializable, IBitstring, Result, Serializable,
    SliceData, UInt256,
};
use std::fmt;

#[cfg(test)]
#[path = "tests/test_master.rs"]
mod tests;

/*
_ (HashmapE 32 ^(BinTree ShardDescr)) = ShardHashes;
_ (HashmapAugE 96 ShardFeeCreated ShardFeeCreated) = ShardFees;
*/
define_HashmapE! {ShardHashes, 32, InRefValue<BinTree<ShardDescr>>}
define_HashmapE! {CryptoSignatures, 16, CryptoSignaturePair}
define_HashmapAugE! {ShardFees, 96, ShardIdentFull, ShardFeeCreated, ShardFeeCreated}

impl Augmentation<ShardFeeCreated> for ShardFeeCreated {
    fn aug(&self) -> Result<ShardFeeCreated> {
        Ok(self.clone())
    }
}

#[derive(Clone, Debug, Default)]
pub struct ShardIdentFull {
    pub workchain_id: i32,
    pub prefix: u64, // with terminated bit!
}

impl ShardIdentFull {
    pub fn new(workchain_id: i32, prefix: u64) -> ShardIdentFull {
        ShardIdentFull { workchain_id, prefix }
    }
}

impl Serializable for ShardIdentFull {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.workchain_id.write_to(cell)?;
        self.prefix.write_to(cell)?;
        Ok(())
    }
}

impl Deserializable for ShardIdentFull {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.workchain_id.read_from(cell)?;
        self.prefix.read_from(cell)?;
        Ok(())
    }
}

impl fmt::Display for ShardIdentFull {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{:016X}", self.workchain_id, self.prefix)
    }
}

impl fmt::LowerHex for ShardIdentFull {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}:{:016X}", self.workchain_id, self.prefix)
    }
}

impl ShardHashes {
    pub fn iterate_shards_for_workchain<F>(&self, workchain_id: i32, mut func: F) -> Result<()>
    where
        F: FnMut(ShardIdent, ShardDescr) -> Result<bool>,
    {
        if let Some(InRefValue(shards)) = self.get(&workchain_id)? {
            shards.iterate(|prefix, shard_descr| {
                let shard_ident = ShardIdent::with_prefix_slice(workchain_id, prefix)?;
                func(shard_ident, shard_descr)
            })?;
        }
        Ok(())
    }
    pub fn iterate_shards<F>(&self, mut func: F) -> Result<bool>
    where
        F: FnMut(ShardIdent, ShardDescr) -> Result<bool>,
    {
        self.iterate_with_keys(|wc_id: i32, InRefValue(shards)| {
            shards.iterate(|prefix, shard_descr| {
                let shard_ident = ShardIdent::with_prefix_slice(wc_id, prefix)?;
                func(shard_ident, shard_descr)
            })
        })
    }
    pub fn iterate_shards_with_siblings<F>(&self, mut func: F) -> Result<bool>
    where
        F: FnMut(ShardIdent, ShardDescr, Option<ShardDescr>) -> Result<bool>,
    {
        self.iterate_with_keys(|wc_id: i32, InRefValue(shards)| {
            shards.iterate_pairs(|prefix, shard_descr, sibling| {
                let prefix = SliceData::load_bitstring(prefix)?;
                let shard_ident = ShardIdent::with_prefix_slice(wc_id, prefix)?;
                func(shard_ident, shard_descr, sibling)
            })
        })
    }
    pub fn iterate_shards_with_siblings_mut<F>(&self, mut _func: F) -> Result<()>
    where
        F: FnMut(ShardIdent, ShardDescr, Option<ShardDescr>) -> Result<Option<ShardDescr>>,
    {
        unimplemented!()
    }
    pub fn find_shard(&self, shard: &ShardIdent) -> Result<Option<McShardRecord>> {
        if let Some(InRefValue(bintree)) = self.get(&shard.workchain_id())? {
            let shard_id = shard.shard_key(false);
            if let Some((key, descr)) = bintree.find(shard_id)? {
                let shard = ShardIdent::with_prefix_slice(shard.workchain_id(), key)?;
                return Ok(Some(McShardRecord::from_shard_descr(shard, descr)));
            }
        }
        Ok(None)
    }
    pub fn find_shard_by_prefix(
        &self,
        prefix: &AccountIdPrefixFull,
    ) -> Result<Option<McShardRecord>> {
        if let Some(InRefValue(bintree)) = self.get(&prefix.workchain_id())? {
            let shard_id = prefix.shard_key(false);
            if let Some((key, descr)) = bintree.find(shard_id)? {
                let shard = ShardIdent::with_prefix_slice(prefix.workchain_id(), key)?;
                return Ok(Some(McShardRecord::from_shard_descr(shard, descr)));
            }
        }
        Ok(None)
    }
    pub fn get_shard(&self, shard: &ShardIdent) -> Result<Option<McShardRecord>> {
        if let Some(InRefValue(bintree)) = self.get(&shard.workchain_id())? {
            let shard_id = shard.shard_key(false);
            if let Some(descr) = bintree.get(shard_id)? {
                return Ok(Some(McShardRecord::from_shard_descr(shard.clone(), descr)));
            }
        }
        Ok(None)
    }
    pub fn calc_shard_cc_seqno(&self, shard: &ShardIdent) -> Result<u32> {
        if shard.is_masterchain() {
            fail!("Given `shard` can't be masterchain")
        }
        ShardIdent::check_workchain_id(shard.workchain_id())?;

        let shard1 = self
            .find_shard(&shard.left_ancestor_mask()?)?
            .ok_or_else(|| error!("get_shard_cc_seqno: can't find shard1"))?;

        if shard1.shard().is_ancestor_for(shard) {
            return Ok(shard1.descr.next_catchain_seqno);
        } else if !shard.is_parent_for(shard1.shard()) {
            fail!("get_shard_cc_seqno: invalid shard1 {} for {}", shard1.shard(), shard)
        }

        let shard2 = self
            .find_shard(&shard.right_ancestor_mask()?)?
            .ok_or_else(|| error!("get_shard_cc_seqno: can't find shard2"))?;

        if !shard.is_parent_for(shard2.shard()) {
            fail!("get_shard_cc_seqno: invalid shard2 {} for {}", shard2.shard(), shard)
        }

        Ok(std::cmp::max(shard1.descr.next_catchain_seqno, shard2.descr.next_catchain_seqno) + 1)
    }
    pub fn split_shard(
        &mut self,
        splitted_shard: &ShardIdent,
        splitter: impl FnOnce(ShardDescr) -> Result<(ShardDescr, ShardDescr)>,
    ) -> Result<()> {
        let mut tree = self
            .get(&splitted_shard.workchain_id())?
            .ok_or_else(|| error!("Can't find workchain {}", splitted_shard.workchain_id()))?;
        if !tree.0.split(splitted_shard.shard_key(false), splitter)? {
            fail!("Splitted shard {} is not found", splitted_shard)
        } else {
            self.set(&splitted_shard.workchain_id(), &tree)
        }
    }
    pub fn merge_shards(
        &mut self,
        new_shard: &ShardIdent,
        merger: impl FnOnce(ShardDescr, ShardDescr) -> Result<ShardDescr>,
    ) -> Result<()> {
        let mut tree = self
            .get(&new_shard.workchain_id())?
            .ok_or_else(|| error!("Can't find workchain {}", new_shard.workchain_id()))?;
        if !tree.0.merge(new_shard.shard_key(false), merger)? {
            fail!("Merged shards's parent {} is not found", new_shard)
        } else {
            self.set(&new_shard.workchain_id(), &tree)
        }
    }
    pub fn update_shard(
        &mut self,
        shard: &ShardIdent,
        mutator: impl FnOnce(ShardDescr) -> Result<ShardDescr>,
    ) -> Result<()> {
        let mut tree = self
            .get(&shard.workchain_id())?
            .ok_or_else(|| error!("Can't find workchain {}", shard.workchain_id()))?;
        if !tree.0.update(shard.shard_key(false), mutator)? {
            fail!("Updated shard {} is not found", shard)
        } else {
            self.set(&shard.workchain_id(), &tree)
        }
    }
    pub fn add_workchain(
        &mut self,
        workchain_id: i32,
        reg_mc_seqno: u32,
        zerostate_root_hash: UInt256,
        zerostate_file_hash: UInt256,
    ) -> Result<()> {
        if self.contains(&workchain_id)? {
            fail!("Workchain {} is already added", workchain_id);
        }

        let descr = ShardDescr {
            reg_mc_seqno,
            root_hash: zerostate_root_hash,
            file_hash: zerostate_file_hash,
            next_validator_shard: SHARD_FULL,
            min_ref_mc_seqno: u32::MAX,
            ..ShardDescr::default()
        };
        let tree = BinTree::with_item(&descr)?;

        self.set(&workchain_id, &InRefValue(tree))
    }
}

impl ShardHashes {
    pub fn dump(&self, heading: &str) -> usize {
        let mut count = 0;
        println!("dumping shard records for: {}", heading);
        self.iterate_with_keys(|workchain_id: i32, InRefValue(bintree)| {
            println!("workchain: {}", workchain_id);
            bintree.iterate(|prefix, descr| {
                let shard = ShardIdent::with_prefix_slice(workchain_id, prefix)?;
                println!(
                    "shard: {:064b} seq_no: {} shard: 0x{}",
                    shard.shard_prefix_with_tag(),
                    descr.seq_no,
                    shard.shard_prefix_as_str_with_tag()
                );
                count += 1;
                Ok(true)
            })
        })
        .unwrap();
        count
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct McShardRecord {
    pub descr: ShardDescr,
    pub block_id: BlockIdExt,
}

impl McShardRecord {
    pub fn from_shard_descr(shard: ShardIdent, descr: ShardDescr) -> Self {
        let block_id = BlockIdExt::with_params(
            shard,
            descr.seq_no,
            descr.root_hash.clone(),
            descr.file_hash.clone(),
        );
        Self { descr, block_id }
    }

    pub fn from_block(block: &Block, block_id: BlockIdExt) -> Result<Self> {
        let info = block.read_info()?;
        let value_flow = block.read_value_flow()?;
        Ok(McShardRecord {
            descr: ShardDescr {
                seq_no: info.seq_no(),
                reg_mc_seqno: 0xffff_ffff, // by t-node
                start_lt: info.start_lt(),
                end_lt: info.end_lt(),
                root_hash: block_id.root_hash().clone(),
                file_hash: block_id.file_hash().clone(),
                before_split: info.before_split(),
                before_merge: false, // by t-node
                want_split: info.want_split(),
                want_merge: info.want_merge(),
                nx_cc_updated: false, // by t-node
                flags: info.flags() & !7,
                next_catchain_seqno: info.gen_catchain_seqno(),
                next_validator_shard: info.shard().shard_prefix_with_tag(),
                min_ref_mc_seqno: info.min_ref_mc_seqno(),
                gen_utime: info.gen_utime(),
                split_merge_at: FutureSplitMerge::None, // is not used in McShardRecord
                fees_collected: value_flow.fees_collected,
                funds_created: value_flow.created,
            },
            block_id,
        })
    }

    pub fn shard(&self) -> &ShardIdent {
        self.block_id.shard()
    }

    pub fn descr(&self) -> &ShardDescr {
        &self.descr
    }

    // to be deleted
    pub fn blk_id(&self) -> &BlockIdExt {
        &self.block_id
    }

    pub fn block_id(&self) -> &BlockIdExt {
        &self.block_id
    }

    pub fn basic_info_equal(
        &self,
        other: &Self,
        compare_fees: bool,
        compare_reg_seqno: bool,
    ) -> bool {
        self.block_id == other.block_id
            && self.descr.start_lt == other.descr.start_lt
            && self.descr.end_lt == other.descr.end_lt
            && (!compare_reg_seqno || self.descr.reg_mc_seqno == other.descr.reg_mc_seqno)
            && self.descr.gen_utime == other.descr.gen_utime
            && self.descr.min_ref_mc_seqno == other.descr.min_ref_mc_seqno
            && self.descr.before_split == other.descr.before_split
            && self.descr.want_split == other.descr.want_split
            && self.descr.want_merge == other.descr.want_merge
            && (!compare_fees
                || (self.descr.fees_collected == other.descr.fees_collected
                    && self.descr.funds_created == other.descr.funds_created))
    }
}

impl ShardFees {
    pub fn store_shard_fees(
        &mut self,
        shard: &ShardIdent,
        fees: CurrencyCollection,
        created: CurrencyCollection,
    ) -> Result<()> {
        let id = ShardIdentFull {
            workchain_id: shard.workchain_id(),
            prefix: shard.shard_prefix_with_tag(),
        };
        let fee = ShardFeeCreated { fees, create: created };
        self.set(&id, &fee, &fee)?;
        Ok(())
    }
}

/*
masterchain_block_extra#cca5
  key_block:(## 1)
  shard_hashes:ShardHashes
  shard_fees:ShardFees
  ^[ prev_blk_signatures:(HashmapE 16 CryptoSignaturePair)
     recover_create_msg:(Maybe ^InMsg)
     mint_msg:(Maybe ^InMsg) ]
  config:key_block?ConfigParams
= McBlockExtra;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McBlockExtra {
    shards: ShardHashes, // workchain_id of ShardIdent from all blocks
    fees: ShardFees,
    prev_blk_signatures: CryptoSignatures,
    recover_create_msg: Option<ChildCell<InMsg>>,
    mint_msg: Option<ChildCell<InMsg>>,
    config: Option<ConfigParams>,
}

impl McBlockExtra {
    ///
    /// Get all fees for blockchain
    ///
    pub fn total_fee(&self) -> &CurrencyCollection {
        &self.fees.root_extra().fees
    }

    ///
    /// Get total fees for shard
    ///
    pub fn fee(&self, ident: &ShardIdent) -> Result<Option<CurrencyCollection>> {
        Ok(match self.fees.get_serialized(ident.full_key()?)? {
            Some(shards) => Some(shards.fees),
            None => None,
        })
    }

    pub fn is_key_block(&self) -> bool {
        self.config.is_some()
    }

    pub fn hashes(&self) -> &ShardHashes {
        &self.shards
    }
    pub fn hashes_mut(&mut self) -> &mut ShardHashes {
        &mut self.shards
    }

    pub fn shards(&self) -> &ShardHashes {
        &self.shards
    }
    pub fn shards_mut(&mut self) -> &mut ShardHashes {
        &mut self.shards
    }

    pub fn fees(&self) -> &ShardFees {
        &self.fees
    }
    pub fn fees_mut(&mut self) -> &mut ShardFees {
        &mut self.fees
    }

    pub fn prev_blk_signatures(&self) -> &CryptoSignatures {
        &self.prev_blk_signatures
    }
    pub fn prev_blk_signatures_mut(&mut self) -> &mut CryptoSignatures {
        &mut self.prev_blk_signatures
    }

    pub fn config(&self) -> Option<&ConfigParams> {
        self.config.as_ref()
    }
    pub fn config_mut(&mut self) -> &mut Option<ConfigParams> {
        &mut self.config
    }
    pub fn set_config(&mut self, config: ConfigParams) {
        self.config = Some(config)
    }

    pub fn read_recover_create_msg(&self) -> Result<Option<InMsg>> {
        self.recover_create_msg.as_ref().map(|mr| mr.read_struct()).transpose()
    }
    pub fn write_recover_create_msg(&mut self, msg: &InMsg) -> Result<()> {
        self.recover_create_msg = Some(ChildCell::with_struct(msg)?);
        Ok(())
    }
    pub fn recover_create_msg_cell(&self) -> Option<Cell> {
        self.recover_create_msg.as_ref().map(|mr| mr.cell())
    }

    pub fn read_mint_msg(&self) -> Result<Option<InMsg>> {
        self.mint_msg.as_ref().map(ChildCell::read_struct).transpose()
    }
    pub fn write_mint_msg(&mut self, msg: &InMsg) -> Result<()> {
        self.mint_msg = Some(ChildCell::with_struct(msg)?);
        Ok(())
    }
    pub fn mint_msg_cell(&self) -> Option<Cell> {
        self.mint_msg.as_ref().map(|mr| mr.cell())
    }
}

const MC_BLOCK_EXTRA_TAG: u16 = 0xCCA5; // Original struct.

impl Deserializable for McBlockExtra {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_u16()?;
        if tag != MC_BLOCK_EXTRA_TAG {
            fail!(BlockError::InvalidConstructorTag {
                t: tag.into(),
                s: std::any::type_name::<Self>().to_string()
            })
        }
        let key_block = cell.get_next_bit()?;
        self.shards.read_from(cell)?;
        self.fees.read_from(cell)?;

        let cell1 = &mut SliceData::load_cell(cell.checked_drain_reference()?)?;
        self.prev_blk_signatures.read_from(cell1)?;
        self.recover_create_msg.read_from(cell1)?;
        self.mint_msg.read_from(cell1)?;

        self.config = if key_block { Some(ConfigParams::construct_from(cell)?) } else { None };

        Ok(())
    }
}

impl Serializable for McBlockExtra {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        let tag = MC_BLOCK_EXTRA_TAG;
        cell.append_u16(tag)?;
        self.config.is_some().write_to(cell)?;
        self.shards.write_to(cell)?;
        self.fees.write_to(cell)?;

        let mut cell1 = self.prev_blk_signatures.write_to_new_cell()?;
        self.recover_create_msg.write_to(&mut cell1)?;
        self.mint_msg.write_to(&mut cell1)?;

        cell.checked_append_reference(cell1.into_cell()?)?;

        if let Some(config) = &self.config {
            config.write_to(cell)?;
        }

        Ok(())
    }
}

// _ key:Bool max_end_lt:uint64 = KeyMaxLt;
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct KeyMaxLt {
    pub key: bool,
    pub max_end_lt: u64,
}

impl Deserializable for KeyMaxLt {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.key.read_from(slice)?;
        self.max_end_lt.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for KeyMaxLt {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.key.write_to(cell)?;
        self.max_end_lt.write_to(cell)?;
        Ok(())
    }
}

impl Augmentable for KeyMaxLt {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        if other.key {
            self.key = true
        }
        if self.max_end_lt < other.max_end_lt {
            self.max_end_lt = other.max_end_lt
        }
        Ok(true)
    }
}

// _ key:Bool blk_ref:ExtBlkRef = KeyExtBlkRef;
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct KeyExtBlkRef {
    pub key: bool,
    pub blk_ref: ExtBlkRef,
}

impl KeyExtBlkRef {
    pub fn fake(key: bool, seq_no: u32, end_lt: u64) -> Self {
        KeyExtBlkRef { key, blk_ref: ExtBlkRef::fake(seq_no, end_lt) }
    }
    pub fn key(&self) -> bool {
        self.key
    }
    pub fn blk_ref(&self) -> &ExtBlkRef {
        &self.blk_ref
    }
    pub fn master_block_id(self) -> (u64, BlockIdExt, bool) {
        (self.blk_ref.end_lt, BlockIdExt::from_ext_blk(self.blk_ref), self.key)
    }
}

impl Deserializable for KeyExtBlkRef {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.key.read_from(slice)?;
        self.blk_ref.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for KeyExtBlkRef {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.key.write_to(cell)?;
        self.blk_ref.write_to(cell)?;
        Ok(())
    }
}

impl Augmentation<KeyMaxLt> for KeyExtBlkRef {
    fn aug(&self) -> Result<KeyMaxLt> {
        Ok(KeyMaxLt { key: self.key, max_end_lt: self.blk_ref.end_lt })
    }
}

// _ (HashmapAugE 32 KeyExtBlkRef KeyMaxLt) = OldMcBlocksInfo;
// key - seq_no
define_HashmapAugE!(OldMcBlocksInfo, 32, u32, KeyExtBlkRef, KeyMaxLt);

impl OldMcBlocksInfo {
    pub fn add_fake_id(&mut self, seq_no: u32, key: bool) -> Result<()> {
        let end_lt = seq_no as u64 * 10000;
        let blk_ref = ExtBlkRef {
            seq_no,
            end_lt,
            root_hash: UInt256::from_be_bytes(&[1]),
            file_hash: UInt256::from_be_bytes(&[2]),
        };
        let ext_ref = KeyExtBlkRef { key, blk_ref };
        self.set_augmentable(&seq_no, &ext_ref)?;
        Ok(())
    }
    // returns key block with max block.seqno and block.seqno <= req_seqno
    pub fn get_prev_key_block(&self, req_seqno: u32) -> Result<Option<ExtBlkRef>> {
        let found = self.traverse(|key_prefix, key_prefix_len, aug, value_opt| {
            if !aug.key {
                // no key blocks in subtree, skip
                return Ok(TraverseNextStep::Stop);
            }

            let x = Self::build_key_part(key_prefix, key_prefix_len)?;
            let d = 32 - key_prefix_len;
            if d == 0 {
                return if x <= req_seqno {
                    let value = value_opt.ok_or_else(|| {
                        error!(BlockError::InvalidData(
                            "OldMcBlocksInfo's node with max key length doesn't have value"
                                .to_string()
                        ))
                    })?;
                    Ok(TraverseNextStep::End(value))
                } else {
                    Ok(TraverseNextStep::Stop)
                };
            }
            let y = req_seqno >> (d - 1);
            match y.cmp(&(2 * x)) {
                std::cmp::Ordering::Less => {
                    // (x << d) > req_seqno <=> x > (req_seqno >> d) = (y >> 1) <=> 2 * x > y
                    Ok(TraverseNextStep::Stop) // all nodes in subtree have block.seqno > req_seqno => skip
                }
                std::cmp::Ordering::Equal => {
                    Ok(TraverseNextStep::VisitZero) // visit only left ("0")
                }
                _ => {
                    Ok(TraverseNextStep::VisitOneZero) // visit right, then left ("1" then "0")
                }
            }
        })?;

        if let Some(id) = found {
            debug_assert!(id.blk_ref.seq_no <= req_seqno);
            debug_assert!(id.key);
            Ok(Some(id.blk_ref))
        } else {
            Ok(None)
        }
    }

    // returns key block with min block.seqno and block.seqno >= req_seqno
    pub fn get_next_key_block(&self, req_seqno: u32) -> Result<Option<ExtBlkRef>> {
        let found = self.traverse(|key_prefix, key_prefix_len, aug, value_opt| {
            if !aug.key {
                // no key blocks in subtree, skip
                return Ok(TraverseNextStep::Stop);
            }

            let x = Self::build_key_part(key_prefix, key_prefix_len)?;
            let d = 32 - key_prefix_len;
            if d == 0 {
                return if x >= req_seqno {
                    let value = value_opt.ok_or_else(|| {
                        error!(BlockError::InvalidData(
                            "OldMcBlocksInfo's node with max key length doesn't have value"
                                .to_string()
                        ))
                    })?;
                    Ok(TraverseNextStep::End(value))
                } else {
                    Ok(TraverseNextStep::Stop)
                };
            }
            let y = req_seqno >> (d - 1);
            match y.cmp(&(2 * x + 1)) {
                std::cmp::Ordering::Greater => {
                    // ((x + 1) << d) <= req_seqno <=> (x+1) <= (req_seqno >> d) = (y >> 1) <=> 2*x+2 <= y <=> y > 2*x+1
                    Ok(TraverseNextStep::Stop) // all nodes in subtree have block.seqno < req_seqno => skip
                }
                std::cmp::Ordering::Equal => {
                    Ok(TraverseNextStep::VisitOne) // visit only right ("1")
                }
                _ => {
                    Ok(TraverseNextStep::VisitZeroOne) // visit left, then right ("0" then "1")
                }
            }
        })?;

        if let Some(id) = found {
            debug_assert!(id.blk_ref.seq_no >= req_seqno);
            debug_assert!(id.key);
            Ok(Some(id.blk_ref))
        } else {
            Ok(None)
        }
    }

    pub fn check_block(&self, id: &BlockIdExt) -> Result<()> {
        self.check_key_block(id, None)
    }

    pub fn check_key_block(&self, id: &BlockIdExt, is_key_opt: Option<bool>) -> Result<()> {
        if !id.shard().is_masterchain() {
            fail!(BlockError::InvalidData("Given id does not belong masterchain".to_string()));
        }
        let found_id = self
            .get(&id.seq_no())?
            .ok_or_else(|| error!("Block with given seq_no {} is not found", id.seq_no()))?;

        if found_id.blk_ref.root_hash != *id.root_hash() {
            fail!(
                "Given block has invalid root hash: found {:x}, expected {:x}",
                found_id.blk_ref.root_hash,
                id.root_hash()
            )
        }
        if found_id.blk_ref.file_hash != *id.file_hash() {
            fail!(
                "Given block has invalid file hash: found {:x}, expected {:x}",
                found_id.blk_ref.file_hash,
                id.file_hash()
            )
        }
        if let Some(is_key) = is_key_opt {
            if is_key != found_id.key {
                fail!("Given block has key flag set to: {}, expected {}", found_id.key, is_key)
            }
        }
        Ok(())
    }

    fn build_key_part(key_prefix: &[u8], key_prefix_len: usize) -> Result<u32> {
        if key_prefix_len > 32 {
            fail!(BlockError::InvalidData("key_prefix_len > 32".to_string()));
        }
        let mut key_buf = [0_u8; 4];
        key_buf[..key_prefix.len()].copy_from_slice(key_prefix);
        Ok(u32::from_be_bytes(key_buf) >> (32 - key_prefix_len))
    }
}

// _ fees:CurrencyCollection create:CurrencyCollection = ShardFeeCreated;
#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct ShardFeeCreated {
    pub fees: CurrencyCollection,
    pub create: CurrencyCollection,
}

impl ShardFeeCreated {
    pub fn with_fee(fees: CurrencyCollection) -> Self {
        Self { fees, create: CurrencyCollection::default() }
    }
}

impl Augmentable for ShardFeeCreated {
    fn calc(&mut self, other: &Self) -> Result<bool> {
        let mut result = self.fees.calc(&other.fees)?;
        result |= self.create.calc(&other.create)?;
        Ok(result)
    }
}

impl Deserializable for ShardFeeCreated {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.fees.read_from(cell)?;
        self.create.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for ShardFeeCreated {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.fees.write_to(cell)?;
        self.create.write_to(cell)?;
        Ok(())
    }
}

pub fn umulnexps32(x: u64, k: u32, _trunc: bool) -> u64 {
    (
        (x as f64 * (k as f64 / -65536f64).exp()) // x * exp(-k / 2^16)
        + 0.5f64
        // Need to round up the number to the nearest integer
    ) as u64
}

/// counters#_ last_updated:uint32 total:uint64 cnt2048:uint64 cnt65536:uint64 = Counters;
#[derive(Clone, Debug, Default, Eq)]
pub struct Counters {
    last_updated: u32,
    total: u64,
    cnt2048: u64,
    cnt65536: u64,
}

impl PartialEq for Counters {
    fn eq(&self, other: &Self) -> bool {
        self.last_updated == other.last_updated
            && self.total == other.total
            && self.cnt2048 == other.cnt2048
            && self.cnt65536 == other.cnt65536
    }
}

impl Counters {
    pub fn is_valid(&self) -> bool {
        if self.total == 0 {
            if (self.cnt2048 | self.cnt65536) != 0 {
                return false;
            }
        } else if self.last_updated == 0 {
            return false;
        }
        true
    }
    pub fn is_zero(&self) -> bool {
        self.total == 0
    }
    pub fn almost_zero(&self) -> bool {
        (self.cnt2048 | self.cnt65536) <= 1
    }
    pub fn almost_equals(&self, other: &Self) -> bool {
        self.last_updated == other.last_updated
            && self.total == other.total
            && self.cnt2048 <= other.cnt2048 + 1
            && other.cnt2048 <= self.cnt2048 + 1
            && self.cnt65536 <= other.cnt65536 + 1
            && other.cnt65536 <= self.cnt65536 + 1
    }
    pub fn modified_since(&self, utime: u32) -> bool {
        self.last_updated >= utime
    }
    pub fn increase_by(&mut self, count: u64, now: u32) -> bool {
        if !self.is_valid() {
            return false;
        }
        let scaled = count << 32;
        if self.total == 0 {
            self.last_updated = now;
            self.total = count;
            self.cnt2048 = scaled;
            self.cnt65536 = scaled;
            return true;
        }
        if count > !self.total || self.cnt2048 > !scaled || self.cnt65536 > !scaled {
            return false;
        }
        let dt = now.checked_sub(self.last_updated).unwrap_or_default();
        if dt != 0 {
            // more precise version of cnt2048 = llround(cnt2048 * exp(-dt / 2048.));
            // (rounding error has absolute value < 1)
            self.cnt2048 =
                if dt >= 48 * 2048 { 0 } else { umulnexps32(self.cnt2048, dt << 5, false) };
            // more precise version of cnt65536 = llround(cnt65536 * exp(-dt / 65536.));
            // (rounding error has absolute value < 1)
            self.cnt65536 = umulnexps32(self.cnt65536, dt, false);
        }
        self.total += count;
        self.cnt2048 += scaled;
        self.cnt65536 += scaled;
        self.last_updated = now;
        true
    }
    pub fn total(&self) -> u64 {
        self.total
    }
    pub fn last_updated(&self) -> u32 {
        self.last_updated
    }
    pub fn cnt2048(&self) -> u64 {
        self.cnt2048
    }
    pub fn cnt65536(&self) -> u64 {
        self.cnt65536
    }
}

impl Deserializable for Counters {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        self.last_updated.read_from(slice)?;
        self.total.read_from(slice)?;
        self.cnt2048.read_from(slice)?;
        self.cnt65536.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for Counters {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.last_updated.write_to(cell)?;
        self.total.write_to(cell)?;
        self.cnt2048.write_to(cell)?;
        self.cnt65536.write_to(cell)?;
        Ok(())
    }
}

/// creator_info#4 mc_blocks:Counters shard_blocks:Counters = CreatorStats;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CreatorStats {
    pub mc_blocks: Counters,
    pub shard_blocks: Counters,
}

impl CreatorStats {
    pub fn tag() -> u32 {
        0x4
    }

    pub fn tag_len_bits() -> usize {
        4
    }

    pub fn mc_blocks(&self) -> &Counters {
        &self.mc_blocks
    }

    pub fn shard_blocks(&self) -> &Counters {
        &self.shard_blocks
    }
}

impl Deserializable for CreatorStats {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_int(Self::tag_len_bits())? as u32;
        if tag != Self::tag() {
            fail!(BlockError::InvalidConstructorTag {
                t: tag,
                s: std::any::type_name::<Self>().to_string()
            })
        }

        self.mc_blocks.read_from(slice)?;
        self.shard_blocks.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for CreatorStats {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(Self::tag() as usize, Self::tag_len_bits())?;

        self.mc_blocks.write_to(cell)?;
        self.shard_blocks.write_to(cell)?;
        Ok(())
    }
}

define_HashmapE! {BlockCounters, 256, CreatorStats}

/// block_create_stats#17 counters:(HashmapE 256 CreatorStats) = BlockCreateStats;
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlockCreateStats {
    pub counters: BlockCounters,
}

impl BlockCreateStats {
    pub fn tag() -> u32 {
        0x17
    }

    pub fn tag_len_bits() -> usize {
        8
    }
}

impl Deserializable for BlockCreateStats {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_int(Self::tag_len_bits())? as u32;
        if tag != Self::tag() {
            fail!(BlockError::InvalidConstructorTag {
                t: tag,
                s: std::any::type_name::<Self>().to_string()
            })
        }

        self.counters.read_from(slice)?;
        Ok(())
    }
}

impl Serializable for BlockCreateStats {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_bits(Self::tag() as usize, Self::tag_len_bits())?;

        self.counters.write_to(cell)?;
        Ok(())
    }
}

/*
masterchain_state_extra#cc26
  shard_hashes:ShardHashes
  config:ConfigParams
  ^[ flags:(## 16) { flags <= 1 }
     validator_info:ValidatorInfo
     prev_blocks:OldMcBlocksInfo
     after_key_block:Bool
     last_key_block:(Maybe ExtBlkRef)
     block_create_stats:(flags . 0)?BlockCreateStats ]
  global_balance:CurrencyCollection
= McStateExtra;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct McStateExtra {
    pub shards: ShardHashes,
    pub config: ConfigParams,
    pub validator_info: ValidatorInfo,
    pub prev_blocks: OldMcBlocksInfo,
    pub after_key_block: bool,
    pub last_key_block: Option<ExtBlkRef>,
    pub block_create_stats: Option<BlockCreateStats>,
    pub global_balance: CurrencyCollection,
}

const MC_STATE_EXTRA_TAG: u16 = 0xcc26;
const MC_STATE_CREATE_STATS_FLAG: u16 = 0b0001;

impl McStateExtra {
    /// Adds new workchain
    pub fn add_workchain(&mut self, workchain_id: i32, descr: &ShardDescr) -> Result<ShardIdent> {
        let shards = BinTree::with_item(descr)?;
        self.shards.set(&workchain_id, &InRefValue(shards))?;
        ShardIdent::with_workchain_id(workchain_id)
    }

    ///
    /// Get Shard last seq_no
    ///
    pub fn shard_seq_no(&self, ident: &ShardIdent) -> Result<Option<u32>> {
        Ok(match self.shards.get(&ident.workchain_id())? {
            Some(InRefValue(shards)) => shards.get(ident.shard_key(false))?.map(|s| s.seq_no),
            None => None,
        })
    }

    ///
    /// Get shard last Logical Time
    ///
    pub fn shard_lt(&self, ident: &ShardIdent) -> Result<Option<u64>> {
        Ok(match self.shards.get(&ident.workchain_id())? {
            Some(InRefValue(shards)) => shards.get(ident.shard_key(false))?.map(|s| s.start_lt),
            None => None,
        })
    }

    ///
    /// Get shard last block hash
    ///
    pub fn shard_hash(&self, ident: &ShardIdent) -> Result<Option<UInt256>> {
        Ok(match self.shards.get(&ident.workchain_id())? {
            Some(InRefValue(shards)) => shards.get(ident.shard_key(false))?.map(|s| s.root_hash),
            None => None,
        })
    }

    pub fn shards(&self) -> &ShardHashes {
        &self.shards
    }
    pub fn config(&self) -> &ConfigParams {
        &self.config
    }
}

impl Deserializable for McStateExtra {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_u16()?;
        if tag != MC_STATE_EXTRA_TAG {
            fail!(BlockError::InvalidConstructorTag {
                t: tag.into(),
                s: std::any::type_name::<Self>().to_string()
            })
        }
        self.shards.read_from(cell)?;
        self.config.read_from(cell)?;

        let cell1 = &mut SliceData::load_cell(cell.checked_drain_reference()?)?;
        let mut flags = 0u16;
        flags.read_from(cell1)?; // 16 + 0
        if flags > 1 {
            fail!(BlockError::InvalidData(format!(
                "Invalid flags value ({}). Must be <= 1.",
                flags
            )))
        }
        self.validator_info.read_from(cell1)?; // 65 + 0
        self.prev_blocks.read_from(cell1)?; // 1 + 1
        self.after_key_block.read_from(cell1)?; // 1 + 0
        self.last_key_block.read_from(cell1)?; // 609 + 0
        self.block_create_stats = if flags & MC_STATE_CREATE_STATS_FLAG == 0 {
            None
        } else {
            Some(BlockCreateStats::construct_from(cell1)?) // 1 + 1
        };
        self.global_balance.read_from(cell)?;
        Ok(())
    }
}

impl Serializable for McStateExtra {
    fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        builder.append_u16(MC_STATE_EXTRA_TAG)?;
        self.shards.write_to(builder)?;
        self.config.write_to(builder)?;

        let mut builder1 = BuilderData::new();
        let mut flags = 0;
        if self.block_create_stats.is_some() {
            flags |= MC_STATE_CREATE_STATS_FLAG;
        }
        flags.write_to(&mut builder1)?;
        self.validator_info.write_to(&mut builder1)?;
        self.prev_blocks.write_to(&mut builder1)?;
        self.after_key_block.write_to(&mut builder1)?;
        self.last_key_block.write_to(&mut builder1)?;
        if let Some(ref block_create_stats) = self.block_create_stats {
            block_create_stats.write_to(&mut builder1)?;
        }
        builder.checked_append_reference(builder1.into_cell()?)?;

        self.global_balance.write_to(builder)?;
        Ok(())
    }
}

/*
fsm_none$0

fsm_split$10
    split_utime: uint32
    interval: uint32
= FutureSplitMerge;

fsm_merge$11
    merge_utime: uint32
    interval: uint32
= FutureSplitMerge;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FutureSplitMerge {
    #[default]
    None,
    Split {
        split_utime: u32,
        interval: u32,
    },
    Merge {
        merge_utime: u32,
        interval: u32,
    },
}

impl Deserializable for FutureSplitMerge {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        if !slice.get_next_bit()? {
            *self = FutureSplitMerge::None;
        } else if !slice.get_next_bit()? {
            *self = FutureSplitMerge::Split {
                split_utime: slice.get_next_u32()?,
                interval: slice.get_next_u32()?,
            };
        } else {
            *self = FutureSplitMerge::Merge {
                merge_utime: slice.get_next_u32()?,
                interval: slice.get_next_u32()?,
            };
        }
        Ok(())
    }
}

impl Serializable for FutureSplitMerge {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        match self {
            FutureSplitMerge::None => {
                cell.append_bit_zero()?;
            }
            FutureSplitMerge::Split { split_utime, interval } => {
                cell.append_bit_one()?;
                cell.append_bit_zero()?;
                split_utime.write_to(cell)?;
                interval.write_to(cell)?;
            }
            FutureSplitMerge::Merge { merge_utime, interval } => {
                cell.append_bit_one()?;
                cell.append_bit_one()?;
                merge_utime.write_to(cell)?;
                interval.write_to(cell)?;
            }
        }
        Ok(())
    }
}

// Shard description (header)
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct ShardDescr {
    pub seq_no: u32,
    pub reg_mc_seqno: u32,
    pub start_lt: u64,
    pub end_lt: u64,
    pub root_hash: UInt256,
    pub file_hash: UInt256,
    pub before_split: bool,
    pub before_merge: bool,
    pub want_split: bool,
    pub want_merge: bool,
    pub nx_cc_updated: bool,
    pub flags: u8,
    pub next_catchain_seqno: u32,
    pub next_validator_shard: u64,
    pub min_ref_mc_seqno: u32,
    pub gen_utime: u32,
    pub split_merge_at: FutureSplitMerge,
    pub fees_collected: CurrencyCollection,
    pub funds_created: CurrencyCollection,
}

impl ShardDescr {
    /// Constructs ShardDescr as slice with its params
    pub fn with_params(
        seq_no: u32,
        start_lt: u64,
        end_lt: u64,
        root_hash: UInt256,
        split_merge_at: FutureSplitMerge,
    ) -> Self {
        ShardDescr {
            seq_no,
            reg_mc_seqno: 0,
            start_lt,
            end_lt,
            root_hash,
            file_hash: UInt256::ZERO,
            before_split: false,
            before_merge: false,
            want_split: false,
            want_merge: false,
            nx_cc_updated: false,
            flags: 0,
            next_catchain_seqno: 0,
            next_validator_shard: 0,
            min_ref_mc_seqno: 0,
            gen_utime: 0,
            split_merge_at,
            fees_collected: CurrencyCollection::default(),
            funds_created: CurrencyCollection::default(),
        }
    }
    pub fn fsm_equal(&self, other: &Self) -> bool {
        self.split_merge_at == other.split_merge_at
    }
    pub fn is_fsm_merge(&self) -> bool {
        matches!(self.split_merge_at, FutureSplitMerge::Merge { merge_utime: _, interval: _ })
    }
    pub fn is_fsm_split(&self) -> bool {
        matches!(self.split_merge_at, FutureSplitMerge::Split { split_utime: _, interval: _ })
    }
    pub fn is_fsm_none(&self) -> bool {
        matches!(self.split_merge_at, FutureSplitMerge::None)
    }
    pub fn fsm_utime(&self) -> u32 {
        match self.split_merge_at {
            FutureSplitMerge::Split { split_utime, interval: _ } => split_utime,
            FutureSplitMerge::Merge { merge_utime, interval: _ } => merge_utime,
            _ => 0,
        }
    }
    pub fn fsm_utime_end(&self) -> u32 {
        match self.split_merge_at {
            FutureSplitMerge::Split { split_utime, interval } => split_utime + interval,
            FutureSplitMerge::Merge { merge_utime, interval } => merge_utime + interval,
            _ => 0,
        }
    }
    pub fn fsm_interval(&self) -> u32 {
        match self.split_merge_at {
            FutureSplitMerge::Split { split_utime: _, interval } => interval,
            FutureSplitMerge::Merge { merge_utime: _, interval } => interval,
            _ => 0,
        }
    }
}

const SHARD_IDENT_TAG_A: u8 = 0xa; // 4 bit
const SHARD_IDENT_TAG_B: u8 = 0xb; // 4 bit
const SHARD_IDENT_TAG_LEN: usize = 4;

impl Deserializable for ShardDescr {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_int(SHARD_IDENT_TAG_LEN)? as u8;
        if tag != SHARD_IDENT_TAG_A && tag != SHARD_IDENT_TAG_B {
            fail!(BlockError::InvalidConstructorTag {
                t: tag as u32,
                s: std::any::type_name::<Self>().to_string()
            })
        }

        self.seq_no.read_from(slice)?;
        self.reg_mc_seqno.read_from(slice)?;
        self.start_lt.read_from(slice)?;
        self.end_lt.read_from(slice)?;
        self.root_hash.read_from(slice)?;
        self.file_hash.read_from(slice)?;
        let mut flags: u8 = 0;
        flags.read_from(slice)?;
        self.before_split = (flags >> 7) & 1 == 1;
        self.before_merge = (flags >> 6) & 1 == 1;
        self.want_split = (flags >> 5) & 1 == 1;
        self.want_merge = (flags >> 4) & 1 == 1;
        self.nx_cc_updated = (flags >> 3) & 1 == 1;

        if (flags & 7) != 0 {
            fail!("flags & 7 in ShardDescr must be zero, but {}", flags)
        }

        self.next_catchain_seqno.read_from(slice)?;
        self.next_validator_shard.read_from(slice)?;
        self.min_ref_mc_seqno.read_from(slice)?;
        self.gen_utime.read_from(slice)?;
        self.split_merge_at.read_from(slice)?;
        match tag {
            SHARD_IDENT_TAG_B => {
                self.fees_collected.read_from(slice)?;
                self.funds_created.read_from(slice)?;
            }
            SHARD_IDENT_TAG_A => {
                let mut slice1 = SliceData::load_cell(slice.checked_drain_reference()?)?;
                self.fees_collected.read_from(&mut slice1)?;
                self.funds_created.read_from(&mut slice1)?;
            }
            _ => unreachable!(),
        }

        Ok(())
    }
}

impl Serializable for ShardDescr {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        // TAG_B is not used at all.
        cell.append_bits(SHARD_IDENT_TAG_A as usize, SHARD_IDENT_TAG_LEN)?;

        self.seq_no.write_to(cell)?;
        self.reg_mc_seqno.write_to(cell)?;
        self.start_lt.write_to(cell)?;
        self.end_lt.write_to(cell)?;
        self.root_hash.write_to(cell)?;
        self.file_hash.write_to(cell)?;

        let mut flags: u8 = 0;
        if self.before_split {
            flags |= 1 << 7
        }
        if self.before_merge {
            flags |= 1 << 6;
        }
        if self.want_split {
            flags |= 1 << 5;
        }
        if self.want_merge {
            flags |= 1 << 4;
        }
        if self.nx_cc_updated {
            flags |= 1 << 3;
        }
        if (self.flags & 7) != 0 {
            fail!("flags & 7 must be zero, but it {}", self.flags)
        }

        flags.write_to(cell)?;

        self.next_catchain_seqno.write_to(cell)?;
        self.next_validator_shard.write_to(cell)?;
        self.min_ref_mc_seqno.write_to(cell)?;
        self.gen_utime.write_to(cell)?;
        self.split_merge_at.write_to(cell)?;

        let mut child = BuilderData::new();
        self.fees_collected.write_to(&mut child)?;
        self.funds_created.write_to(&mut child)?;
        cell.checked_append_reference(child.into_cell()?)?;

        Ok(())
    }
}

/*
master_info$_ master:ExtBlkRef = BlkMasterInfo;
*/
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct BlkMasterInfo {
    pub master: ExtBlkRef,
}

impl Deserializable for BlkMasterInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.master.read_from(cell)
    }
}

impl Serializable for BlkMasterInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        self.master.write_to(cell)
    }
}

define_HashmapE!(Publishers, 256, ());
/*
shared_lib_descr$00 lib:^Cell publishers:(Hashmap 256 True) = LibDescr;
*/
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibDescr {
    lib: Cell,
    publishers: Publishers,
}

impl LibDescr {
    pub fn new(lib: Cell) -> Self {
        Self { lib, publishers: Publishers::default() }
    }
    pub fn from_lib_data_by_publisher(lib: Cell, publisher: AccountId) -> Self {
        let mut publishers = Publishers::default();
        publishers.set(&publisher, &()).unwrap();
        Self { lib, publishers }
    }
    pub fn publishers(&self) -> &Publishers {
        &self.publishers
    }
    pub fn publishers_mut(&mut self) -> &mut Publishers {
        &mut self.publishers
    }
    pub fn lib(&self) -> &Cell {
        &self.lib
    }
}

impl Deserializable for LibDescr {
    fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        let tag = slice.get_next_int(2)?;
        if tag != 0 {
            fail!(BlockError::InvalidConstructorTag {
                t: tag as u32,
                s: std::any::type_name::<Self>().to_string()
            })
        }
        self.lib.read_from(slice)?;
        self.publishers.read_hashmap_root(slice)?;
        Ok(())
    }
}

impl Serializable for LibDescr {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if self.publishers.is_empty() {
            fail!(BlockError::InvalidData("self.publishers is empty".to_string()))
        }
        cell.append_bits(0, 2)?;
        self.lib.write_to(cell)?;
        self.publishers.write_hashmap_root(cell)?;
        Ok(())
    }
}
