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
    block::{BlockIdExtExtention, BlockStuff},
    block_proof::BlockProofStuff,
    boot,
    engine_traits::EngineOperations,
};
use adnl::common::Wait;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt::{Debug, Display, Formatter},
    mem::replace,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use storage::{
    archives::{
        package::read_package_from, package_entry_id::PackageEntryId, ARCHIVE_PACKAGE_SIZE,
    },
    block_handle_db::BlockHandle,
};
use ton_block::{error, fail, BlockIdExt, Result, ShardIdent, BASE_WORKCHAIN_ID};

const TARGET: &str = "sync";
const SHARD_RETRIES: u8 = 10;

#[async_trait::async_trait]
pub trait StopSyncChecker {
    async fn check(&self, engine: &Arc<dyn EngineOperations>) -> bool;
}

struct ArchiveContext {
    absent_shards: HashMap<ShardIdent, u8>,
    loaded_shards: HashMap<ShardIdent, BlockMaps>,
    master: Option<BlockMaps>,
    need_shards: bool,
    shards_count: usize,
}

impl ArchiveContext {
    fn calc_downloaded(&self) -> usize {
        let master = if self.master.is_some() { 1 } else { 0 };
        if self.need_shards {
            master + self.loaded_shards.len()
        } else {
            master + self.shards_count
        }
    }
    fn has_retryable_shards(&self) -> bool {
        self.absent_shards.values().any(|&retries| retries > 0)
    }
}

#[derive(Default, Debug)]
struct BlocksEntry {
    block: Option<Arc<BlockStuff>>,
    proof: Option<Arc<BlockProofStuff>>,
}

#[derive(Debug)]
struct BlockMaps {
    mc_blocks_ids: Arc<BTreeMap<u32, Arc<BlockIdExt>>>,
    blocks: BTreeMap<Arc<BlockIdExt>, BlocksEntry>,
}

#[derive(Clone)]
struct SyncContext {
    concurrency: usize,
    downloads: usize,
    engine: Arc<dyn EngineOperations>,
    wait: Arc<WaitBlockWithResult>,
}

type WaitBlockWithResult = Wait<(u32, Result<(ArchiveContext, usize)>)>;

