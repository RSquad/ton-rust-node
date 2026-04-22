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
    block_proof::BlockProofStuff, engine_traits::EngineOperations, shard_state::ShardHashesStuff,
    validating_utils::UNREGISTERED_CHAIN_MAX_LEN,
};
use std::{
    io::{Cursor, Write},
    sync::Arc,
};
use storage::block_handle_db::BlockHandle;
use ton_api::ton::{
    lite_server::{
        blocklink::{BlockLinkBack, BlockLinkForward},
        partialblockproof::PartialBlockProof,
        shardblocklink::ShardBlockLink,
        shardblockproof::ShardBlockProof,
        signature::Signature,
        signature_set::signatureset::{
            Ordinary as TlSignatureSetOrdinary, Simplex as TlSignatureSetSimplex,
        },
        BlockLink, SignatureSet,
    },
    Bool,
};
use ton_block::{
    error, fail, read_single_root_boc, write_boc, AccountBlock, AccountId, AccountIdPrefixFull,
    BlkPrevInfo, Block, BlockIdExt, BlockSignaturesVariant, BocReader, Cell, ConfigParams,
    CryptoSignaturePair, Deserializable, ExtBlkRef, HashmapAugType, HashmapType, McStateExtra,
    MerkleProof, OldMcBlocksInfo, Result, Serializable, ShardDescr, ShardIdent, ShardStateUnsplit,
    SliceData, UInt256, UsageTree,
};

pub type ProofMode = i32;

pub const PM_HAS_TARGET: i32 = 0x0001;
pub const PM_ALLOW_WEAK: i32 = 0x0002;
pub const PM_BASE_FROM_REQ: i32 = 0x1000;

pub const PM_MASK: i32 = PM_HAS_TARGET | PM_ALLOW_WEAK | PM_BASE_FROM_REQ;

pub enum HeaderProofKind {
    Minimal,
    Full,
}

pub struct BlockPrevStuff {
    pub mc_block_id: BlockIdExt,
    pub prev: Vec<BlockIdExt>,
    pub _after_split: bool,
}

/// It is a wrapper around various block's representations and properties.
/// # Remark
/// Because of no deterministic of a bag of cells's serialization need to store `data`
/// to make deserialization and serialization functions symmetric.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct BlockStuff {
    id: BlockIdExt,
    block: Block,
    root: Cell,
    data: Arc<Vec<u8>>, // Arc is used to make cloning more lightweight
}

impl BlockStuff {
    pub fn deserialize_block_checked(id: BlockIdExt, data: Arc<Vec<u8>>) -> Result<Self> {
        let file_hash = UInt256::calc_file_hash(&data);
        if id.file_hash() != &file_hash {
            fail!("wrong file_hash for {}", id)
        }
        Self::deserialize_block(id, data)
    }

    pub fn deserialize_block(id: BlockIdExt, data: Arc<Vec<u8>>) -> Result<Self> {
        let root = BocReader::new().read(&mut Cursor::new(&*data))?.withdraw_single_root()?;
        if id.root_hash != *root.repr_hash() {
            fail!("wrong root hash for {}", id)
        }
        let block = Block::construct_from_cell(root.clone())?;
        Ok(Self { id, block, root, data })
    }

    pub fn block(&self) -> Result<&Block> {
        Ok(&self.block)
    }

    pub fn id(&self) -> &BlockIdExt {
        &self.id
    }

    // Unused
    //    pub fn shard(&self) -> &ShardIdent {
    //        self.id.shard()
    //    }

