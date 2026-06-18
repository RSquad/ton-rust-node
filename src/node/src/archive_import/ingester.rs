/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    archive_import::{scanner::PackageGroup, validator::ValidatorState},
    block::BlockIdExtExtention,
    block_proof::BlockProofStuff,
    internal_db::{
        ARCHIVES_GC_BLOCK, LAST_APPLIED_MC_BLOCK, PSS_KEEPER_MC_BLOCK, SHARD_CLIENT_MC_BLOCK,
    },
    shard_state::ShardHashesStuff,
};
use futures::future::try_join_all;
use rayon::prelude::*;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
    time::Instant,
};
use storage::{
    archives::{
        archive_manager::{ArchiveManager, ImportBlockMeta, ImportEntry},
        package::read_package_from_file,
        package_entry_id::PackageEntryId,
    },
    block_handle_db::BlockHandleStorage,
    block_info_db::BlockInfoDb,
    traits::Serializable,
    types::BlockMeta,
};
use ton_block::{
    error, fail, read_single_root_boc, Block, BlockIdExt, Cell, Deserializable, Error, Result,
    ShardIdent, UInt256,
};

const TARGET: &str = "archive_import";

struct RawEntry {
    block_data: Vec<u8>,
    block_offset: u64,
    proof_data: Vec<u8>,
    proof_offset: u64,
}

async fn read_raw_package(path: &Path) -> Result<HashMap<BlockIdExt, RawEntry>> {
    let mut reader = read_package_from_file(path).await?;
    let mut entries = HashMap::<BlockIdExt, RawEntry>::new();
    let mut offset: u64 = 0;
    while let Some(entry) = reader.next().await? {
        let entry_size = entry.serialized_size();
        let entry_id = PackageEntryId::<BlockIdExt>::from_filename(entry.filename())?;
        let (block_id, is_proof) = match entry_id {
            PackageEntryId::Block(id) => (id, false),
            PackageEntryId::Proof(id) if id.is_masterchain() => (id, true),
            PackageEntryId::ProofLink(id) if !id.is_masterchain() => (id, true),
            entry_id => {
                log::warn!("Unexpected entry type {} in {}", entry_id, path.display());
                offset += entry_size;
                continue;
            }
        };
        let mut data = entry.take_data();
        entries
            .entry(block_id)
            .and_modify(|e| {
                if is_proof {
                    e.proof_data = std::mem::take(&mut data);
                    e.proof_offset = offset;
                } else {
                    e.block_data = std::mem::take(&mut data);
                    e.block_offset = offset;
                }
            })
            .or_insert_with(|| {
                if is_proof {
                    RawEntry {
                        block_data: vec![],
                        block_offset: 0,
                        proof_data: data,
                        proof_offset: offset,
                    }
                } else {
                    RawEntry {
                        block_data: data,
                        block_offset: offset,
                        proof_data: vec![],
                        proof_offset: 0,
                    }
                }
            });
        offset += entry_size;
    }
    Ok(entries)
}

struct McEntry {
    block_id: BlockIdExt,
    prev_block_id: BlockIdExt,
    proof: BlockProofStuff,
    is_key: bool,
    gen_utime: u32,
    end_lt: u64,
    shard_tops: Vec<BlockIdExt>,
    state_update_new: Cell,
    proof_data: Vec<u8>,
    proof_offset: u64,
    block_data: Vec<u8>,
    block_offset: u64,
}

struct ProcessedEntry {
    block_id: BlockIdExt,
    gen_utime: u32,
    end_lt: u64,
    mc_ref_seq_no: u32,
    is_key_block: bool,
    proof_offset: u64,
    block_offset: u64,
    prevs: Vec<BlockIdExt>,
    state_update_new: Cell,
}