pub(crate) async fn start_sync(
    engine: Arc<dyn EngineOperations>,
    check_stop_sync: Option<&dyn StopSyncChecker>,
    max_concurrency: Option<usize>,
) -> Result<()> {
    async fn apply(
        sync_context: &mut SyncContext,
        mc_seq_no: u32,
        last_mc_block_id: &Arc<BlockIdExt>,
        archive_context: ArchiveContext,
    ) -> Result<Option<ArchiveContext>> {
        import_mc_blocks(sync_context, last_mc_block_id, &archive_context).await?;
        if let Some(ret) = import_shard_blocks(sync_context, mc_seq_no, archive_context).await? {
            Ok(Some(ret))
        } else {
            log::info!(target: TARGET, "Archive imported for MC seq_no = {mc_seq_no}");
            Ok(None)
        }
    }

    fn download(
        sync_context: &mut SyncContext,
        mc_seq_no: u32,
        archive_context: Option<ArchiveContext>,
    ) {
        sync_context.wait.request();
        let archive_context = archive_context.unwrap_or_else(|| {
            let shards_count = 1 << sync_context.engine.get_monitor_min_split();
            sync_context.downloads += shards_count + 1;
            log::info!(
                target: TARGET,
                "downloads mc={mc_seq_no} update: {} => {}",
                sync_context.downloads - shards_count - 1, sync_context.downloads,
            );
            ArchiveContext {
                absent_shards: HashMap::new(),
                loaded_shards: HashMap::new(),
                master: None,
                need_shards: shards_count > 0,
                shards_count,
            }
        });
        let sync_context = sync_context.clone();
        tokio::spawn(async move {
            let res = download_archives(&sync_context, mc_seq_no, archive_context).await;
            sync_context.wait.respond(Some((mc_seq_no, res)));
        });
        log::info!(target: TARGET, "Download scheduled for MC seq_no = {mc_seq_no}");
    }

    async fn force_redownload(
        sync_context: &mut SyncContext,
        queue: &mut [(u32, ArchiveStatus)],
    ) -> Result<()> {
        let mut all = String::new();
        let mut new = String::new();
        let mut log = |seq_no| {
            if !new.is_empty() {
                new.push(',');
            }
            new.push_str(format!("{seq_no}").as_str());
        };
        // Find latest downloaded archive
        let mut latest = None;
        queue.iter().for_each(|(seq_no, status)| {
            if let ArchiveStatus::Downloaded(_) = status {
                if latest.as_ref().map_or(true, |latest| latest < seq_no) {
                    latest = Some(*seq_no)
                }
            }
            if !all.is_empty() {
                all.push_str(", ");
            }
            all.push_str(format!("{seq_no}/{status}").as_str());
        });
        // Redownload previous not found ones
        if let Some(latest) = latest {
            for (seq_no, status) in queue.iter_mut() {
                match status {
                    ArchiveStatus::Incomplete(_) if latest > *seq_no => (),
                    _ => continue,
                }
                redownload_archive(sync_context, seq_no, status, 1)?;
                log(*seq_no);
            }
        }
        // Redownload earliest incomplete if only incomplete remained
        if !sync_context.engine.check_sync().await? {
            let mut earliest = None;
            if queue
                .iter()
                .find(|(seq_no, status)| {
                    if let ArchiveStatus::Incomplete(_) = status {
                        if earliest.as_ref().map_or(true, |earliest| earliest > seq_no) {
                            earliest = Some(*seq_no)
                        }
                        false
                    } else {
                        true
                    }
                })
                .is_none()
            {
                if let Some(earliest) = earliest {
                    queue
                        .iter_mut()
                        .find(|(seq_no, _)| earliest == *seq_no)
                        .map(|(seq_no, status)| {
                            redownload_archive(sync_context, seq_no, status, 2)?;
                            log(*seq_no);
                            Ok(())
                        })
                        .unwrap_or_else(|| {
                            fail!("INTERNAL ERROR: Archive {earliest} not found in queue")
                        })?;
                }
            }
        }
        log::info!(target: TARGET, "Force redownloads over queue {all}: [{new}]");
        Ok(())
    }

    async fn is_stopped(
        check_stop_sync: &Option<&dyn StopSyncChecker>,
        engine: &Arc<dyn EngineOperations>,
    ) -> bool {
        if let Some(check_stop_sync) = check_stop_sync.as_ref() {
            if check_stop_sync.check(engine).await {
                log::info!(target: TARGET, "Sync is managed to stop");
                return true;
            }
        }
        if engine.check_stop() {
            log::info!(target: TARGET, "Engine is stopping, quit sync");
            true
        } else {
            false
        }
    }

    async fn new_downloads(
        sync_context: &mut SyncContext,
        queue: &mut Vec<(u32, ArchiveStatus)>,
        sync_mc_seq_no: u32,
    ) -> Result<()> {
        force_redownload(sync_context, queue).await?;
        // Gap-close: nothing in queue moves us past sync_mc_seq_no.
        if !queue.iter().any(|(s, _)| *s <= sync_mc_seq_no) {
            queue.push((sync_mc_seq_no, ArchiveStatus::Downloading));
            download(sync_context, sync_mc_seq_no, None);
        }
        // Prefetch above the max queued entry to avoid overlapping with existing.
        let mut next =
            queue.iter().map(|(s, _)| *s).max().unwrap_or(sync_mc_seq_no) + ARCHIVE_PACKAGE_SIZE;
        while sync_context.downloads < sync_context.concurrency {
            if queue.len() > sync_context.concurrency {
                // Do not download too much in advance due to possible OOM
                break;
            }
            queue.push((next, ArchiveStatus::Downloading));
            download(sync_context, next, None);
            next += ARCHIVE_PACKAGE_SIZE;
        }
        log::info!(
            target: TARGET,
            "Active downloads: {}, queue size: {}",
            sync_context.downloads, queue.len()
        );
        Ok(())
    }

    fn redownload_archive(
        sync_context: &mut SyncContext,
        seq_no: &u32,
        status: &mut ArchiveStatus,
        tag: u8,
    ) -> Result<()> {
        let archive_context = match replace(status, ArchiveStatus::Downloading) {
            ArchiveStatus::Incomplete(archive_context) => archive_context,
            what => fail!(
                "INTERNAL ERROR: Unexpected status of archive for MC seq_no = {seq_no}: {}, {tag}",
                what
            ),
        };
        download(sync_context, *seq_no, Some(archive_context));
        Ok(())
    }

    enum ArchiveStatus {
        Downloading,
        Downloaded(ArchiveContext),
        Incomplete(ArchiveContext),
    }

    impl Display for ArchiveStatus {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                ArchiveStatus::Downloading => write!(f, "Downloading"),
                ArchiveStatus::Downloaded(_) => write!(f, "Downloaded"),
                ArchiveStatus::Incomplete(_) => write!(f, "Incomplete"),
            }
        }
    }

    const MAX_CONCURRENCY: usize = 10;
    let max_concurrency = max_concurrency.unwrap_or(MAX_CONCURRENCY);
    let (wait, mut reader) = Wait::new();

    log::info!(
        target: TARGET,
        "Started sync, monitor_min_split {}",
        engine.get_monitor_min_split()
    );

    let mut queue: Vec<(u32, ArchiveStatus)> = Vec::new();
    let mut sync_context = SyncContext { concurrency: 1, downloads: 0, engine, wait };

    'check: while !sync_context.engine.check_sync().await? {
        if is_stopped(&check_stop_sync, &sync_context.engine).await {
            return Ok(());
        }

        // Select sync block ID
        let mc_block_id = if let Some(id) = sync_context.engine.load_last_applied_mc_block_id()? {
            id
        } else {
            fail!("INTERNAL ERROR: No last applied MC block in sync")
        };
        let sc_block_id = if let Some(id) = sync_context.engine.load_shard_client_mc_block_id()? {
            id
        } else {
            fail!("INTERNAL ERROR: No shard client MC block in sync")
        };
        let last_mc_block_id = if mc_block_id.seq_no() > sc_block_id.seq_no() {
            Arc::clone(&sc_block_id)
        } else {
            Arc::clone(&mc_block_id)
        };

        log::info!(
            target: TARGET,
            "Last MC seq_no for sync = {} (MC = {mc_block_id}, SC = {sc_block_id})",
            last_mc_block_id.seq_no()
        );

        // Try to find proper # in queue
        let sync_mc_seq_no = last_mc_block_id.seq_no() + 1;
        loop {
            new_downloads(&mut sync_context, &mut queue, sync_mc_seq_no).await?;
            if let Some(index) = queue.iter().position(|(seq_no, status)| match status {
                ArchiveStatus::Downloaded(_) => seq_no <= &sync_mc_seq_no,
                _ => false,
            }) {
                if let (seq_no, ArchiveStatus::Downloaded(archive_context)) = queue.remove(index) {
                    let before = archive_context.calc_downloaded();
                    match apply(&mut sync_context, seq_no, &last_mc_block_id, archive_context).await
                    {
                        Ok(Some(archive_context)) => {
                            let delta = before - archive_context.calc_downloaded();
                            sync_context.downloads += delta;
                            log::info!(
                                target: TARGET,
                                "downloads mc={seq_no} apply-queued-incomplete: {} +{delta} => {}",
                                sync_context.downloads - delta, sync_context.downloads,
                            );
                            queue.insert(
                                index,
                                (seq_no, ArchiveStatus::Incomplete(archive_context)),
                            );
                            force_redownload(&mut sync_context, &mut queue).await?
                        }
                        Ok(None) => continue 'check,
                        Err(e) => log::error!(
                            target: TARGET,
                            "Cannot apply queued package for MC seq_no = {seq_no}: {e}"
                        ),
                    }
                } else {
                    fail!("INTERNAL ERROR: sync queue broken")
                }
            } else {
                break;
            }
        }

        log::info!(target: TARGET, "Continue sync with MC seq_no {sync_mc_seq_no}");

        // Otherwise download
        while !sync_context.engine.check_sync().await? {
            if is_stopped(&check_stop_sync, &sync_context.engine).await {
                return Ok(());
            }
            new_downloads(&mut sync_context, &mut queue, sync_mc_seq_no).await?;
            match sync_context.wait.wait(&mut reader, false).await {
                Some(Some((seq_no, Err(e)))) => {
                    log::error!(
                        target: TARGET,
                        "Error while downloading package seq_no {seq_no}: {e}"
                    );
                    download(&mut sync_context, seq_no, None)
                }
                Some(Some((seq_no_recv, Ok((archive_context, update))))) => {
                    let Some(index) = queue.iter().position(|(seq_no_send, status)| match status {
                        ArchiveStatus::Downloading => seq_no_send == &seq_no_recv,
                        _ => false,
                    }) else {
                        fail!("INTERNAL ERROR: sync queue broken")
                    };
                    sync_context.downloads -= update;
                    log::info!(
                        target: TARGET,
                        "downloads mc={seq_no_recv} response: {} -{update} => {}",
                        sync_context.downloads + update, sync_context.downloads,
                    );
                    let incomplete =
                        archive_context.master.is_none() || archive_context.has_retryable_shards();
                    let archive_context = if incomplete {
                        Some(archive_context)
                    } else if seq_no_recv <= last_mc_block_id.seq_no() + 1 {
                        let before = archive_context.calc_downloaded();
                        match apply(
                            &mut sync_context,
                            seq_no_recv,
                            &last_mc_block_id,
                            archive_context,
                        )
                        .await
                        {
                            Ok(Some(archive_context)) => {
                                let delta = before - archive_context.calc_downloaded();
                                sync_context.downloads += delta;
                                log::info!(
                                    target: TARGET,
                                    "downloads mc={seq_no_recv} apply-fresh-incomplete: {} +{delta} => {}",
                                    sync_context.downloads - delta, sync_context.downloads,
                                );
                                Some(archive_context)
                            }
                            Ok(None) => {
                                queue.remove(index);
                                sync_context.concurrency = max_concurrency;
                                break;
                            }
                            Err(e) => {
                                log::error!(
                                    target: TARGET,
                                    "Cannot apply downloaded package for MC seq_no = {seq_no_recv}: {e}"
                                );
                                download(&mut sync_context, seq_no_recv, None);
                                None
                            }
                        }
                    } else {
                        let (_, status) = &mut queue[index];
                        *status = ArchiveStatus::Downloaded(archive_context);
                        force_redownload(&mut sync_context, &mut queue).await?;
                        None
                    };
                    if let Some(mut archive_context) = archive_context {
                        if !archive_context.has_retryable_shards()
                            && archive_context.master.is_some()
                        {
                            // All absent shard retries exhausted — skip this archive
                            // and move on rather than looping forever
                            log::warn!(
                                target: TARGET,
                                "Giving up on MC seq_no {seq_no_recv}: \
                                shard retries exhausted for {:?}",
                                archive_context.absent_shards.keys().collect::<Vec<_>>()
                            );
                            queue.remove(index);
                            sync_context.concurrency = max_concurrency;
                            break;
                        }
                        let mut msg = format!(
                            "{}, need shards {}",
                            if archive_context.master.is_none() {
                                "master absent"
                            } else {
                                "master present"
                            },
                            archive_context.need_shards,
                        );
                        for (shard, retries) in &archive_context.absent_shards {
                            msg.push_str(
                                format!(", shard {shard} absent (retries={retries})").as_str(),
                            );
                        }
                        for shard in archive_context.loaded_shards.keys() {
                            msg.push_str(format!(", shard {shard} loaded").as_str());
                        }
                        log::info!(
                            target: TARGET,
                            "Incomplete archive detected for MC seq_no {seq_no_recv}: {msg}"
                        );
                        let (_, status) = &mut queue[index];
                        if archive_context.has_retryable_shards() {
                            archive_context.need_shards = true;
                        }
                        *status = ArchiveStatus::Incomplete(archive_context);
                        force_redownload(&mut sync_context, &mut queue).await?;
                        continue;
                    }
                }
                Some(None) => fail!("INTERNAL ERROR: sync broken: Some(None)"),
                None => fail!(
                    "INTERNAL ERROR: sync broken: {} jobs awaiting",
                    sync_context.wait.count()
                ),
            }
        }
    }

    log::info!(target: TARGET, "Sync complete");
    Ok(())
}

