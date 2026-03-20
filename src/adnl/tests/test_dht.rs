/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::{
    common::AdnlPeers, node::AdnlNode, DhtNode, DhtSearchPolicy, OverlayNode, OverlayNodeInfo,
    OverlayNodesSearchContext, OverlayParams, OverlayShortId,
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    thread::sleep,
    time::{Duration, Instant},
};
use ton_block::{base64_encode, fail, KeyOption, Result};

#[path = "./test_utils.rs"]
mod test_utils;
use test_utils::{get_adnl_config, init_compatibility_test, init_test, TestContext};

const CONFIG_MAINNET_FILE: &str = "tests/config/mainnet.json";
const CONFIG_TESTNET_FILE: &str = "tests/config/testnet.json";
const KEY_TAG: usize = 0;

const ZEROSTATE_MAINNET: &str = "0nC4eylStbp9qnCq8KjDYb789NjS25L5ZA1UQwcIOOQ=";
const ZEROSTATE_TESTNET: &str = "XplPz01CXAps5qeSWUtxcyBfdAo5zVb1N979KLSKD24=";

const TARGET: &str = "dht";

fn init_dht_compatibility_test(
    local_ip_template: &str,
    zero_state_file_hash: &str,
    config_file: &str,
) -> TestContext {
    init_compatibility_test(
        local_ip_template,
        4190,
        "dht",
        KEY_TAG,
        KEY_TAG,
        zero_state_file_hash,
        config_file,
        false,
        false,
        #[cfg(feature = "dump")]
        None,
    )
}

fn init_mainnet_test(local_ip_template: &str) -> TestContext {
    init_dht_compatibility_test(local_ip_template, ZEROSTATE_MAINNET, CONFIG_MAINNET_FILE)
}

fn init_testnet_test(local_ip_template: &str) -> TestContext {
    init_dht_compatibility_test(local_ip_template, ZEROSTATE_TESTNET, CONFIG_TESTNET_FILE)
}

fn init_local_test() -> (
    tokio::runtime::Runtime,
    Arc<AdnlNode>,
    Arc<DhtNode>,
    Arc<OverlayNode>,
    Arc<AdnlNode>,
    Arc<DhtNode>,
    Arc<OverlayNode>,
) {
    let rt = init_test();
    let config1 =
        rt.block_on(get_adnl_config("dht", "127.0.0.1:4191", vec![KEY_TAG], true)).unwrap();
    let config2 =
        rt.block_on(get_adnl_config("dht", "127.0.0.1:4192", vec![KEY_TAG], true)).unwrap();
    let adnl1 = rt.block_on(AdnlNode::with_config(config1)).unwrap();
    let dht1 = DhtNode::with_adnl_node(adnl1.clone(), KEY_TAG).unwrap();
    let overlay1 = OverlayNode::with_params(adnl1.clone(), &[1u8; 32], KEY_TAG).unwrap();
    rt.block_on(adnl1.start_over_udp(vec![dht1.clone(), overlay1.clone()])).unwrap();
    let adnl2 = rt.block_on(AdnlNode::with_config(config2)).unwrap();
    let dht2 = DhtNode::with_adnl_node(adnl2.clone(), KEY_TAG).unwrap();
    let overlay2 = OverlayNode::with_params(adnl2.clone(), &[1u8; 32], KEY_TAG).unwrap();
    rt.block_on(adnl2.start_over_udp(vec![dht2.clone(), overlay2.clone()])).unwrap();
    (rt, adnl1, dht1, overlay1, adnl2, dht2, overlay2)
}

async fn run_test<F: Future<Output = Result<bool>>>(f: impl Fn() -> F) -> bool {
    for _ in 0..4 {
        if let Ok(true) = f().await {
            return true;
        }
    }
    f().await.unwrap()
}