impl ProcessedEntry {
    fn to_import_entries(&self) -> [ImportEntry; 2] {
        let proof_entry_id = if self.block_id.is_masterchain() {
            PackageEntryId::Proof(self.block_id.clone())
        } else {
            PackageEntryId::ProofLink(self.block_id.clone())
        };
        [
            ImportEntry { entry_id: proof_entry_id, offset: self.proof_offset, block_meta: None },
            ImportEntry {
                entry_id: PackageEntryId::Block(self.block_id.clone()),
                offset: self.block_offset,
                block_meta: Some(ImportBlockMeta {
                    seq_no: self.block_id.seq_no(),
                    shard: self.block_id.shard_id.clone(),
                    gen_utime: self.gen_utime,
                    end_lt: self.end_lt,
                    mc_ref_seq_no: self.mc_ref_seq_no,
                }),
            },
        ]
    }
}

struct KeyBlockData {
    block_id: BlockIdExt,
    proof_data: Vec<u8>,
    block_data: Vec<u8>,
}

pub struct LastGroupState {
    pub mc_block_id: BlockIdExt,
    pub shard_tops: Vec<BlockIdExt>,
}

fn parse_and_verify_block(data: &[u8], declared_id: &BlockIdExt) -> Result<Block> {
    let file_hash = UInt256::calc_file_hash(data);
    let root_cell = read_single_root_boc(data)?;
    let root_hash = root_cell.repr_hash().clone();
    let block = Block::construct_from_cell(root_cell)?;
    let info = block.read_info()?;
    let actual_id =
        BlockIdExt::with_params(info.shard().clone(), info.seq_no(), root_hash, file_hash);
    if actual_id != *declared_id {
        return Err(error!("Block declared as {} but data contains {}", declared_id, actual_id));
    }
    Ok(block)
}

fn deserialize_mc_entry(block_id: BlockIdExt, raw: RawEntry) -> Result<McEntry> {
    if raw.proof_data.is_empty() {
        return Err(error!("MC block {} has no proof in the package", block_id));
    }
    if raw.block_data.is_empty() {
        return Err(error!("MC block {} has no block data in the package", block_id));
    }

    let proof = BlockProofStuff::deserialize(&block_id, raw.proof_data.clone(), false)?;
    let (virt_block, _) = proof.virtualize_block()?;
    let is_key = virt_block.read_info()?.key_block();

    let block = parse_and_verify_block(&raw.block_data, &block_id)?;
    let block_info = block.read_info()?;
    let gen_utime = block_info.gen_utime();
    let end_lt = block_info.end_lt();
    let mut prev_ids = block_info.read_prev_ids()?;
    if prev_ids.len() != 1 {
        return Err(error!("MC block {} has {} prev refs, expected 1", block_id, prev_ids.len()));
    }
    let prev_block_id = prev_ids.pop().unwrap();
    let extra = block
        .read_extra()?
        .read_custom()?
        .ok_or_else(|| error!("No McExtra in master block {}", block_id))?;
    let shard_tops = ShardHashesStuff::from(extra.shards().clone()).top_blocks_all()?;
    let state_update_new = block.read_state_update()?.new;

    Ok(McEntry {
        block_id,
        prev_block_id,
        proof,
        is_key,
        gen_utime,
        end_lt,
        shard_tops,
        state_update_new,
        proof_data: raw.proof_data,
        proof_offset: raw.proof_offset,
        block_data: raw.block_data,
        block_offset: raw.block_offset,
    })
}

fn validate_mc_range(
    entries: &[McEntry],
    key_proof: &Option<BlockProofStuff>,
    zerostate: &Arc<crate::shard_state::ShardStateStuff>,
) -> Result<()> {
    entries.par_iter().try_for_each(|e| match key_proof {
        None => e.proof.check_with_master_state(zerostate),
        Some(kb) => e.proof.check_with_prev_key_block_proof(kb),
    })
}

fn check_mc_chain(entries: &[McEntry], expected_first_prev: &BlockIdExt) -> Result<()> {
    if let Some(first) = entries.first() {
        if first.prev_block_id != *expected_first_prev {
            fail!(
                "MC chain gap between packages: block {} prev_ref = {} but expected {}",
                first.block_id,
                first.prev_block_id,
                expected_first_prev,
            );
        }
    }
    for w in entries.windows(2) {
        if w[1].prev_block_id != w[0].block_id {
            fail!(
                "MC chain gap: block {} prev_ref = {} but expected {}",
                w[1].block_id,
                w[1].prev_block_id,
                w[0].block_id,
            );
        }
    }
    Ok(())
}