    pub fn root_cell(&self) -> &Cell {
        &self.root
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    // Unused
    //    pub fn is_masterchain(&self) -> bool {
    //        self.id.shard().is_masterchain()
    //    }

    pub fn gen_utime(&self) -> Result<u32> {
        Ok(self.block()?.read_info()?.gen_utime())
    }

    pub fn is_key_block(&self) -> Result<bool> {
        Ok(self.block()?.read_info()?.key_block())
    }

    pub fn construct_prev_id(&self) -> Result<(BlockIdExt, Option<BlockIdExt>)> {
        let header = self.block()?.read_info()?;
        match header.read_prev_ref()? {
            BlkPrevInfo::Block { prev } => {
                let shard_id = if header.after_split() {
                    header.shard().merge()?
                } else {
                    header.shard().clone()
                };
                let id = BlockIdExt {
                    shard_id,
                    seq_no: prev.seq_no,
                    root_hash: prev.root_hash,
                    file_hash: prev.file_hash,
                };

                Ok((id, None))
            }
            BlkPrevInfo::Blocks { prev1, prev2 } => {
                let prev1 = prev1.read_struct()?;
                let prev2 = prev2.read_struct()?;
                let (shard1, shard2) = header.shard().split()?;
                let id1 = BlockIdExt {
                    shard_id: shard1,
                    seq_no: prev1.seq_no,
                    root_hash: prev1.root_hash,
                    file_hash: prev1.file_hash,
                };
                let id2 = BlockIdExt {
                    shard_id: shard2,
                    seq_no: prev2.seq_no,
                    root_hash: prev2.root_hash,
                    file_hash: prev2.file_hash,
                };
                Ok((id1, Some(id2)))
            }
        }
    }

    pub fn construct_master_id(&self) -> Result<BlockIdExt> {
        let mc_id = self.get_master_id()?;
        Ok(BlockIdExt::from_ext_blk(mc_id))
    }

    pub fn get_account(&self, account_id: &AccountId) -> Result<Option<AccountBlock>> {
        self.block()?.read_extra()?.read_account_blocks()?.get(account_id)
    }

    pub fn get_master_id(&self) -> Result<ExtBlkRef> {
        Ok(self
            .block()?
            .read_info()?
            .read_master_ref()?
            .ok_or_else(|| error!("Can't get master ref: given block is a master block"))?
            .master)
    }

    pub fn write_to<T: Write>(&self, dst: &mut T) -> Result<()> {
        dst.write_all(&self.data)?;
        Ok(())
    }

    pub fn shard_hashes(&self) -> Result<ShardHashesStuff> {
        Ok(ShardHashesStuff::from(
            self.block()?
                .read_extra()?
                .read_custom()?
                .ok_or_else(|| error!("Given block is not a master block."))?
                .shards()
                .clone(),
        ))
    }

    pub fn top_blocks(&self, workchain_id: i32) -> Result<Vec<BlockIdExt>> {
        let mut shards = Vec::new();
        self.block()?
            .read_extra()?
            .read_custom()?
            .ok_or_else(|| error!("Given block is not a master block."))?
            .shards()
            .iterate_shards_for_workchain(
                workchain_id,
                |ident: ShardIdent, descr: ShardDescr| {
                    let last_shard_block = BlockIdExt {
                        shard_id: ident.clone(),
                        seq_no: descr.seq_no,
                        root_hash: descr.root_hash,
                        file_hash: descr.file_hash,
                    };
                    shards.push(last_shard_block);
                    Ok(true)
                },
            )?;

        Ok(shards)
    }

    pub fn _top_blocks_all_headers(&self) -> Result<Vec<(BlockIdExt, ShardDescr)>> {
        let mut shards = Vec::new();
        self.block()?
            .read_extra()?
            .read_custom()?
            .ok_or_else(|| error!("Given block is not a master block."))?
            .shards()
            .iterate_shards(|ident: ShardIdent, descr: ShardDescr| {
                let id = BlockIdExt {
                    shard_id: ident.clone(),
                    seq_no: descr.seq_no,
                    root_hash: descr.root_hash.clone(),
                    file_hash: descr.file_hash.clone(),
                };
                shards.push((id, descr));
                Ok(true)
            })?;

        Ok(shards)
    }

    pub fn top_blocks_all(&self) -> Result<Vec<BlockIdExt>> {
        let mut shards = Vec::new();
        self.block()?
            .read_extra()?
            .read_custom()?
            .ok_or_else(|| error!("Given block is not a master block."))?
            .shards()
            .iterate_shards(|ident: ShardIdent, descr: ShardDescr| {
                let last_shard_block = BlockIdExt {
                    shard_id: ident.clone(),
                    seq_no: descr.seq_no,
                    root_hash: descr.root_hash,
                    file_hash: descr.file_hash,
                };
                shards.push(last_shard_block);
                Ok(true)
            })?;
        Ok(shards)
    }

    pub fn read_config_params(&self) -> Result<ConfigParams> {
        self.block()?
            .read_extra()?
            .read_custom()?
            .ok_or_else(|| error!("Block {} doesn't contain `custom` field", self.id))?
            .config_mut()
            .take()
            .ok_or_else(|| error!("Block {} doesn't contain `config` field", self.id))
    }

    // Unused
    //    pub fn read_cur_validator_set_and_cc_conf(&self) -> Result<(ValidatorSet, CatchainConfig)> {
    //       self.block()?.read_cur_validator_set_and_cc_conf()
    //    }

    pub fn calculate_tr_count(&self) -> Result<usize> {
        let now = std::time::Instant::now();
        let mut tr_count = 0;

        self.block()?.read_extra()?.read_account_blocks()?.iterate_objects(|account_block| {
            tr_count += account_block.transactions().len()?;
            Ok(true)
        })?;
        log::trace!(
            "calculate_tr_count: transactions {}, TIME: {}ms, block: {}",
            tr_count,
            now.elapsed().as_millis(),
            self.id()
        );
        Ok(tr_count)
    }
}

#[cfg(test)]
use ton_block::{BlkMasterInfo, BlockInfo};

#[cfg(test)]
impl BlockStuff {
    pub fn from_block(block: Block) -> Result<Self> {
        let root = block.serialize()?;
        let data = write_boc(&root)?;
        let file_hash = UInt256::calc_file_hash(&data);
        let block_info = block.read_info()?;
        let id = BlockIdExt {
            shard_id: block_info.shard().clone(),
            seq_no: block_info.seq_no(),
            root_hash: root.repr_hash().clone(),
            file_hash,
        };
        Ok(Self { id, block, root, data: Arc::new(data) })
    }