#[test]
fn dht_compatibility() {
    let ctx = init_testnet_test("0.0.0.0:1");
    ctx.rt.block_on(async move {
        let key = ctx.dht.key();
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.find_dht_nodes(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.store_ip_address(&key)).await);
        assert!(run_test(|| ctx.dht.get_signed_address_list(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.store_ip_address(&key)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.get_signed_address_list(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.get_signed_address_list(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        ctx.adnl.stop().await;
    })
}

#[test]
fn dht_multitask() {
    const THREADS: usize = 200;
    let ctx = init_testnet_test("0.0.0.0:1");
    let ping = Arc::new(tokio::sync::Barrier::new(THREADS + 1));
    let pass = Arc::new(AtomicU32::new(0));
    for i in 0..THREADS {
        let dht = ctx.dht.clone();
        let peer = ctx.peer.clone();
        let ping = ping.clone();
        let pass = pass.clone();
        ctx.rt.spawn(async move {
            for _ in 0..20 {
                if run_test(|| dht.ping(&peer)).await {
                    pass.fetch_add(1, Ordering::Relaxed);
                    break;
                }
            }
            ping.wait().await
        });
        // Ensure channel creation
        let delay = if i < 3 { 300 } else { 30 };
        sleep(Duration::from_millis(delay));
    }
    ctx.rt.block_on(ping.wait());
    assert_eq!(pass.load(Ordering::Relaxed), THREADS as u32);
    ctx.rt.block_on(ctx.adnl.stop());
}

async fn test_find_nodes(dht: &Arc<DhtNode>, overlay_id: &Arc<OverlayShortId>) -> Result<bool> {
    let mut pool = Vec::new();
    let mut ctx =
        OverlayNodesSearchContext::with_params(overlay_id, DhtSearchPolicy::FastSearch(5))?;
    let started = Instant::now();
    loop {
        if started.elapsed().as_secs() > 60 {
            break;
        }
        let mut nodes = dht.find_overlay_nodes(&mut ctx).await?;
        if !nodes.is_empty() {
            pool.append(&mut nodes);
        }
        log::debug!(target: TARGET, "Overlay nodes search: {} nodes found", pool.len());
        if !nodes.is_empty() {
            break;
        }
    }
    if !pool.is_empty() {
        log::debug!(target: TARGET, "---- Found overlay nodes:");
        for (ip, node) in pool {
            let key: Arc<dyn KeyOption> = node.id().try_into()?;
            log::debug!(
                target: TARGET,
                "\n{} key ID {}, key {}, version {}, signature {}",
                ip,
                key.id(),
                base64_encode(key.pub_key()?),
                node.version(),
                base64_encode(node.signature())
            );
        }
        Ok(true)
    } else {
        fail!("Cannot find overlay nodes")
    }
}

fn find_overlay_nodes(ctx: &TestContext) {
    ctx.rt.block_on(async move {
        assert!(run_test(|| test_find_nodes(&ctx.dht, &ctx.overlay_id)).await);
        ctx.adnl.stop().await;
    })
}

#[test]
fn find_overlay_nodes_testnet() {
    let ctx = init_testnet_test("0.0.0.0:1");
    find_overlay_nodes(&ctx)
}

#[test]
fn find_overlay_nodes_mainnet() {
    let ctx = init_mainnet_test("0.0.0.0:1");
    find_overlay_nodes(&ctx)
}

#[test]
fn dht_response_mainnet() {
    let ctx = init_mainnet_test("0.0.0.0:1");
    ctx.rt.block_on(async move {
        assert!(run_test(|| ctx.dht.get_signed_address_list(&ctx.peer)).await);
        assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        ctx.adnl.stop().await;
    })
}

#[test]
fn adnl_reset_channel() {
    let ctx = init_testnet_test("0.0.0.0:1");
    ctx.rt.block_on(async move {
        for _ in 0..10 {
            assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        }
        let peers = AdnlPeers::with_keys(ctx.dht.key().id().clone(), ctx.peer.clone());
        ctx.adnl.reset_peers(&peers).unwrap();
        for _ in 0..10 {
            assert!(run_test(|| ctx.dht.ping(&ctx.peer)).await);
        }
        ctx.adnl.stop().await;
    })
}

#[test]
fn dht_session() {
    let (rt, adnl1, dht1, overlay1, adnl2, dht2, overlay2) = init_local_test();
    rt.block_on(async move {
        let peer1 = dht2.add_peer(&dht1.get_signed_node().unwrap()).unwrap().unwrap();
        let peer2 = dht1.add_peer(&dht2.get_signed_node().unwrap()).unwrap().unwrap();
        assert!(dht1.ping(&peer2).await.unwrap());
        assert!(dht2.ping(&peer1).await.unwrap());
        assert!(dht1.find_dht_nodes(&peer2).await.unwrap());
        assert!(dht2.find_dht_nodes(&peer1).await.unwrap());
        assert!(dht1.store_ip_address(&dht1.key()).await.unwrap());
        assert!(dht2.store_ip_address(&dht2.key()).await.unwrap());
        let overlay_id = overlay1.calc_overlay_id(-1, 0x8000000000000000u64 as i64).unwrap();
        let overlay_short_id =
            overlay1.calc_overlay_short_id(-1, 0x8000000000000000u64 as i64).unwrap();
        let params = OverlayParams::with_id_only(&overlay_short_id);
        overlay1.add_local_workchain_overlay(params).unwrap();
        let params = OverlayParams::with_id_only(&overlay_short_id);
        overlay2.add_local_workchain_overlay(params).unwrap();
        let OverlayNodeInfo::V1(node1) =
            overlay1.get_signed_node(&overlay_short_id, false).unwrap()
        else {
            panic!("Unexpected V2 overlay node info")
        };
        let OverlayNodeInfo::V1(node2) =
            overlay2.get_signed_node(&overlay_short_id, false).unwrap()
        else {
            panic!("Unexpected V2 overlay node info")
        };
        assert!(dht1.store_overlay_node(&overlay_id, &node2).await.unwrap());
        assert!(dht2.store_overlay_node(&overlay_id, &node1).await.unwrap());
        adnl1.stop().await;
        adnl2.stop().await;
    })
}

#[test]
fn dht_store_testnet() {
    let ctx = init_testnet_test("0.0.0.0:1");
    ctx.rt.block_on(async move {
        let overlay_long_id =
            ctx.overlay.calc_overlay_id(-1, 0x8000000000000000u64 as i64).unwrap();
        let params = OverlayParams::with_id_only(&ctx.overlay_id);
        ctx.overlay.add_local_workchain_overlay(params).unwrap();
        let OverlayNodeInfo::V1(node) =
            ctx.overlay.get_signed_node(&ctx.overlay_id, false).unwrap()
        else {
            panic!("Unexpected V2 overlay node info")
        };
        let key = ctx.dht.key();
        assert!(run_test(|| ctx.dht.store_ip_address(&key)).await);
        assert!(run_test(|| ctx.dht.store_overlay_node(&overlay_long_id, &node)).await);
        ctx.adnl.stop().await;
    })
}