fn parse_mc_entries(
    raw: HashMap<BlockIdExt, RawEntry>,
    validator: &mut ValidatorState,
    skip: bool,
    expected_first_prev: BlockIdExt,
) -> Result<(Vec<ProcessedEntry>, Option<KeyBlockData>, Vec<(u32, Vec<BlockIdExt>)>, LastGroupState)>
{
    let mut entries: Vec<McEntry> =
        raw.into_par_iter().map(|(id, r)| deserialize_mc_entry(id, r)).collect::<Result<_>>()?;
    entries.sort_by_key(|e| e.block_id.seq_no());
    check_mc_chain(&entries, &expected_first_prev)?;

    let rest_start = if entries.first().map(|e| e.is_key).unwrap_or(false) {
        let block_id = &entries[0].block_id;
        if !skip {
            // Skip re-validation if this key block is already the current validation root (resume).
            let already_done = validator
                .current_key_block_proof()
                .map(|kp| kp.id().seq_no() >= block_id.seq_no())
                .unwrap_or(false);
            if !already_done {
                let is_hardfork = validator.is_hardfork(block_id);
                if is_hardfork {
                    log::info!(
                        target: TARGET,
                        "Hard fork block {} accepted as new validation root",
                        block_id,
                    );
                } else {
                    let key_proof = validator.current_key_block_proof().cloned();
                    let zerostate = Arc::clone(validator.zerostate());
                    validate_mc_range(&entries[..1], &key_proof, &zerostate)?;
                }
            }
        }
        validator.set_key_block_proof(entries[0].proof.clone());
        1
    } else {
        0
    };

    if !skip {
        let key_proof = validator.current_key_block_proof().cloned();
        let zerostate = Arc::clone(validator.zerostate());
        validate_mc_range(&entries[rest_start..], &key_proof, &zerostate)?;
    }

    let mut processed = Vec::with_capacity(entries.len());
    let mut key_block: Option<KeyBlockData> = None;
    let mut mc_shard_tops: Vec<(u32, Vec<BlockIdExt>)> = Vec::new();

    for entry in entries {
        if entry.is_key {
            if Some(&entry.block_id) != validator.current_key_block_proof().map(|kp| kp.id()) {
                fail!("Second key block {} in package", entry.block_id);
            }
            key_block = Some(KeyBlockData {
                block_id: entry.block_id.clone(),
                proof_data: entry.proof_data,
                block_data: entry.block_data.clone(),
            });
        }
        mc_shard_tops.push((entry.block_id.seq_no(), entry.shard_tops));
        processed.push(ProcessedEntry {
            mc_ref_seq_no: entry.block_id.seq_no(),
            block_id: entry.block_id,
            gen_utime: entry.gen_utime,
            end_lt: entry.end_lt,
            is_key_block: entry.is_key,
            proof_offset: entry.proof_offset,
            block_offset: entry.block_offset,
            prevs: vec![entry.prev_block_id],
            state_update_new: entry.state_update_new,
        });
    }

    let last_group_state =
        processed.last().ok_or_else(|| error!("MC package is empty")).map(|e| LastGroupState {
            mc_block_id: e.block_id.clone(),
            shard_tops: mc_shard_tops.last().map(|(_, tops)| tops.clone()).unwrap_or_default(),
        })?;
    Ok((processed, key_block, mc_shard_tops, last_group_state))
}

fn deserialize_shard_entry(
    block_id: BlockIdExt,
    raw: RawEntry,
    skip: bool,
) -> Result<ProcessedEntry> {
    if raw.proof_data.is_empty() {
        return Err(error!("Shard block {} has no proof link in the package", block_id));
    }
    if raw.block_data.is_empty() {
        return Err(error!("Shard block {} has no block data in the package", block_id));
    }

    if !skip {
        let proof = BlockProofStuff::deserialize(&block_id, raw.proof_data.clone(), true)?;
        proof.check_proof_link()?;
    }

    let block = parse_and_verify_block(&raw.block_data, &block_id)?;
    let info = block.read_info()?;
    let prevs = info.read_prev_ids()?;
    let state_update_new = block.read_state_update()?.new;

    Ok(ProcessedEntry {
        gen_utime: info.gen_utime(),
        end_lt: info.end_lt(),
        mc_ref_seq_no: 0,
        is_key_block: false,
        proof_offset: raw.proof_offset,
        block_offset: raw.block_offset,
        block_id,
        prevs,
        state_update_new,
    })
}

