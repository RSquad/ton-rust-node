/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    engine_traits::EngineOperations, internal_db::state_gc_resolver::AllowStateGcSmartResolver,
    types::awaiters_pool::AwaitersPool, validator::out_msg_queue::build_proofs,
};
use std::{
    cmp::{Ord, PartialOrd},
    collections::HashMap,
    fmt::{Display, Formatter},
    ops::Deref,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use storage::shardstate_db_async::AllowStateGcResolver;
use ton_api::ton::ton_node::OutMsgQueueProof;
use ton_block::{
    error, fail, read_boc, Block, BlockIdExt, ConfigParams, Deserializable, ImportedMsgQueueLimits,
    MerkleProof, OutMsgQueueInfo, Result, ShardIdent, ShardStateUnsplit,
};

#[derive(Ord, PartialOrd, Eq, PartialEq, Hash, Clone)]
struct ShardNeighbours {
    shard: ShardIdent,
    neighbours: Vec<BlockIdExt>,
}
impl Display for ShardNeighbours {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}[{}]",
            self.shard,
            self.neighbours.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(", ")
        )
    }
}
impl ShardNeighbours {
    fn new(shard: &ShardIdent) -> Self {
        Self { shard: shard.clone(), neighbours: Vec::new() }
    }
    fn add(&mut self, neighbour: &BlockIdExt) {
        self.neighbours.push(neighbour.clone());
    }
    fn len(&self) -> usize {
        self.neighbours.len()
    }
}

pub struct OutMsgQueueManager {
    engine: Arc<dyn EngineOperations>,
    queue_awaiters: AwaitersPool<ShardNeighbours, HashMap<BlockIdExt, OutMsgQueueInfo>>,
    request_id: AtomicU64,
    max_bytes_limit: AtomicU32,
    max_messages_limit: AtomicU32,
    cache_resolver: Arc<AllowStateGcSmartResolver>,

    // Only queues that were obtained by broadcast are stored here because requested queues may be
    // not long enough (they are constructed as part of the neighbours' set).
    queues_cache: lockfree::map::Map<(BlockIdExt, ShardIdent), OutMsgQueueInfo>,

    // One request for one group of shards (ancestors of monitor_min_split shards)
    requests_cache: lockfree::map::Map<ShardNeighbours, HashMap<BlockIdExt, OutMsgQueueInfo>>,
}