async fn download_archive(
    sync_context: &SyncContext,
    shard: Option<ShardIdent>,
    mc_seq_no: u32,
) -> Result<Option<Vec<u8>>> {
    let msg = get_archive_title(&shard);
    log::info!(target: TARGET, "Requesting {} for MC seq_no = {}", msg, mc_seq_no);
    match sync_context.engine.download_archive(shard, mc_seq_no).await {
        Ok(Some(data)) => {
            log::info!(
                target: TARGET,
                "Downloaded {} for MC seq_no = {}, package size = {} bytes",
                msg, mc_seq_no, data.len()
            );
            Ok(Some(data))
        }
        Ok(None) => {
            log::info!(target: TARGET, "No {} found for MC seq_no = {}", msg, mc_seq_no);
            Ok(None)
        }
        Err(e) => fail!("Download {msg} failed for MC seq_no = {mc_seq_no}, err: {e}"),
    }
}

async fn download_archives(
    sync_context: &SyncContext,
    mc_seq_no: u32,
    mut archive_context: ArchiveContext,
) -> Result<(ArchiveContext, usize)> {
    let before = archive_context.calc_downloaded();
    let mut tasks = Vec::new();
    if archive_context.master.is_none() {
        let context = sync_context.clone();
        let task =
            tokio::spawn(async move { (None, download_archive(&context, None, mc_seq_no).await) });
        tasks.push(task);
    }
    if archive_context.need_shards {
        let mut scheduled_shards = HashSet::new();
        for i in 0..archive_context.shards_count {
            let shard = ShardIdent::with_tagged_prefix(
                BASE_WORKCHAIN_ID,
                ((i as u64) * 2 + 1) << (63 - sync_context.engine.get_monitor_min_split()),
            )?;
            scheduled_shards.insert(shard.clone());
            if archive_context.loaded_shards.get(&shard).is_some() {
                continue;
            }
            let retry = archive_context.absent_shards.entry(shard.clone()).or_insert(SHARD_RETRIES);
            if *retry == 0 {
                continue;
            }
            *retry -= 1;
            let context = sync_context.clone();
            let task = tokio::spawn(async move {
                let shard = Some(shard);
                (shard.clone(), download_archive(&context, shard, mc_seq_no).await)
            });
            tasks.push(task);
        }
        // Decrement retries for absent shards not covered by min_split
        // (e.g. shards from a different split depth in the MC block)
        for (shard, retries) in archive_context.absent_shards.iter_mut() {
            if scheduled_shards.contains(shard) || *retries == 0 {
                continue;
            }
            log::warn!(
                target: TARGET,
                "Shard {shard} absent but not in min_split layout, \
                decrementing retries ({retries} -> {})",
                *retries - 1
            );
            *retries -= 1;
        }
    }
    if tasks.is_empty() {
        return Ok((archive_context, 0));
    }

    let results = futures::future::try_join_all(tasks).await?;
    for (shard, result) in results {
        let Ok(Some(data)) = result else { continue };
        let msg = get_archive_title(&shard);
        log::info!(target: TARGET, "Reading {msg} for MC seq_no = {mc_seq_no}");
        let maps = match read_package(&data).await {
            Ok(maps) => maps,
            Err(e) => {
                log::warn!(
                    target: TARGET,
                    "Error while parsing {msg} MC seq_no = {mc_seq_no}: {e}"
                );
                continue;
            }
        };
        if let Some(shard) = shard {
            log::info!(
                target: TARGET,
                "Downloaded {msg} for MC seq_no = {mc_seq_no} contains {} shard blocks",
                maps.blocks.len() - maps.mc_blocks_ids.len()
            );
            archive_context.absent_shards.remove(&shard);
            archive_context.loaded_shards.insert(shard, maps);
        } else {
            log::info!(
                target: TARGET,
                "Downloaded {msg} for MC seq_no = {mc_seq_no} contains \
                {} masterchain blocks, {} blocks overall",
                maps.mc_blocks_ids.len(), maps.blocks.len(),
            );
            if maps.mc_blocks_ids.keys().next().is_none() {
                fail!(
                    "Downloaded {msg} for MC seq_no = {mc_seq_no} \
                    doesn't contain masterchain blocks!"
                );
            }
            archive_context.master = Some(maps)
        }
    }

    let after = archive_context.calc_downloaded();
    let Some(master) = archive_context.master.as_ref() else {
        return Ok((archive_context, after - before));
    };
    let check = archive_context.need_shards
        && archive_context.loaded_shards.is_empty()
        && (master.mc_blocks_ids.len() < master.blocks.len());
    if check {
        for (id, _) in master.blocks.iter() {
            if !id.is_masterchain() {
                archive_context.need_shards = false;
                break;
            }
        }
    }
    Ok((archive_context, after - before))
}