fn parse_shard_entries(
    raw: HashMap<BlockIdExt, RawEntry>,
    archive_id: u32,
    shard: ShardIdent,
    mc_shard_tops: Vec<(u32, Vec<BlockIdExt>)>,
    prev_shard_tops: Vec<BlockIdExt>,
    skip: bool,
) -> Result<Vec<ProcessedEntry>> {
    let now = Instant::now();
    let results: HashMap<BlockIdExt, ProcessedEntry> = raw
        .into_par_iter()
        .map(|(id, r)| deserialize_shard_entry(id.clone(), r, skip).map(|res| (id, res)))
        .collect::<Result<_>>()?;
    log::debug!(target: TARGET, "Deserialized shard entries after {:#?}", now.elapsed());

    let entries = if !skip {
        let prev_committed: HashSet<BlockIdExt> =
            prev_shard_tops.into_iter().filter(|id| id.shard_id.intersect_with(&shard)).collect();
        validate_shard_and_assign_mc_refs(&shard, mc_shard_tops, results, prev_committed)?
    } else {
        // mc_ref_seq_no must be >= archive_id for choose_package() to find the right file.
        let mut entries: Vec<ProcessedEntry> = results
            .into_iter()
            .map(|(_, mut entry)| {
                entry.mc_ref_seq_no = archive_id;
                entry
            })
            .collect();
        entries.sort_by_key(|e| e.block_id.seq_no());
        entries
    };

    Ok(entries)
}

fn validate_shard_and_assign_mc_refs(
    shard: &ShardIdent,
    mut mc_shard_tops: Vec<(u32, Vec<BlockIdExt>)>,
    mut blocks: HashMap<BlockIdExt, ProcessedEntry>,
    prev_committed: HashSet<BlockIdExt>,
) -> Result<Vec<ProcessedEntry>> {
    if blocks.len() == 0 {
        return Ok(vec![]);
    }

    let mut known: HashSet<BlockIdExt> = prev_committed;
    mc_shard_tops.sort_by_key(|(seqno, _)| *seqno);

    let mut entries = Vec::with_capacity(blocks.len());
    for (mc_seqno, tops) in mc_shard_tops {
        for top in tops {
            if !top.shard_id.intersect_with(shard) {
                continue;
            }
            let mut current = top;
            loop {
                if known.contains(&current) {
                    break;
                }
                if let Some(mut entry) = blocks.remove(&current) {
                    entry.mc_ref_seq_no = mc_seqno;
                    let mut prevs = entry.prevs.clone();
                    entries.push(entry);
                    // blocks before merge are always committed by MC block
                    if prevs.len() > 1
                        && (blocks.contains_key(&prevs[0]) || blocks.contains_key(&prevs[1]))
                    {
                        fail!("Block {} parents are not committed by MC blocks", current);
                    }
                    let prev =
                        prevs.pop().ok_or_else(|| error!("Block {} has no parents", current))?;
                    known.insert(current);
                    current = prev;
                } else {
                    fail!(
                        "Shard chain break: block {} is not in current package \
                         and was not committed by previous archive group",
                        current,
                    );
                }
            }
        }
    }

    if !blocks.is_empty() {
        fail!("Some blocks in shard {} are not reachable from MC shard_hashes", shard);
    }

    // Sort by seqno ascending: prev block handles must exist when setting next links.
    // This also handles cross-shard deps (parent shard blocks have lower seqno than children after split).
    entries.sort_by_key(|e| e.block_id.seq_no());
    Ok(entries)
}