impl OutMsgQueueManager {
    pub async fn new(engine: Arc<dyn EngineOperations>) -> Result<Arc<Self>> {
        let manager = Arc::new(Self {
            queues_cache: lockfree::map::Map::new(),
            requests_cache: lockfree::map::Map::new(),
            queue_awaiters: AwaitersPool::new(
                "OutMsgQueueManager",
                #[cfg(feature = "telemetry")]
                engine.engine_telemetry().clone(),
                engine.engine_allocated().clone(),
            ),
            engine,
            request_id: AtomicU64::new(192837465), // This is informational id, used only for logging
            max_bytes_limit: AtomicU32::new(0),
            max_messages_limit: AtomicU32::new(0),
            cache_resolver: Arc::new(AllowStateGcSmartResolver::new(0)),
        });
        let last_mc_state = manager.engine.load_last_applied_mc_state().await?;
        manager.apply_config(last_mc_state.config_params()?)?;
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            loop {
                if let Err(e) = manager_clone.clean_cache_worker().await {
                    log::error!("OutMsgQueueManager unexpected error in clean_cache_worker: {}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                } else {
                    break;
                }
            }
        });
        Ok(manager)
    }

    #[allow(dead_code)]
    pub async fn request(
        &self,
        dst_shard: &ShardIdent,
        neighbours: &[BlockIdExt],
        timeout_ms: Option<u64>,
    ) -> Result<HashMap<BlockIdExt, OutMsgQueueInfo>> {
        let mut result = HashMap::new();
        let monitor_min_split = self.engine.get_monitor_min_split();
        let rq_id = self.request_id.fetch_add(1, Ordering::Relaxed);
        let started_at = Instant::now();
        let mut attempt = 0;

        log::debug!(
            "Request #{}, dst shard: {}, neighbours count: {}",
            rq_id,
            dst_shard,
            neighbours.len()
        );

        while result.len() < neighbours.len() {
            if let Some(timeout) = timeout_ms {
                if started_at.elapsed().as_millis() > timeout as u128 {
                    log::warn!("{}: Timeout", rq_id);
                    fail!("Timeout while downloading neighbours' queues");
                }
            }
            attempt += 1;

            let mut requests = HashMap::new();

            for neighbour in neighbours {
                if result.contains_key(neighbour) {
                    continue;
                }

                if self.engine.need_monitor(neighbour.shard())? {
                    // 1) check for local shards (shards we sync in)
                    let queue = self.load_locally(neighbour, timeout_ms).await?;
                    result.insert(neighbour.clone(), queue);
                    log::trace!("{}: {} loaded from local DB", rq_id, neighbour);
                } else if let Some(kv) =
                    self.queues_cache.get(&(neighbour.clone(), dst_shard.clone()))
                {
                    // 2) check queues_cache
                    result.insert(neighbour.clone(), kv.val().clone());
                    log::trace!("{}: {} loaded from queues cache", rq_id, neighbour);
                } else {
                    // 3) devede neighbours into groups of shards by monitor_min_split
                    requests
                        .entry(neighbour.shard().relative_with_len(monitor_min_split)?)
                        .or_insert_with(|| ShardNeighbours::new(dst_shard))
                        .add(neighbour);
                }
            }

            let mut tasks = Vec::new();
            for (shard, neighbours) in requests.iter_mut() {
                neighbours.neighbours.sort();

                // No more than 16 neighbours per request
                let mut neighbours_chunk = ShardNeighbours::new(shard);

                for i in 0..neighbours.len() {
                    neighbours_chunk.add(&neighbours.neighbours[i]);

                    if neighbours_chunk.len() == 16 || i == neighbours.neighbours.len() - 1 {
                        if let Some(cached) = self.requests_cache.get(&neighbours_chunk) {
                            // 4) check requests_cache
                            for (id, queue) in cached.val() {
                                result.insert(id.clone(), queue.clone());
                                log::trace!(
                                    "{}: Neighbours from {} loaded from requests cache",
                                    rq_id,
                                    shard
                                );
                            }
                        } else {
                            // 5) prepare network request (in parallel)
                            tasks.push(self.queue_awaiters.do_or_wait_with_owned_key(
                                neighbours_chunk.clone(),
                                timeout_ms,
                                self.download(neighbours_chunk),
                            ));
                            log::trace!("{}: Neighbours from {} downloading...", rq_id, shard);
                        }
                        neighbours_chunk = ShardNeighbours::new(shard);
                    }
                }
            }

            if !tasks.is_empty() {
                // 6) wait for all requests to finish
                let responses = futures::future::join_all(tasks).await;

                // 7) parse responses
                for response in responses {
                    match response {
                        Ok(Some(queues)) => {
                            for (id, queue) in queues {
                                log::trace!("{}: {} downloaded", rq_id, id);
                                result.insert(id.clone(), queue.clone());
                                self.queues_cache.insert((id, dst_shard.clone()), queue);
                            }
                        }
                        Ok(None) => continue, // No data received
                        Err(e) => {
                            if attempt > 3 {
                                log::warn!("{}: Error while downloading: {}", rq_id, e);
                            } else {
                                log::debug!("{}: Error while downloading: {}", rq_id, e);
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue; // Retry
                        }
                    }
                }
            }
        }

        log::debug!("{} done for {:#?}", rq_id, started_at.elapsed());

        Ok(result)
    }

    fn apply_config(&self, config: &ConfigParams) -> Result<()> {
        if let Some(queue_limits) = config.block_limits(false)?.imported_msg_queue() {
            self.max_bytes_limit.store(queue_limits.max_bytes, Ordering::Relaxed);
            self.max_messages_limit.store(queue_limits.max_msgs, Ordering::Relaxed);
        } else {
            let default_limits = ImportedMsgQueueLimits::default();
            self.max_bytes_limit.store(default_limits.max_bytes, Ordering::Relaxed);
            self.max_messages_limit.store(default_limits.max_msgs, Ordering::Relaxed);
            log::warn!(
                "ImportedMsgQueueLimits not found in config, using default: {:?}",
                default_limits
            );
        }
        Ok(())
    }

    async fn clean_cache_worker(&self) -> Result<()> {
        let id = self
            .engine
            .load_last_applied_mc_block_id()?
            .ok_or_else(|| error!("Cannot load last applied mc block id"))?;
        let mut handle = self.engine.load_block_handle(&id)?.ok_or_else(|| {
            error!("Cannot load handle for msg queues cache cleaner block {}", id)
        })?;
        loop {
            if self.engine.check_stop() {
                return Ok(());
            }

            if handle.is_key_block()? {
                let state = self.engine.load_state(handle.id()).await?;
                let config = state.config_params()?;
                self.apply_config(config)?;
            }

            // update gc resolver
            let advanced = self.cache_resolver.advance(&handle.id(), self.engine.deref()).await?;
            if advanced {
                // clear cache
                let mut queues_total = 0;
                let mut queues_cleaned = 0;
                let mut requests_total = 0;
                let mut requests_cleaned = 0;
                let now = Instant::now();
                for guard in &self.queues_cache {
                    queues_total += 1;
                    if self.cache_resolver.allow_state_gc(&guard.0 .0, 0, 0)? {
                        self.queues_cache.remove(&guard.0);
                        queues_cleaned += 1;
                    }
                }
                for guard in &self.requests_cache {
                    requests_total += 1;
                    for neighbour in guard.0.neighbours.iter() {
                        if self.cache_resolver.allow_state_gc(neighbour, 0, 0)? {
                            self.requests_cache.remove(&guard.0);
                            requests_cleaned += 1;
                            break;
                        }
                    }
                }
                log::debug!(
                    "clean_cache_worker: TIME: {time:#?}, cleared queues: {queues_cleaned}/{queues_total}, \
                    requests: {requests_cleaned}/{requests_total}",
                    time = now.elapsed(),
                );
            }

            // wait next mc block
            handle = loop {
                if let Ok(h) = self.engine.wait_next_applied_mc_block(&handle, Some(500)).await {
                    break h.0;
                } else if self.engine.check_stop() {
                    return Ok(());
                }
            };
        }
    }

    /* TODO
        pub fn process_broadcast(
            &self,
            broadcast: OutMsgQueueProofBroadcast
        ) -> Result<()> {


            // OutMsgQueueProofBroadcast contains OutMsgQueueProof with only one queue proof

            self.check(
                block_state_proofs: Vec<u8>,
                queue_proofs: Vec<u8>,
                msg_counts: Vec<i32>, // signed int to match ton_api.tl
                neighbours: &ShardNeighbours,
                limits: &ImportedMsgQueueLimits,
            )

            self.queues_cache.insert((neighbour, dst_shard), proof);

            Ok(())
        }
    */

    async fn load_locally(
        &self,
        id: &BlockIdExt,
        timeout_ms: Option<u64>,
    ) -> Result<OutMsgQueueInfo> {
        let ss = self.engine.clone().wait_state(id, timeout_ms, false).await?;
        ss.state()?.read_out_msg_queue_info()
    }

    // download and check neighbours' queues proofs from the network
    async fn download(
        &self,
        neighbours: ShardNeighbours,
    ) -> Result<HashMap<BlockIdExt, OutMsgQueueInfo>> {
        let limits = ImportedMsgQueueLimits::new(
            self.max_bytes_limit.load(Ordering::Relaxed) * neighbours.len() as u32,
            self.max_messages_limit.load(Ordering::Relaxed) * neighbours.len() as u32,
        );

        let respond = self
            .engine
            .download_out_msg_queue_proof(&neighbours.shard, &neighbours.neighbours, &limits)
            .await?;
        let omqp = if let OutMsgQueueProof::TonNode_OutMsgQueueProof(omqp) = respond {
            omqp
        } else {
            fail!("OutMsgQueueProofEmpty returned");
        };
        if omqp.msg_counts.len() != neighbours.neighbours.len() {
            fail!(
                "Responded msg_counts {} does not match requested queues count {}",
                omqp.msg_counts.len(),
                neighbours.neighbours.len()
            );
        }

        let queues = self.check(
            omqp.block_state_proofs,
            omqp.queue_proofs,
            omqp.msg_counts,
            &neighbours,
            &limits,
        )?;

        self.requests_cache.insert(neighbours, queues.clone());

        Ok(queues)
    }

    fn check(
        &self,
        block_state_proofs: Vec<u8>,
        queue_proofs: Vec<u8>,
        msg_counts: Vec<i32>, // signed int to match ton_api.tl
        neighbours: &ShardNeighbours,
        limits: &ImportedMsgQueueLimits,
    ) -> Result<HashMap<BlockIdExt, OutMsgQueueInfo>> {
        // 1) deserialize bocs
        let state_proof_roots = read_boc(&block_state_proofs)?.roots;
        let queue_proof_roots = read_boc(&queue_proofs)?.roots;
        if queue_proof_roots.len() != neighbours.neighbours.len() {
            fail!(
                "Queue proof roots count {} does not match requested queues count {}",
                queue_proof_roots.len(),
                neighbours.neighbours.len()
            );
        }

        let mut state_roots = Vec::with_capacity(neighbours.neighbours.len());
        let mut i_state_proof = 0;
        for i in 0..neighbours.neighbours.len() {
            let queue_proof = MerkleProof::construct_from_cell(queue_proof_roots[i].clone())?;
            let nbr_id = &neighbours.neighbours[i];

            if nbr_id.seq_no() == 0 {
                // 2') it is neighbour's zerostate (strange case, but ok)
                if queue_proof.hash != *neighbours.neighbours[i].root_hash() {
                    fail!(
                        "Queue proof hash {} does not match requested {}",
                        queue_proof.hash,
                        nbr_id
                    );
                }
            } else {
                let cell = state_proof_roots
                    .get(i_state_proof)
                    .ok_or_else(|| error!("State proof for neighbour {} was not found", nbr_id))?;
                let state_proof = MerkleProof::construct_from_cell(cell.clone())?;
                i_state_proof += 1;

                // 2) check if block proof correspond requested neighbour
                if state_proof.hash != *nbr_id.root_hash() {
                    fail!(
                        "State proof repr hash {} does not match requested {}",
                        state_proof.hash,
                        nbr_id
                    );
                }

                // 3) check queue proof root hash
                let block: Block = state_proof.virtualize()?;
                let state_hash = block.read_state_update()?.new_hash;

                if queue_proof.hash != state_hash {
                    fail!(
                        "Queue proof hash {} does not match state's hash from block {}",
                        queue_proof.hash,
                        state_hash
                    );
                }
            }

            state_roots.push(queue_proof.proof.virtualize(1));
        }
        if state_proof_roots.len() != i_state_proof {
            fail!(
                "State proof roots count {} does not match calculated {}",
                state_proof_roots.len(),
                i_state_proof
            );
        }

        // 4) check queues by rebuilding it from itself
        let (queue_re_proof, calculated_msg_counts) =
            build_proofs(&neighbours.shard, &neighbours.neighbours, &state_roots, &limits)?;
        for i in 0..calculated_msg_counts.len() {
            if calculated_msg_counts[i] != msg_counts[i] as u32 {
                fail!(
                    "Calculated messages count {} for {} does not match requested {}",
                    calculated_msg_counts[i],
                    neighbours.neighbours[i],
                    msg_counts[i]
                );
            }
        }

        if queue_re_proof != queue_proof_roots {
            fail!("Rebuilt queue proof roots do not match downloaded ones");
        }

        // 5) build queues
        let mut queues = HashMap::new();
        for i in 0..neighbours.neighbours.len() {
            let state = ShardStateUnsplit::construct_from_cell(state_roots[i].clone())?;
            queues.insert(neighbours.neighbours[i].clone(), state.read_out_msg_queue_info()?);
        }

        Ok(queues)
    }
}