fn get_archive_title(shard: &Option<ShardIdent>) -> String {
    if let Some(shard) = shard {
        format!("shard {} archive", shard)
    } else {
        "masterchain archive".to_string()
    }
}

async fn import_mc_blocks(
    sync_context: &mut SyncContext,
    mut last_mc_block_id: &Arc<BlockIdExt>,
    archive_context: &ArchiveContext,
) -> Result<()> {
    let Some(maps) = archive_context.master.as_ref() else {
        fail!("No masterchain data in archive for MC seq_no = {}", last_mc_block_id.seq_no() + 1)
    };

    for (_, id) in maps.mc_blocks_ids.iter() {
        if id.seq_no() <= last_mc_block_id.seq_no() {
            if (id.seq_no() == last_mc_block_id.seq_no()) && (last_mc_block_id != id) {
                fail!("Bad old masterchain block ID");
            }
            log::debug!(target: TARGET, "Skipped already applied MC block {id}");
            continue;
        }
        if id.seq_no() != last_mc_block_id.seq_no() + 1 {
            fail!(
                "There is a hole in the masterchain seq_no! \
                Last applied seq_no = {}, current seq_no = {}",
                last_mc_block_id.seq_no(),
                id.seq_no()
            );
        }
        log::debug!(target: TARGET, "Importing MC block: {id}");
        last_mc_block_id = id;
        if let Some(handle) = sync_context.engine.load_block_handle(last_mc_block_id)? {
            if handle.is_applied() {
                log::debug!(target: TARGET, "Skipped already applied MC block {last_mc_block_id}");
                continue;
            }
        }

        let Some(entry) = maps.blocks.get(last_mc_block_id) else {
            fail!("Inconsistent blocks map: block {} is missing", last_mc_block_id.seq_no());
        };
        let (handle, block, _proof) =
            save_block(&sync_context.engine, last_mc_block_id, entry).await?;
        log::debug!(target: TARGET, "Applying masterchain block: {last_mc_block_id}...");
        sync_context
            .engine
            .clone()
            .apply_block(&handle, &block, last_mc_block_id.seq_no(), false)
            .await?;
    }

    log::debug!(target: TARGET, "Last applied MC seq_no = {}", last_mc_block_id.seq_no());
    Ok(())
}