pub struct Ingester {
    archive_manager: Arc<ArchiveManager>,
    block_handle_storage: Arc<BlockHandleStorage>,
    archive_state_db: Arc<storage::archive_shardstate_db::ArchiveShardStateDb>,
    prev1_block_db: BlockInfoDb,
    prev2_block_db: BlockInfoDb,
    next1_block_db: BlockInfoDb,
    next2_block_db: BlockInfoDb,
    move_files: bool,
    skip_validation: bool,
}

impl Ingester {
    pub fn new(
        archive_manager: Arc<ArchiveManager>,
        block_handle_storage: Arc<BlockHandleStorage>,
        archive_state_db: Arc<storage::archive_shardstate_db::ArchiveShardStateDb>,
        prev1_block_db: BlockInfoDb,
        prev2_block_db: BlockInfoDb,
        next1_block_db: BlockInfoDb,
        next2_block_db: BlockInfoDb,
        move_files: bool,
        skip_validation: bool,
    ) -> Self {
        Self {
            archive_manager,
            block_handle_storage,
            archive_state_db,
            prev1_block_db,
            prev2_block_db,
            next1_block_db,
            next2_block_db,
            move_files,
            skip_validation,
        }
    }

    pub async fn run_groups(
        &self,
        groups: &[PackageGroup],
        start_idx: usize,
        total: usize,
        mut validator: ValidatorState,
        mut last_group_state: LastGroupState,
    ) -> Result<ValidatorState> {
        let mut prefetch: Option<
            tokio::task::JoinHandle<
                Result<(HashMap<BlockIdExt, RawEntry>, Vec<HashMap<BlockIdExt, RawEntry>>)>,
            >,
        > = None;
        let start = Instant::now();

        for (local_idx, group) in groups.iter().enumerate() {
            let global_idx = start_idx + local_idx;
            let elapsed = start.elapsed();
            let eta = (elapsed * total as u32 / (global_idx + 1) as u32).saturating_sub(elapsed);
            log::info!(
                target: TARGET,
                "Processing group {}/{}: archive_id={}, {} shard packages. ETA {:#?}",
                global_idx + 1,
                total,
                group.archive_id,
                group.shard_packages.len(),
                eta,
            );

            let next_prefetch = groups.get(local_idx + 1).map(|next| {
                let mc_path = next.mc_package.path.clone();
                let shard_paths: Vec<_> =
                    next.shard_packages.iter().map(|p| p.path.clone()).collect();
                tokio::spawn(async move {
                    let (mc_raw, shard_raws) = tokio::try_join!(
                        read_raw_package(&mc_path),
                        try_join_all(shard_paths.iter().map(|p| read_raw_package(p))),
                    )?;
                    Ok::<_, Error>((mc_raw, shard_raws))
                })
            });

            let (mc_raw, shard_raws) = match prefetch.take() {
                Some(handle) => {
                    handle.await.map_err(|e| error!("Prefetch task panicked: {}", e))??
                }
                None => tokio::try_join!(
                    read_raw_package(&group.mc_package.path),
                    try_join_all(group.shard_packages.iter().map(|p| read_raw_package(&p.path))),
                )?,
            };

            let (new_validator, new_state) = self
                .ingest_group_from_raw(group, mc_raw, shard_raws, validator, last_group_state)
                .await?;
            validator = new_validator;
            last_group_state = new_state;
            prefetch = next_prefetch;
        }

        self.block_handle_storage.save_full_node_state(
            LAST_APPLIED_MC_BLOCK.to_string(),
            &last_group_state.mc_block_id,
        )?;
        self.block_handle_storage.save_full_node_state(
            SHARD_CLIENT_MC_BLOCK.to_string(),
            &last_group_state.mc_block_id,
        )?;
        self.block_handle_storage
            .save_full_node_state(ARCHIVES_GC_BLOCK.to_string(), &last_group_state.mc_block_id)?;
        self.block_handle_storage
            .save_full_node_state(PSS_KEEPER_MC_BLOCK.to_string(), &last_group_state.mc_block_id)?;

        Ok(validator)
    }