    pub fn read_block_from_file(filename: &str) -> Result<Self> {
        let data = Arc::new(std::fs::read(filename)?);
        let file_hash = UInt256::calc_file_hash(&data);
        let root = read_single_root_boc(&*data)?;
        let block = Block::construct_from_cell(root.clone())?;
        let block_info = block.read_info()?;
        let id = BlockIdExt {
            shard_id: block_info.shard().clone(),
            seq_no: block_info.seq_no(),
            root_hash: root.repr_hash().clone(),
            file_hash,
        };
        Ok(Self { id, block, root, data })
    }

    pub fn fake_block(
        id: BlockIdExt,
        mc_block_id: Option<BlockIdExt>,
        is_key_block: bool,
    ) -> Result<Self> {
        let mut block = Block::default();
        if let Some(mc_block_id) = mc_block_id {
            let mut info = BlockInfo::default();
            info.write_master_ref(Some(&BlkMasterInfo {
                master: ExtBlkRef {
                    end_lt: 0,
                    seq_no: mc_block_id.seq_no,
                    root_hash: mc_block_id.root_hash,
                    file_hash: mc_block_id.file_hash,
                },
            }))?;
            info.set_key_block(is_key_block);
            block.write_info(&info)?;
        }
        Ok(BlockStuff { id, block, root: Cell::default(), data: Arc::new(vec![0xfe; 10_000]) })
    }