async fn import_shard_blocks(
    sync_context: &mut SyncContext,
    mc_seq_no: u32,
    mut archive_context: ArchiveContext,
) -> Result<Option<ArchiveContext>> {
    let Some(mut maps) = archive_context.master else {
        fail!("No masterchain data in archive for MC block: {mc_seq_no}")
    };
    if archive_context.need_shards {
        for (_, shard) in archive_context.loaded_shards.iter_mut() {
            for (id, entry) in shard.blocks.iter() {
                if !id.is_masterchain() {
                    save_block(&sync_context.engine, id, entry).await?;
                }
            }
            maps.blocks.append(&mut shard.blocks);
        }
    } else {
        for (id, entry) in maps.blocks.iter() {
            if !id.is_masterchain() {
                save_block(&sync_context.engine, id, entry).await?;
            }
        }
    }

    let maps = Arc::new(maps);
    let absent_blocks = Arc::new(AtomicU32::new(0));
    let mut total_blocks = 0;
    let mut shard_client_mc_block_id = match sync_context.engine.load_shard_client_mc_block_id()? {
        Some(id) => id,
        None => fail!("INTERNAL ERROR: No shard client MC block set in sync"),
    };

    for mc_block_id in maps.mc_blocks_ids.values() {
        if mc_block_id.seq_no() <= shard_client_mc_block_id.seq_no() {
            log::debug!(
                target: TARGET,
                "Skipped shardchain blocks for already appplied MC block: {mc_block_id}"
            );
            continue;
        }
        log::debug!(target: TARGET, "Importing shardchain blocks for MC block: {mc_block_id}...");
        let mc_handle = sync_context
            .engine
            .load_block_handle(mc_block_id)?
            .ok_or_else(|| error!("Cannot load handle for master block {mc_block_id}"))?;
        let shard_blocks =
            sync_context.engine.load_block(&mc_handle).await?.top_blocks(BASE_WORKCHAIN_ID)?;

        let mut bad_shards = HashSet::new();
        let mut tasks: Vec<tokio::task::JoinHandle<Result<BlockIdExt>>> =
            Vec::with_capacity(shard_blocks.len());
        for id in shard_blocks {
            let absent_blocks = absent_blocks.clone();
            let engine = sync_context.engine.clone();
            let mc_handle = mc_handle.clone();
            let maps = maps.clone();
            total_blocks += 1;
            bad_shards.insert(id.shard().clone());
            let task = tokio::spawn(async move {
                log::debug!(
                    target: TARGET,
                    "Importing shardchain block: {id} for MC block: {}...",
                    mc_handle.id()
                );
                if let Some(handle) = engine.load_block_handle(&id)? {
                    if handle.is_applied() {
                        log::debug!(target: TARGET, "Skipped already applied block: {id}");
                        return Ok(id);
                    }
                    if id.seq_no() == 0 {
                        log::info!(target: TARGET, "Downloading zerostate: {id}...");
                        boot::download_zerostate(engine.as_ref(), &id).await?;
                        return Ok(id);
                    }
                    log::debug!(target: TARGET, "Applying shardchain block: {id}...");
                    let block = match maps.blocks.get(&id) {
                        Some(entry) => match entry.block {
                            Some(ref block) => Some(block.as_ref().clone()),
                            None => engine.load_block(&handle).await.ok(),
                        },
                        None => {
                            log::warn!(
                                target: TARGET,
                                "Shard block is not found in the package: {id}"
                            );
                            engine.load_block(&handle).await.ok()
                        }
                    };
                    if let Some(block) = block {
                        engine.apply_block(&handle, &block, mc_handle.id().seq_no(), false).await?;
                        return Ok(id);
                    }
                }
                log::warn!(
                    target: TARGET,
                    "Shard block {id} is not found either in the package or among \
                    unapplied blocks. Will try to download it directly"
                );
                absent_blocks.fetch_add(1, Ordering::Relaxed);
                engine.download_and_apply_block(&id, mc_handle.id().seq_no(), false).await?;
                Ok(id)
            });
            tasks.push(task)
        }

        for res in futures::future::try_join_all(tasks).await?.into_iter() {
            match res {
                Ok(id) => {
                    bad_shards.remove(id.shard());
                }
                Err(e) => log::error!(target: TARGET, "Cannot import archive: {e}"),
            }
        }
        if !bad_shards.is_empty() {
            for shard in bad_shards {
                archive_context.loaded_shards.remove(&shard);
                // Use or_insert to avoid resetting retry counter for
                // shards that already exhausted their download attempts
                archive_context.absent_shards.entry(shard).or_insert(SHARD_RETRIES);
            }
            let Ok(maps) = Arc::try_unwrap(maps) else {
                fail!("INTERNAL ERROR: archive master maps are locked")
            };
            let ret = ArchiveContext {
                absent_shards: archive_context.absent_shards,
                loaded_shards: archive_context.loaded_shards,
                master: Some(maps),
                need_shards: archive_context.need_shards,
                shards_count: archive_context.shards_count,
            };
            return Ok(Some(ret));
        }
        shard_client_mc_block_id = mc_block_id.clone();
        sync_context.engine.save_shard_client_mc_block_id(mc_block_id)?;
    }

    let absent_blocks = absent_blocks.load(Ordering::Relaxed);
    if absent_blocks > 0 {
        log::info!(
            target: TARGET,
            "Downloaded shard archives for MC seq_no = {mc_seq_no} \
            miss {absent_blocks} blocks among {total_blocks} total ones"
        );
    }
    let delta = archive_context.shards_count - archive_context.loaded_shards.len();
    sync_context.downloads -= delta;
    log::info!(
        target: TARGET,
        "downloads mc={mc_seq_no} import-done: {} -{delta} => {}",
        sync_context.downloads + delta, sync_context.downloads,
    );
    Ok(None)
}