    async fn ingest_group_from_raw(
        &self,
        group: &PackageGroup,
        mc_raw: HashMap<BlockIdExt, RawEntry>,
        shard_raws: Vec<HashMap<BlockIdExt, RawEntry>>,
        validator: ValidatorState,
        prev_group_state: LastGroupState,
    ) -> Result<(ValidatorState, LastGroupState)> {
        let skip = self.skip_validation;
        let expected_first_mc_prev = prev_group_state.mc_block_id;
        let prev_shard_tops = prev_group_state.shard_tops;
        let mc_block_count = mc_raw.len();
        let group_start = Instant::now();

        let t = Instant::now();
        let (mc_entries, key_block, mc_shard_tops, last_group_state, validator) =
            tokio::task::spawn_blocking(move || -> Result<_> {
                let mut v = validator;
                let (entries, key_block, shard_tops, last_state) =
                    parse_mc_entries(mc_raw, &mut v, skip, expected_first_mc_prev)?;
                Ok((entries, key_block, shard_tops, last_state, v))
            })
            .await
            .map_err(|e| error!("MC parse task panicked: {}", e))??;
        let parse_mc_ms = t.elapsed().as_millis();

        let t = Instant::now();
        for entry in &mc_entries {
            self.update_block_handles(entry)?;
        }
        let mc_handles_ms = t.elapsed().as_millis();

        let mc_import_entries: Vec<ImportEntry> =
            mc_entries.iter().flat_map(|e| e.to_import_entries()).collect();

        let archive_id = group.archive_id;
        let shard_parse_handles: Vec<_> = shard_raws
            .into_iter()
            .zip(group.shard_packages.iter())
            .map(|(raw, pkg)| {
                let shard = pkg.shard.clone();
                let tops = mc_shard_tops.clone();
                let prev = prev_shard_tops.clone();
                tokio::task::spawn_blocking(move || {
                    parse_shard_entries(raw, archive_id, shard, tops, prev, skip)
                })
            })
            .collect();
        let archive_state_db = Arc::clone(&self.archive_state_db);
        let fill_mc_states_db = tokio::task::spawn_blocking(move || -> Result<()> {
            for entry in &mc_entries {
                archive_state_db.put_update(&entry.block_id, entry.state_update_new.clone())?;
            }
            Ok(())
        });

        let mc_shard = ShardIdent::masterchain();

        // Run mc_import, mc_states, and the full shard pipeline concurrently.
        let t = Instant::now();
        let mut shard_block_count = 0usize;
        let (_, _, shard_pipeline_ms) = tokio::try_join!(
            // Task 1: import MC package into archive
            self.archive_manager.import_package(
                &group.mc_package.path,
                group.mc_package.archive_id,
                &mc_shard,
                &mc_import_entries,
                false,
                key_block.is_some(),
            ),
            // Task 2: save MC state cells
            async {
                fill_mc_states_db.await.map_err(|e| error!("MC states db task panicked: {}", e))?
            },
            // Task 3: shard pipeline — parse → handles+states → import
            async {
                let t_pipeline = Instant::now();

                // 3a: await shard parse (already spawned above)
                let shard_parse_results: Vec<Vec<ProcessedEntry>> =
                    try_join_all(shard_parse_handles.into_iter().map(|h| async move {
                        h.await.map_err(|e| error!("Shard parse task panicked: {}", e))?
                    }))
                    .await?;

                // 3b: update block handles + save shard state cells
                for shard_entries in &shard_parse_results {
                    shard_block_count += shard_entries.len();
                    for entry in shard_entries {
                        self.update_block_handles(entry)?;
                        self.archive_state_db
                            .put_update(&entry.block_id, entry.state_update_new.clone())?;
                    }
                }

                // 3c: import shard packages into archive
                let shard_import_entries: Vec<Vec<ImportEntry>> = shard_parse_results
                    .iter()
                    .map(|entries| entries.iter().flat_map(|e| e.to_import_entries()).collect())
                    .collect();

                try_join_all(
                    group
                        .shard_packages
                        .iter()
                        .zip(shard_import_entries.iter())
                        .filter(|(_, entries)| !entries.is_empty())
                        .map(|(pkg, import_entries)| {
                            self.archive_manager.import_package(
                                &pkg.path,
                                pkg.archive_id,
                                &pkg.shard,
                                import_entries,
                                self.move_files,
                                false,
                            )
                        }),
                )
                .await?;

                Ok(t_pipeline.elapsed().as_millis())
            },
        )?;
        let parallel_ms = t.elapsed().as_millis();

        let t = Instant::now();
        if let Some(kb) = key_block {
            self.archive_key_block(&kb.block_id, kb.proof_data, kb.block_data).await?;
        }
        let key_block_ms = t.elapsed().as_millis();

        if self.move_files {
            if let Err(e) = tokio::fs::remove_file(&group.mc_package.path).await {
                log::warn!(
                    target: TARGET,
                    "Failed to remove MC pack {} after import: {}",
                    group.mc_package.path.display(),
                    e,
                );
            }
        }

        log::info!(
            target: TARGET,
            "Imported archive {} ({} MC, {} shard blocks, {} shard pkgs) total {:#?}: \
             parse_mc {}ms, mc_handles {}ms, \
             parallel {}ms (shard_pipeline {}ms), key_block {}ms",
            group.archive_id,
            mc_block_count,
            shard_block_count,
            group.shard_packages.len(),
            group_start.elapsed(),
            parse_mc_ms,
            mc_handles_ms,
            parallel_ms,
            shard_pipeline_ms,
            key_block_ms,
        );

        Ok((validator, last_group_state))
    }