    pub fn fake_with_block(id: BlockIdExt, block: Block) -> Self {
        BlockStuff { id, block, root: Cell::default(), data: Arc::new(vec![0xfe; 10_000]) }
    }
}

pub trait BlockIdExtExtention {
    fn is_masterchain(&self) -> bool;
}
impl BlockIdExtExtention for BlockIdExt {
    fn is_masterchain(&self) -> bool {
        self.shard().is_masterchain()
    }
}
// unpack_block_prev_blk_try in t-node
pub fn construct_and_check_prev_stuff(
    block_root: &Cell,
    id: &BlockIdExt,
    fetch_blkid: bool,
) -> Result<(BlockIdExt, BlockPrevStuff)> {
    let block = Block::construct_from_cell(block_root.clone())?;
    let info = block.read_info()?;

    if info.version() != 0 {
        fail!("Block -> info -> version should be zero (found {})", info.version())
    }

    let out_block_id = if fetch_blkid {
        BlockIdExt {
            shard_id: info.shard().clone(),
            seq_no: info.seq_no(),
            root_hash: block_root.repr_hash().clone(),
            file_hash: UInt256::default(),
        }
    } else {
        if id.shard() != info.shard() {
            fail!(
                "block header contains shard ident: {}, but expected: {}",
                info.shard(),
                id.shard()
            )
        }
        if id.seq_no() != info.seq_no() {
            fail!("block header contains seq_no: {}, but expected: {}", info.seq_no(), id.seq_no())
        }
        if *id.root_hash() != *block_root.repr_hash() {
            fail!(
                "block header has incorrect root hash: {:x}, but expected: {:x}",
                block_root.repr_hash(),
                id.root_hash()
            )
        }
        BlockIdExt::default()
    };

    let master_ref = info.read_master_ref()?;
    if master_ref.is_some() == info.shard().is_masterchain() {
        fail!("Block info: `info.is_master()` and `info.shard().is_masterchain()` mismatch");
    }

    let out_after_split = info.after_split();

    let mut out_prev = vec![];
    let prev_seqno = match info.read_prev_ref()? {
        BlkPrevInfo::Block { prev } => {
            out_prev.push(BlockIdExt {
                shard_id: if info.after_split() {
                    info.shard().merge()?
                } else {
                    info.shard().clone()
                },
                seq_no: prev.seq_no,
                root_hash: prev.root_hash,
                file_hash: prev.file_hash,
            });
            prev.seq_no
        }
        BlkPrevInfo::Blocks { prev1, prev2 } => {
            if info.after_split() {
                fail!("shardchains cannot be simultaneously split and merged at the same block")
            }
            let prev1 = prev1.read_struct()?;
            let prev2 = prev2.read_struct()?;
            if prev1.seq_no == 0 || prev2.seq_no == 0 {
                fail!("shardchains cannot be merged immediately after initial state")
            }
            let (shard1, shard2) = info.shard().split()?;
            out_prev.push(BlockIdExt {
                shard_id: shard1,
                seq_no: prev1.seq_no,
                root_hash: prev1.root_hash,
                file_hash: prev1.file_hash,
            });
            out_prev.push(BlockIdExt {
                shard_id: shard2,
                seq_no: prev2.seq_no,
                root_hash: prev2.root_hash,
                file_hash: prev2.file_hash,
            });
            prev1.seq_no.max(prev2.seq_no)
        }
    };

    if id.seq_no() != prev_seqno + 1 {
        fail!(
            "new block has invalid seqno (not equal to one plus maximum of seqnos of its ancestors)"
        );
    }

    let out_mc_block_id = if info.shard().is_masterchain() {
        out_prev[0].clone()
    } else {
        master_ref
            .ok_or_else(|| error!("non masterchain block doesn't contain mc block ref"))?
            .master
            .master_block_id()
            .1
    };

    if info.shard().is_masterchain() && (info.vert_seqno_incr() != 0) && !info.key_block() {
        fail!("non-key masterchain block cannot have vert_seqno_incr set")
    }

    Ok((
        out_block_id,
        BlockPrevStuff {
            mc_block_id: out_mc_block_id,
            prev: out_prev,
            _after_split: out_after_split,
        },
    ))
}

#[inline]
pub fn ensure_mc_full(id: &BlockIdExt) -> Result<()> {
    if !id.is_masterchain() || !id.shard().is_full() {
        fail!("BlockIdExt must be masterchain FULL shard: {}", id);
    }
    Ok(())
}

fn sigset_from_proof_boc(id: &BlockIdExt, boc: &[u8]) -> Result<SignatureSet> {
    let proof = BlockProofStuff::deserialize(id, boc.to_vec(), false)?;

    let block_sigs =
        proof.drain_signatures().map_err(|_| error!("no signatures in proof for {}", id))?;

    let mut out_sigs: Vec<Signature> =
        Vec::with_capacity(block_sigs.pure_signatures().count() as usize);

    block_sigs.pure_signatures().signatures().iterate_slices(
        |_key_bits, mut slice: SliceData| {
            // Every value — that's CryptoSignaturePair
            let pair = CryptoSignaturePair::construct_from(&mut slice)?;
            out_sigs.push(Signature {
                node_id_short: pair.node_id_short.clone(),
                signature: pair.sign.as_bytes().to_vec(),
            });
            Ok(true)
        },
    )?;
    let validator_set_hash = block_sigs.validator_info().validator_list_hash_short as i32;
    let cc_seqno = block_sigs.validator_info().catchain_seqno as i32;

    Ok(match block_sigs {
        BlockSignaturesVariant::Ordinary(_) => {
            SignatureSet::LiteServer_SignatureSet_Ordinary(TlSignatureSetOrdinary {
                validator_set_hash,
                catchain_seqno: cc_seqno,
                signatures: out_sigs,
            })
        }
        BlockSignaturesVariant::Simplex(simplex) => {
            SignatureSet::LiteServer_SignatureSet_Simplex(TlSignatureSetSimplex {
                cc_seqno,
                validator_set_hash,
                signatures: out_sigs,
                session_id: simplex.session_id.clone(),
                slot: simplex.slot as i32,
                candidate: simplex.candidate_data_bytes()?,
            })
        }
    })
}

async fn build_zs_config_proof(
    engine: &Arc<dyn EngineOperations>,
    zs: &BlockIdExt,
) -> Result<Vec<u8>> {
    debug_assert_eq!(zs.seq_no, 0);
    let zs_state = engine.load_state(zs).await?;
    tokio::task::spawn_blocking(move || {
        let zs_root = zs_state.root_cell();
        let usage = UsageTree::with_root(zs_root.clone());
        let ss: ShardStateUnsplit = ShardStateUnsplit::construct_from_cell(usage.root_cell())?;
        let custom = ss.read_custom()?.ok_or_else(|| error!("No custom in zerostate"))?;
        let cfg = custom.config();
        let _ = cfg.validator_set()?;
        let _ = cfg.catchain_config()?;
        let _ = cfg.prev_validator_set_present()?;
        let _ = cfg.next_validator_set_present()?;

        Ok(MerkleProof::create_by_usage_tree(&zs_root, &usage)?.write_to_bytes()?)
    })
    .await?
}

pub fn proof_mc_to_shard_top(
    mc_root: &Cell,
    target_shard: &ShardIdent,
) -> Result<(BlockIdExt, Vec<u8>)> {
    let usage = UsageTree::with_params(mc_root.clone(), true);
    let mc_blk: Block = Block::construct_from_cell(usage.root_cell())?;

    let _info = mc_blk.read_info()?;
    let mc_extra = mc_blk
        .read_extra()?
        .read_custom()?
        .ok_or_else(|| error!("no extra.custom in masterchain block"))?;

    let mut rec_opt = mc_extra.shards().get_shard(target_shard)?;
    if rec_opt.is_none() {
        let mut cur = target_shard.clone();
        while cur.prefix_len() > 0 && rec_opt.is_none() {
            cur = cur.merge()?;
            rec_opt = mc_extra.shards().get_shard(&cur)?;
        }
    }
    let rec = rec_opt
        .ok_or_else(|| error!("target shard {target_shard} not found in MC shard_hashes"))?;

    let top_id = rec.block_id().clone();
    let proof = MerkleProof::create_by_usage_tree(mc_root, &usage)?.write_to_bytes()?;
    Ok((top_id, proof))
}

pub fn create_shard_proof(mc_root: &Cell, shard_id: &ShardIdent) -> Result<MerkleProof> {
    let usage_tree = UsageTree::with_root(mc_root.clone());
    let mc_state = ShardStateUnsplit::construct_from_cell(usage_tree.root_cell())?;
    let Some(custom) = mc_state.read_custom()? else {
        fail!("No shard info in MC state with root {:x}", mc_root.repr_hash());
    };
    if custom.shards().get_shard(shard_id)?.is_none() {
        fail!("No shard {shard_id} in MC state with root {:x}", mc_root.repr_hash());
    }
    MerkleProof::create_by_usage_tree(mc_root, &usage_tree).map_err(|e| {
        error!(
            "Create Merkle-proof for shard {shard_id} in MC with root {} error: {e}",
            mc_root.repr_hash()
        )
    })
}

pub fn proof_shard_prev_link(
    shard_root: &Cell,
    cur_id: &BlockIdExt,
    target_id: &BlockIdExt,
) -> Result<(BlockIdExt, Vec<u8>)> {
    let usage = UsageTree::with_root(shard_root.clone());
    let (_, prev_stuff) = construct_and_check_prev_stuff(&usage.root_cell(), cur_id, false)?;

    let target_shard = target_id.shard();
    let chosen_prev = prev_stuff
        .prev
        .into_iter()
        .find(|pid| pid.shard().intersect_with(target_shard))
        .ok_or_else(|| error!("failed to find suitable prev intersecting target shard"))?;

    let proof_bytes = MerkleProof::create_by_usage_tree(shard_root, &usage)?.write_to_bytes()?;
    Ok((chosen_prev, proof_bytes))
}

pub fn create_prevblocks_proof(state_root: Cell, target_seqno: u32) -> Result<(Vec<u8>, bool)> {
    let usage = UsageTree::with_root(state_root.clone());
    let state = ShardStateUnsplit::construct_from_cell(usage.root_cell())?;
    let custom = state.read_custom()?.ok_or_else(|| error!("No custom in mc state"))?;
    let cfg = custom.config();
    let _ = cfg.catchain_config();
    let _ = cfg.validator_set()?;
    let _ = cfg.prev_validator_set_present()?;
    let _ = cfg.next_validator_set_present()?;

    let _ = custom.prev_blocks.get(&0)?.ok_or_else(|| error!("No zerostate in prev_blocks"))?;
    let tref = custom
        .prev_blocks
        .get(&target_seqno)?
        .ok_or_else(|| error!("No such target block in prev_blocks"))?;
    let to_key_block = tref.key;

    let proof = MerkleProof::create_by_usage_tree(&state_root, &usage)?;
    Ok((proof.write_to_bytes()?, to_key_block))
}

pub fn visit_prev_blocks_info(custom: &McStateExtra, ss: &ShardStateUnsplit) -> Result<()> {
    let last_mc_seqno = ss.seq_no();

    let _ = custom.prev_blocks.get(&0)?;

    let mut seq = last_mc_seqno;
    let mut taken = 0usize;
    while taken < 16 && seq > 0 {
        if seq == 0 {
            break;
        }
        seq -= 1;

        if let Some(_) = custom.prev_blocks.get(&seq)? {
            taken += 1;
        }
    }

    let _ = &custom.last_key_block;
    let _ = custom.prev_blocks.get_prev_key_block(last_mc_seqno)?;

    let mut seq100 = (last_mc_seqno / 100) * 100;
    let mut taken100 = 0usize;
    if seq100 == last_mc_seqno {
        seq100 = seq100.saturating_sub(100);
        taken100 += 1;
    }

    while taken100 < 16 {
        if let Some(_) = custom.prev_blocks.get(&seq100)? {
            taken100 += 1;
        } else {
            break;
        }
        if seq100 < 100 {
            break;
        }
        seq100 -= 100;
    }
    Ok(())
}

pub fn header_proof(block_root: &Cell, kind: HeaderProofKind) -> Result<MerkleProof> {
    let usage = UsageTree::with_params(block_root.clone(), true);
    let blk = Block::construct_from_cell(usage.root_cell())?;
    let info = blk.read_info()?;
    let _ = info.read_prev_ref()?;

    match kind {
        HeaderProofKind::Minimal => {}
        HeaderProofKind::Full => {
            let _ = blk.read_value_flow()?;
            let _ = blk.read_state_update()?;
            let _ = blk.read_extra()?;
        }
    }
    MerkleProof::create_by_usage_tree(block_root, &usage)
}

pub async fn get_shard_block_proof(
    engine: &Arc<dyn EngineOperations>,
    id: BlockIdExt,
) -> Result<ShardBlockProof> {
    if id.is_masterchain() {
        return Ok(ShardBlockProof { masterchain_id: id, links: vec![] });
    }

    // search base-mc-block which connect target-shard-block
    let handle = engine.load_block_handle(&id)?.ok_or_else(|| error!("no handle for {}", id))?;

    let mc_ref_seqno = handle.masterchain_ref_seq_no();

    let mc_prefix = AccountIdPrefixFull::any_masterchain();
    let (base_mc_id, base_mc_raw) =
        engine
            .lookup_block_by_seqno(&mc_prefix, mc_ref_seqno)
            .await?
            .ok_or_else(|| error!("cannot find base masterchain block by seqno {mc_ref_seqno}"))?;
    let mut cur_id = base_mc_id.clone();
    let mut cur_root =
        tokio::task::spawn_blocking(move || read_single_root_boc(&base_mc_raw)).await??;

    let mut links: Vec<ShardBlockLink> = Vec::new();

    loop {
        let id_snap = cur_id.clone();
        let root_snap = cur_root.clone();

        if id_snap.is_masterchain() {
            // MC → top(shard)@MC
            let (prev_id, proof_bytes) = proof_mc_to_shard_top(&root_snap, &id.shard())?;
            links.push(ShardBlockLink { id: prev_id.clone(), proof: proof_bytes });

            if links.len() > UNREGISTERED_CHAIN_MAX_LEN as usize {
                fail!("proof chain is too long (>{})", UNREGISTERED_CHAIN_MAX_LEN);
            }

            if prev_id == id {
                break;
            }
            cur_id = prev_id;
            cur_root = {
                let h = engine
                    .load_block_handle(&cur_id)?
                    .ok_or_else(|| error!("no handle for {}", cur_id))?;
                let raw = engine.load_block_raw(&h).await?;
                tokio::task::spawn_blocking(move || read_single_root_boc(&raw)).await??
            };
        } else {
            let (prev_id, proof_bytes) = proof_shard_prev_link(&root_snap, &id_snap, &id)?;
            links.push(ShardBlockLink { id: prev_id.clone(), proof: proof_bytes });

            if links.len() > UNREGISTERED_CHAIN_MAX_LEN as usize {
                fail!("proof chain is too long");
            }
            if prev_id == id {
                break;
            }
            cur_id = prev_id;
            let handle = engine
                .load_block_handle(&cur_id)?
                .ok_or_else(|| error!("no handle for {}", cur_id))?;
            let bp = engine.load_block_proof(&handle, !cur_id.is_masterchain()).await?;
            let (_blk, virt_root) = bp.virtualize_block()?;
            cur_root = virt_root;
        }
    }
    Ok(ShardBlockProof { masterchain_id: base_mc_id, links })
}

pub async fn get_block_proof(
    engine: &Arc<dyn EngineOperations>,
    mode_raw: i32,
    known_block: BlockIdExt,
    target_block_opt: Option<BlockIdExt>,
) -> Result<PartialBlockProof> {
    if !known_block.is_masterchain() || !known_block.shard().is_full() {
        fail!("known_block must be in masterchain");
    }

    let mode: ProofMode = mode_raw & PM_MASK;

    let (target_block, base_block) =
        resolve_target_and_base(&engine, mode, &known_block, target_block_opt).await?;

    ensure_mc_full(&target_block)?;
    ensure_mc_full(&base_block)?;

    if known_block.seq_no > base_block.seq_no {
        fail!("known {} is newer than base {}", known_block, base_block);
    }
    if target_block.seq_no > base_block.seq_no {
        fail!("target {} is newer than base {}", target_block, base_block);
    }
    if known_block.seq_no == target_block.seq_no {
        return Ok(PartialBlockProof {
            complete: Bool::BoolTrue,
            from: known_block,
            to: target_block,
            steps: Vec::new(),
        });
    }
    if target_block.seq_no < known_block.seq_no {
        build_backward_proof(&engine, &known_block, &target_block).await
    } else {
        build_forward_proof(&engine, &known_block, &target_block, &base_block).await
    }
}

pub fn get_last_liteserver_state_block(
    engine: &Arc<dyn EngineOperations>,
) -> Result<Arc<BlockIdExt>> {
    let last_mc_id = engine
        .load_last_applied_mc_block_id()?
        .ok_or_else(|| error!("Cannot load last applied mc block id"))?;
    let Some(shard_client_id) = engine.load_shard_client_mc_block_id()? else {
        return Ok(last_mc_id);
    };
    if shard_client_id.seq_no + 8 < last_mc_id.seq_no {
        Ok(last_mc_id)
    } else {
        Ok(shard_client_id)
    }
}

pub async fn resolve_target_and_base(
    engine: &Arc<dyn EngineOperations>,
    mode_raw: ProofMode,
    known_block: &BlockIdExt,
    target_block_opt: Option<BlockIdExt>,
) -> Result<(BlockIdExt, BlockIdExt)> {
    ensure_mc_full(known_block)?;
    let mode = mode_raw & PM_MASK;

    let has_target = (mode & PM_HAS_TARGET) != 0;
    let allow_weak = (mode & PM_ALLOW_WEAK) != 0;
    let base_from_req = (mode & PM_BASE_FROM_REQ) != 0;

    let target_block = if has_target {
        let tb =
            target_block_opt.ok_or_else(|| error!("mode HAS_TARGET but target_block is None"))?;
        ensure_mc_full(&tb)?;
        tb
    } else if allow_weak {
        get_last_liteserver_state_block(engine)?.as_ref().clone()
    } else {
        engine
            .load_shard_client_mc_block_id()?
            .ok_or_else(|| error!("shard client mc block id is unknown"))?
            .as_ref()
            .clone()
    };

    let need_max_known_target = (has_target && base_from_req) || (!has_target && !allow_weak);

    let base_block = if need_max_known_target {
        if known_block.seq_no > target_block.seq_no {
            known_block.clone()
        } else {
            target_block.clone()
        }
    } else {
        get_last_liteserver_state_block(engine)?.as_ref().clone()
    };

    Ok((target_block, base_block))
}

async fn is_key_block(engine: &dyn EngineOperations, id: &BlockIdExt) -> Result<bool> {
    let h = engine.load_block_handle(id)?.ok_or_else(|| error!("no handle for {}", id))?;
    h.is_key_block()
}

fn choose_is_link(handle: &BlockHandle) -> Result<bool> {
    if handle.has_proof() {
        Ok(false)
    } else if handle.has_proof_link() {
        Ok(true)
    } else {
        fail!("no proof/proof_link in archive for {:?}", handle);
    }
}

fn prev_key_of(extra: &OldMcBlocksInfo, seq: u32) -> Result<Option<BlockIdExt>> {
    Ok(extra.get_prev_key_block(seq)?.map(|ext| ext.master_block_id().1))
}

fn next_key_after(extra: &OldMcBlocksInfo, seq: u32) -> Result<Option<BlockIdExt>> {
    Ok(extra.get_next_key_block(seq.saturating_add(1))?.map(|ext| ext.master_block_id().1))
}

fn same_interval(extra: &OldMcBlocksInfo, a_seq: u32, b_seq: u32) -> Result<bool> {
    // prev key strictly before
    let pa = extra.get_prev_key_block(a_seq)?;
    let pb = extra.get_prev_key_block(b_seq)?;
    Ok(match (pa, pb) {
        (Some(a), Some(b)) => a.seq_no == b.seq_no,
        _ => false,
    })
}

#[inline]
pub async fn build_back_link(
    engine: &Arc<dyn EngineOperations>,
    from: &BlockIdExt,
    to: &BlockIdExt,
) -> Result<BlockLinkBack> {
    let from_state = engine.load_state(from).await?;
    let from_state_root = from_state.root_cell();
    let (prevblocks_proof, _) = create_prevblocks_proof(from_state_root.clone(), to.seq_no)?;

    let (dest_proof, to_is_key) = if to.seq_no == 0 {
        (Vec::<u8>::new(), false)
    } else {
        let handle = engine.load_block_handle(to)?.ok_or_else(|| error!("no handle for {}", to))?;
        let bp_to = engine.load_block_proof(&handle, !to.is_masterchain()).await?;
        let dest_proof = write_boc(bp_to.merkle_proof_root_cell())?;
        let to_is_key =
            engine.load_block_handle(to)?.ok_or_else(|| error!("no handle"))?.is_key_block()?;
        (dest_proof, to_is_key)
    };

    let from_handle =
        engine.load_block_handle(from)?.ok_or_else(|| error!("no handle for {}", from))?;
    let bp_from = engine.load_block_proof(&from_handle, !from.is_masterchain()).await?;
    let from = from.clone();
    let to = to.clone();
    tokio::task::spawn_blocking(move || {
        let (_virt_block, virt_root) = bp_from.virtualize_block()?;
        let usage = UsageTree::with_params(virt_root.clone(), true);
        let blk = Block::construct_from_cell(usage.root_cell())?;
        let _ = blk.read_info()?;
        let _ = blk.read_state_update()?;
        let state_proof =
            MerkleProof::create_by_usage_tree(&virt_root, &usage)?.write_to_bytes()?;

        Ok(BlockLinkBack {
            to_key_block: if to_is_key { Bool::BoolTrue } else { Bool::BoolFalse },
            from,
            to,
            dest_proof,
            proof: state_proof,
            state_proof: prevblocks_proof,
        })
    })
    .await?
}

pub async fn build_backward_proof(
    engine: &Arc<dyn EngineOperations>,
    known_block: &BlockIdExt,
    target_block: &BlockIdExt,
) -> Result<PartialBlockProof> {
    let back = build_back_link(engine, known_block, target_block).await?;
    Ok(PartialBlockProof {
        complete: Bool::BoolTrue,
        from: known_block.clone(),
        to: target_block.clone(),
        steps: vec![BlockLink::LiteServer_BlockLinkBack(back)],
    })
}

pub async fn build_forward_proof(
    engine: &Arc<dyn EngineOperations>,
    from: &BlockIdExt,
    to: &BlockIdExt,
    base: &BlockIdExt,
) -> Result<PartialBlockProof> {
    let mut steps = Vec::new();

    let base_state = engine.load_state(&base).await?;
    let prev_blocks = &base_state.shard_state_extra()?.prev_blocks;

    let mut cur = from.clone();
    // if from not key_block, we back to prev_key
    let cur_is_key = is_key_block(engine.as_ref(), &cur).await?;
    if !cur_is_key && cur.seq_no != 0 {
        let prev_key = prev_key_of(prev_blocks, cur.seq_no)?
            .ok_or_else(|| error!("no prev key for {}", cur))?;
        let back_link = build_back_link(engine, &cur, &prev_key).await?;
        steps.push(BlockLink::LiteServer_BlockLinkBack(back_link));
        cur = prev_key;
    }

    let mut last = cur.clone();
    let mut complete = Bool::BoolFalse;

    while steps.len() < 16 {
        let step_to = if same_interval(prev_blocks, cur.seq_no, to.seq_no)? {
            to.clone()
        } else {
            next_key_after(prev_blocks, cur.seq_no)?
                .ok_or_else(|| error!("no next key block after {}", cur))?
        };

        let to_handle = engine
            .load_block_handle(&step_to)?
            .ok_or_else(|| error!("no handle for {}", step_to))?;
        let is_link_to = choose_is_link(&to_handle)?;
        let bp_to = engine.load_block_proof(&to_handle, is_link_to).await?;
        let (_vblk_to, vroot_to) = bp_to.virtualize_block()?;
        let dest_proof: Vec<u8> =
            header_proof(&vroot_to, HeaderProofKind::Full)?.write_to_bytes()?;

        let config_proof = if last.seq_no == 0 {
            build_zs_config_proof(engine, &last).await?
        } else {
            let last_handle =
                engine.load_block_handle(&last)?.ok_or_else(|| error!("no handle for {}", last))?;
            let is_link_last = choose_is_link(&last_handle)?;
            let bp_last = engine.load_block_proof(&last_handle, is_link_last).await?;
            write_boc(bp_last.merkle_proof_root_cell())?
        };

        let to_is_key = is_key_block(engine.as_ref(), &step_to).await?;

        let signatures = sigset_from_proof_boc(&step_to, &bp_to.data())?;

        let link = BlockLink::LiteServer_BlockLinkForward(BlockLinkForward {
            to_key_block: to_is_key.into(),
            from: cur.clone(),
            to: step_to.clone(),
            dest_proof,
            config_proof,
            signatures,
        });

        steps.push(link);
        cur = step_to.clone();
        last = step_to;

        if last.seq_no == to.seq_no {
            complete = Bool::BoolTrue;
            break;
        }
    }

    if complete == Bool::BoolFalse {
        log::warn!(
            "get_block_proof: forward incomplete after {} steps (limit=16), last seqno={}",
            steps.len(),
            last.seq_no
        );
    }
    Ok(PartialBlockProof { complete, from: from.clone(), to: last, steps })
}

#[cfg(test)]
#[path = "tests/test_block.rs"]
mod tests;