async fn read_package(data: &[u8]) -> Result<BlockMaps> {
    let mut reader = read_package_from(data).await?;
    let mut blocks = BTreeMap::new();
    let mut mc_blocks_ids = BTreeMap::new();

    while let Some(entry) = reader.next().await? {
        log::trace!(
            target: TARGET,
            "Processing archive entry: {}, size = {}",
            entry.filename(),
            entry.data().len()
        );
        let entry_id = PackageEntryId::from_filename(entry.filename())?;

        match entry_id {
            PackageEntryId::Block(id) => {
                let id = Arc::new(id);
                blocks.entry(id.clone()).or_insert_with(BlocksEntry::default).block =
                    Some(Arc::new(BlockStuff::deserialize_block_checked(
                        (*id).clone(),
                        Arc::new(entry.take_data()),
                    )?));
                if id.is_masterchain() {
                    mc_blocks_ids.insert(id.seq_no(), id);
                }
            }

            PackageEntryId::Proof(id) => {
                if !id.is_masterchain() {
                    log::warn!(
                        target: TARGET,
                        "Proof for shard chain must be skipped: {}, entry filename: {}",
                        id,
                        entry.filename()
                    );
                    continue;
                }
                let id = Arc::new(id);
                blocks.entry(id.clone()).or_insert_with(BlocksEntry::default).proof =
                    Some(Arc::new(BlockProofStuff::deserialize(&id, entry.take_data(), false)?));
                mc_blocks_ids.insert(id.seq_no(), id);
            }

            PackageEntryId::ProofLink(id) => {
                if id.is_masterchain() {
                    log::warn!(
                        target: TARGET,
                        "Proof-link for masterchain must be skipped: {}, entry filename: {}",
                        id,
                        entry.filename()
                    );
                    continue;
                }
                blocks.entry(Arc::new(id)).or_insert_with(BlocksEntry::default).proof =
                    Some(Arc::new(BlockProofStuff::deserialize(&id, entry.take_data(), true)?));
            }

            _ => fail!("Unsupported entry: {:?}", entry_id),
        }
    }

    let maps = BlockMaps { blocks, mc_blocks_ids: Arc::new(mc_blocks_ids) };
    Ok(maps)
}