    async fn archive_key_block(
        &self,
        block_id: &BlockIdExt,
        proof_data: Vec<u8>,
        block_data: Vec<u8>,
    ) -> Result<()> {
        let handle = self.block_handle_storage.load_handle_by_id(block_id)?.ok_or_else(|| {
            error!("Block handle not found for key block {} during key archive creation", block_id)
        })?;
        self.archive_manager
            .add_block_data_to_package(
                proof_data,
                &handle,
                &PackageEntryId::Proof(block_id.clone()),
                true,
            )
            .await?;
        self.archive_manager
            .add_block_data_to_package(
                block_data,
                &handle,
                &PackageEntryId::Block(block_id.clone()),
                true,
            )
            .await?;
        Ok(())
    }

    fn update_block_handles(&self, entry: &ProcessedEntry) -> Result<()> {
        let meta = BlockMeta::for_import(
            entry.gen_utime,
            entry.end_lt,
            entry.mc_ref_seq_no,
            entry.is_key_block,
            entry.block_id.is_masterchain(),
            entry.prevs.len() > 1,
        );

        if let Some(handle) =
            self.block_handle_storage.create_handle(entry.block_id.clone(), meta, None)?
        {
            log::trace!(
                target: TARGET,
                "Created block handle for {} (key={})",
                entry.block_id,
                entry.is_key_block,
            );
            let _ = handle;
        }

        let prev1 = entry
            .prevs
            .first()
            .ok_or_else(|| error!("Block {} has no prev refs", entry.block_id))?;

        self.prev1_block_db.put(&entry.block_id, &prev1.serialize())?;
        self.store_next_link(&entry.block_id, prev1)?;

        if let Some(prev2) = entry.prevs.get(1) {
            self.prev2_block_db.put(&entry.block_id, &prev2.serialize())?;
            self.store_next_link(&entry.block_id, prev2)?;
        }

        Ok(())
    }

    fn store_next_link(&self, block_id: &BlockIdExt, prev_id: &BlockIdExt) -> Result<()> {
        let prev_handle =
            self.block_handle_storage.load_handle_by_id(prev_id)?.ok_or_else(|| {
                error!("Block handle not found for prev block {} of {}", prev_id, block_id)
            })?;

        let prev_shard = prev_id.shard();
        let shard = block_id.shard();
        if prev_shard != shard && prev_shard.split()?.1 == *shard {
            // After split: right child → next2
            self.next2_block_db.put(prev_id, &block_id.serialize())?;
            prev_handle.set_next2();
        } else {
            // Simple chain or after merge or left child → next1
            self.next1_block_db.put(prev_id, &block_id.serialize())?;
            prev_handle.set_next1();
        }
        self.block_handle_storage.save_handle(&prev_handle, None)?;
        Ok(())
    }
}
