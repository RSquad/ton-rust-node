/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::common::{AdnlPeers, QueryAnswer, QueryResult, Subscriber};
use node::{
    engine_traits::EngineOperations, network::liteserver::LiteServerQuerySubscriber,
    shard_state::ShardStateStuff,
};
use std::sync::Arc;
use ton_api::{
    serialize_boxed,
    ton::rpc::lite_server::{GetMasterchainInfo, GetState, GetTime, Query},
    AnyBoxedSerialize,
};
use ton_block::{BlockIdExt, KeyId, Result, Serializable, ShardIdent, ShardStateUnsplit, UInt256};

// ---------------------------------------------------------------------------
// Minimal mock engine
// ---------------------------------------------------------------------------

struct BenchEngine {
    mc_block_id: BlockIdExt,
    mc_state: Arc<ShardStateStuff>,
    zerostate_id: BlockIdExt,
}

impl BenchEngine {
    fn new() -> Self {
        let ss = ShardStateUnsplit::with_ident(ShardIdent::masterchain());
        let cell = ss.serialize().expect("serialize shard state");
        let bytes = ton_block::write_boc(&cell).expect("write boc");
        let root_hash = cell.repr_hash();
        let file_hash = UInt256::calc_file_hash(&bytes);

        let mc_block_id =
            BlockIdExt { shard_id: ShardIdent::masterchain(), seq_no: 0, root_hash, file_hash };

        let mc_state = ShardStateStuff::deserialize_zerostate(
            mc_block_id.clone(),
            &bytes,
            #[cfg(feature = "telemetry")]
            &node::collator_test_bundle::create_engine_telemetry(),
            &node::collator_test_bundle::create_engine_allocated(),
        )
        .expect("deserialize state");

        let zerostate_id = mc_block_id.clone();

        Self { mc_block_id, mc_state, zerostate_id }
    }
}

#[async_trait::async_trait]
impl EngineOperations for BenchEngine {
    fn now(&self) -> u32 {
        1_700_000_000
    }

    fn zerostate_id(&self) -> Result<&BlockIdExt> {
        Ok(&self.zerostate_id)
    }

    fn load_last_applied_mc_block_id(&self) -> Result<Option<Arc<BlockIdExt>>> {
        Ok(Some(Arc::new(self.mc_block_id.clone())))
    }

    async fn load_state(&self, id: &BlockIdExt) -> Result<Arc<ShardStateStuff>> {
        if *id == self.mc_block_id {
            Ok(self.mc_state.clone())
        } else {
            Err(anyhow::anyhow!("state not found for {id}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dummy_peers() -> AdnlPeers {
    AdnlPeers::with_keys(KeyId::from_data([0u8; 32]), KeyId::from_data([0u8; 32]))
}

async fn dispatch_and_wait(subscriber: &LiteServerQuerySubscriber, data: Vec<u8>) -> Result<()> {
    let query = Query { data: data.into() };
    let reply = subscriber.try_consume_query(query.into_tl_object(), &dummy_peers()).await?;
    match reply {
        QueryResult::Consumed(QueryAnswer::Ready(_)) => Ok(()),
        QueryResult::Consumed(QueryAnswer::Pending(handle)) => {
            handle.await??;
            Ok(())
        }
        _ => Err(anyhow::anyhow!("query rejected")),
    }
}

// GetTime — immediate path, lightest possible query
fn build_get_time_query() -> Vec<u8> {
    serialize_boxed(&GetTime.into_tl_object()).unwrap()
}

// GetMasterchainInfo — immediate path, reads state
fn build_get_mc_info_query() -> Vec<u8> {
    serialize_boxed(&GetMasterchainInfo.into_tl_object()).unwrap()
}

// GetState — heavy (pipeline) path, serializes entire state to BOC
fn build_get_state_query(block_id: &BlockIdExt) -> Vec<u8> {
    serialize_boxed(&GetState { id: block_id.clone() }.into_tl_object()).unwrap()
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Immediate dispatch: queries processed inline, 0 spawns, 0 channels
fn bench_immediate_dispatch(c: &mut criterion::Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter(); // required for old create_queue which calls tokio::spawn in new()
    let engine: Arc<dyn EngineOperations> = Arc::new(BenchEngine::new());
    let subscriber =
        LiteServerQuerySubscriber::new(rt.handle().clone(), engine, 256, 16, 50).unwrap();

    let mut group = c.benchmark_group("immediate_dispatch");

    let data = build_get_time_query();
    group.bench_function("GetTime", |b| {
        b.to_async(&rt).iter(|| {
            let d = data.clone();
            async { dispatch_and_wait(&subscriber, d).await.unwrap() }
        });
    });

    let data = build_get_mc_info_query();
    group.bench_function("GetMasterchainInfo", |b| {
        b.to_async(&rt).iter(|| {
            let d = data.clone();
            async { dispatch_and_wait(&subscriber, d).await.unwrap() }
        });
    });

    group.finish();
}

/// Pipeline dispatch: heavy query goes through spawn + semaphore
fn bench_pipeline_dispatch(c: &mut criterion::Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let engine = Arc::new(BenchEngine::new());
    let block_id = engine.mc_block_id.clone();
    let engine: Arc<dyn EngineOperations> = engine;
    let subscriber =
        LiteServerQuerySubscriber::new(rt.handle().clone(), engine, 256, 16, 50).unwrap();

    let mut group = c.benchmark_group("pipeline_dispatch");

    let data = build_get_state_query(&block_id);
    group.bench_function("GetState", |b| {
        b.to_async(&rt).iter(|| {
            let d = data.clone();
            async { dispatch_and_wait(&subscriber, d).await.unwrap() }
        });
    });

    group.finish();
}

/// Concurrent pipeline: N heavy queries dispatched simultaneously
fn bench_pipeline_concurrent(c: &mut criterion::Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let engine = Arc::new(BenchEngine::new());
    let block_id = engine.mc_block_id.clone();
    let engine: Arc<dyn EngineOperations> = engine;
    let subscriber =
        Arc::new(LiteServerQuerySubscriber::new(rt.handle().clone(), engine, 256, 16, 50).unwrap());

    let mut group = c.benchmark_group("pipeline_concurrent");

    for concurrency in [10, 100, 500, 10_000] {
        let data = build_get_state_query(&block_id);
        group.bench_with_input(
            criterion::BenchmarkId::from_parameter(concurrency),
            &concurrency,
            |b, &n| {
                b.to_async(&rt).iter(|| {
                    let sub = subscriber.clone();
                    let d = data.clone();
                    async move {
                        let mut handles = Vec::with_capacity(n);
                        for _ in 0..n {
                            let sub = sub.clone();
                            let d = d.clone();
                            handles.push(tokio::spawn(async move {
                                dispatch_and_wait(&sub, d).await.unwrap();
                            }));
                        }
                        for h in handles {
                            h.await.unwrap();
                        }
                    }
                });
            },
        );
    }

    group.finish();
}

criterion::criterion_group!(
    benches,
    bench_immediate_dispatch,
    bench_pipeline_dispatch,
    bench_pipeline_concurrent,
);
criterion::criterion_main!(benches);