async fn save_block(
    engine: &Arc<dyn EngineOperations>,
    block_id: &BlockIdExt,
    entry: &BlocksEntry,
) -> Result<(Arc<BlockHandle>, Arc<BlockStuff>, Arc<BlockProofStuff>)> {
    log::trace!(target: TARGET, "save_block: id = {}", block_id);
    let block = if let Some(ref block) = entry.block {
        Arc::clone(block)
    } else {
        fail!("Block not found in archive: {}", block_id);
    };
    let proof = if let Some(ref proof) = entry.proof {
        Arc::clone(proof)
    } else {
        let link_str = if block_id.shard().is_masterchain() { "" } else { "link" };
        fail!("Proof{} not found in archive: {}", link_str, block_id);
    };
    proof.check_proof(engine.as_ref()).await?;
    let handle = engine.store_block(&block).await?.to_non_created().ok_or_else(|| {
        error!("INTERNAL ERROR: mismatch in block {} store result during sync", block_id)
    })?;
    let handle = engine
        .store_block_proof(block_id, Some(handle), &proof)
        .await?
        .to_non_created()
        .ok_or_else(|| {
        error!("INTERNAL ERROR: mismatch in block {} proof store result during sync", block_id)
    })?;
    Ok((handle, block, proof))
}

#[cfg(test)]
#[path = "tests/test_sync.rs"]
mod tests;
