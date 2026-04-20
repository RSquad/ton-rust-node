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
#![allow(clippy::type_complexity)]

use adnl::{
    common::{
        AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice, TaggedTlObject,
    },
    node::{AdnlNode, AdnlNodeConfig, AdnlSendMethod, IpAddress},
    telemetry::{Metric, MetricBuilder, TelemetryItem, TelemetryPrinter},
    AddressSearchContext, DhtSearchPolicy, OverlayNode, OverlayNodeInfo, OverlayNodesSearchContext,
    OverlayParams, OverlayShortId, OverlayUtils, RaptorqDecoder, RaptorqEncoder, RldpNode,
};
use rand::Rng;
#[cfg(feature = "dump")]
use std::path::PathBuf;
use std::{
    cmp::min,
    convert::TryInto,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    thread::{self, sleep, JoinHandle},
    time::{Duration, Instant},
};
use ton_api::{
    deserialize_boxed, deserialize_boxed_with_suffix, serialize_boxed,
    ton::{
        overlay::{
            broadcast::Broadcast as BroadcastOrd, membercertificate::MemberCertificate,
            node::Node as NodeV1, nodev2::NodeV2, Certificate as OverlayCertificate,
            Node as NodeV1Boxed,
        },
        rpc::{overlay::Ping as OverlayPing, ton_node::GetCapabilities},
        ton_node::capabilities::Capabilities,
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};
use ton_block::{
    base64_decode, base64_encode, error, fail, sha256_digest, Ed25519KeyOption, KeyId, KeyOption,
    Result, UInt256, UnixTime,
};

#[path = "./test_utils.rs"]
mod test_utils;
use test_utils::{
    find_overlay_peer, get_adnl_config, init_compatibility_test, init_test, init_test_log,
    TestContext,
};

const KEY_TAG_DHT: usize = 1;
const KEY_TAG_OVERLAY: usize = 2;
const ZERO_STATE: &str = "XplPz01CXAps5qeSWUtxcyBfdAo5zVb1N979KLSKD24=";

const CONFIG_TESTNET_FILE: &str = "tests/config/testnet.json";
const TARGET: &str = "overlay";

/*
pub fn build_dht_node_info_ex(
    ip: &str,
    key: &str,
    signature: &str,
    addr_version: i32,
    node_version: i32
) -> Result<Node> {
    let key = base64_decode(key)?;
    if key.len() != 32 {
        fail!("Bad public key length")
    }
    let addrs = vec![IpAddress::from_versioned_string(ip, None)?.into_udp().into_boxed()];
    let signature = base64_decode(signature)?;
    let node = Node {
        id: Ed25519 {
            key: UInt256::with_array(key.try_into().unwrap())
        }.into_boxed(),
        addr_list: AddressList {
            addrs: addrs.into(),
            version: addr_version,
            reinit_date: addr_version,
            priority: 0,
            expire_at: 0
        },
        version: node_version,
        signature
    };
    Ok(node)
}
*/

/// Base port for test_broadcast nodes (range 4210..4219, does not overlap with other tests).
const BROADCAST_TEST_BASE_PORT: u16 = 4210;

fn init_overlay_simple_compatibility_test(
    local_ip_template: &str,
    #[cfg(feature = "dump")] dump_path: Option<&str>,
) -> TestContext {
    init_compatibility_test(
        local_ip_template,
        4190,
        "overlay",
        KEY_TAG_DHT,
        KEY_TAG_OVERLAY,
        ZERO_STATE,
        CONFIG_TESTNET_FILE,
        true,
        true,
        #[cfg(feature = "dump")]
        dump_path,
    )
}

fn init_overlay_compatibility_test(
    local_ip_template: &str,
    min_peers: Option<usize>,
    #[cfg(feature = "dump")] dump_path: Option<&str>,
) -> (TestContext, Vec<(IpAddress, NodeV1)>, OverlayNodesSearchContext) {
    let mut ctx_test = init_overlay_simple_compatibility_test(
        local_ip_template,
        #[cfg(feature = "dump")]
        dump_path,
    );
    let mut found_peers = Vec::new();
    let mut peers_cache = Vec::new();
    let mut ctx_search = OverlayNodesSearchContext::with_params(
        &ctx_test.overlay_id,
        DhtSearchPolicy::FastSearch(5),
    )
    .unwrap();
    loop {
        let (ip, node) =
            find_overlay_peer(&mut peers_cache, &mut ctx_search, &mut ctx_test, TARGET);
        found_peers.push((ip, node));
        let Some(min) = min_peers else {
            break;
        };
        if found_peers.len() >= min {
            break;
        }
    }
    for (ip, node) in found_peers.iter() {
        log::info!(target: TARGET, "Found overlay peer {ip} {node}");
    }
    log::info!(
        target: TARGET,
        "Use local key {}",
        ctx_test.adnl.key_by_tag(KEY_TAG_OVERLAY).unwrap().id()
    );
    (ctx_test, found_peers, ctx_search)
}

#[test]
fn test_overlay_id() {
    let workchain = -1i32;
    let shard = 0x8000000000000000u64 as i64;
    let file_hash = base64_decode("aAMjutqwMSgcejzVa/OWEHRiECI2i5yxn9FM/Thpa2Q=").unwrap();
    let file_hash = file_hash.as_slice().try_into().unwrap();
    assert_eq!(
        hex::encode(&OverlayUtils::calc_overlay_id(workchain, shard, file_hash).unwrap()),
        "2441f387e2f355d4fd82ffc63d94d98e1d078d8855270a3d3c10c09e0701976f"
    );
    assert_eq!(
        hex::encode(
            OverlayUtils::calc_overlay_short_id(workchain, shard, file_hash).unwrap().data()
        ),
        "dc7c6d60991db081780e7e12627d8c315dc171db982452e91f1f30d738cef966"
    );
}

/// Helper: create a [u8; 32] filled with a repeating byte
fn make_hash(fill: u8) -> [u8; 32] {
    [fill; 32]
}

/// Test vectors for tonNode.customOverlayId
#[test]
fn test_custom_overlay_id() {
    // Test 1: zero hash, name="test_overlay", no nodes
    let short_id =
        OverlayUtils::calc_custom_overlay_short_id(&make_hash(0x00), "test_overlay", &[]).unwrap();
    assert_eq!(
        hex::encode(short_id.data()).to_uppercase(),
        "C1025E62A9F844FC4EECBEC12F0AD960F815FAD4C71803DC8192147AC206ACE3"
    );

    // Test 2: 0xBB hash, name="my_custom_overlay", 2 nodes [0x44, 0x55]
    let short_id = OverlayUtils::calc_custom_overlay_short_id(
        &make_hash(0xBB),
        "my_custom_overlay",
        &[make_hash(0x44), make_hash(0x55)],
    )
    .unwrap();
    assert_eq!(
        hex::encode(short_id.data()).to_uppercase(),
        "71F4DE370407E0A639B302875000D46E3DA36D5F1865D3FE4D4F7E59FA35CB2D"
    );

    // Test 3: 0xCC hash, name="", 1 node [0x99]
    let short_id =
        OverlayUtils::calc_custom_overlay_short_id(&make_hash(0xCC), "", &[make_hash(0x99)])
            .unwrap();
    assert_eq!(
        hex::encode(short_id.data()).to_uppercase(),
        "E7DD8EA0B9328C37E0AB480B6CC2BE0F41AE212E0A940CFB9CFDBB22C48A99BC"
    );
}

/// Test vectors for tonNode.fastSyncOverlayId
#[test]
fn test_fast_sync_overlay_id() {
    // Test 1: zero hash, masterchain, full shard
    let short_id = OverlayUtils::calc_fast_sync_overlay_short_id(
        &make_hash(0x00),
        -1,
        0x8000000000000000u64 as i64,
    )
    .unwrap();
    assert_eq!(
        hex::encode(short_id.data()).to_uppercase(),
        "83B34F1742838DBCED437626F883FAC84C86A33010358D0423BA9EA8703060DD"
    );

    // Test 2: 0xAA hash, workchain 0, full shard
    let short_id = OverlayUtils::calc_fast_sync_overlay_short_id(
        &make_hash(0xAA),
        0,
        0x8000000000000000u64 as i64,
    )
    .unwrap();
    assert_eq!(
        hex::encode(short_id.data()).to_uppercase(),
        "19A8453DC1BB389A708D517119CF5EDBD592ABEE96190610E2D3DB1617B92F12"
    );

    // Test 3: 0xFF hash, workchain 0, half shard
    let short_id = OverlayUtils::calc_fast_sync_overlay_short_id(
        &make_hash(0xFF),
        0,
        0x4000000000000000u64 as i64,
    )
    .unwrap();
    assert_eq!(
        hex::encode(short_id.data()).to_uppercase(),
        "32A27DC0BB32999146A191BA3D5881362A8E817B7FF01A273918B35CE1F45396"
    );
}

struct TestConsumer {
    pub received: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl Subscriber for TestConsumer {
    async fn try_consume_query(&self, query: TLObject, peers: &AdnlPeers) -> Result<QueryResult> {
        println!("RECEIVED {:?} query {} -> {}", query, peers.other(), peers.local());
        self.received.fetch_add(1, Ordering::Relaxed);
        let query = match query.downcast::<GetCapabilities>() {
            Ok(_) => {
                let answer = TaggedTlObject {
                    object: Capabilities { version_major: 2, version_minor: 1 }
                        .into_boxed()
                        .into_tl_object(),
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                };
                return Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Object(answer)))));
            }
            Err(q) => q,
        };
        fail!("Unknown query {:?}", query);
    }
}

fn test_random_peers(ctx_test: &TestContext) {
    ctx_test.rt.block_on(async move {
        println!("sending getRandomPeers request...");
        let overlay_peers = loop {
            let overlay_peers =
                ctx_test.overlay.wait_for_peers(&ctx_test.overlay_id).await.unwrap();
            if let Some(overlay_peers) = overlay_peers {
                break overlay_peers;
            }
        };
        println!("received {} overlay peers:", overlay_peers.len());
        assert!(!overlay_peers.is_empty());
        for node in overlay_peers {
            println!("{:?}", node);
            let OverlayNodeInfo::V1(node) = node else { panic!("Unexpected V2 node info") };
            let key: Arc<dyn KeyOption> = (&node.id).try_into().unwrap();
            let mut ctx_search =
                AddressSearchContext::with_params(key.id(), DhtSearchPolicy::FastSearch(5))
                    .unwrap();
            match ctx_test.dht.find_address(&mut ctx_search).await {
                Ok(Some((ip, _, _))) => println!("IP {}", ip),
                Ok(None) => println!("Address not found"),
                Err(err) => println!("Error {}", err),
            }
        }
    })
}

fn test_overlay_broadcast_receive(ctx_test: &TestContext) {
    let get_it = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicU32::new(0));
    ctx_test
        .overlay
        .add_consumer(&ctx_test.overlay_id, Arc::new(TestConsumer { received: received.clone() }))
        .unwrap();

    log::info!(target: TARGET, "Use overlay peer {} for receiving test", ctx_test.peer);
    let got_it = get_it.clone();
    let overlay = ctx_test.overlay.clone();
    let overlay_id = ctx_test.overlay_id.clone();
    ctx_test.rt.spawn(async move {
        let now = Instant::now();
        let qty = 25;
        let mut count = 0;
        for _ in 0..qty {
            let recv = overlay.wait_for_broadcast(&overlay_id).await.unwrap().unwrap();
            println!(
                "RECEIVED {} bytes FROM OVERLAY {}/{}",
                recv.data.len(),
                overlay_id,
                recv.recv_from
            );
            count += recv.data.len();
        }
        get_it.store(true, Ordering::Relaxed);
        println!(
            "RECEIVED {} brodcasts ({} bytes) in {} msec",
            qty,
            count,
            now.elapsed().as_millis()
        );
    });

    ctx_test.rt.block_on(async move {
        let start = Instant::now();
        let is_completed =
            || got_it.load(Ordering::Relaxed) && (received.load(Ordering::Relaxed) >= 3);
        let mut known_nodes = std::collections::HashSet::new();
        loop {
            if is_completed() {
                break;
            }
            if start.elapsed().as_secs() > 500 {
                assert!(false)
            }
            let nodes = match ctx_test.overlay.wait_for_peers(&ctx_test.overlay_id).await.unwrap() {
                Some(nodes) => nodes,
                None => break,
            };
            for node in nodes {
                if is_completed() {
                    break;
                }
                let OverlayNodeInfo::V1(node_v1) = &node else {
                    panic!("Unexpected V2 overlay node info")
                };
                let key: Arc<dyn KeyOption> = (&node_v1.id).try_into().unwrap();
                let key_id = key.id();
                if !known_nodes.contains(key_id) {
                    let mut ctx_search =
                        AddressSearchContext::with_params(key_id, DhtSearchPolicy::FastSearch(5))
                            .unwrap();
                    if let Ok(Some((ip, _, _))) = ctx_test.dht.find_address(&mut ctx_search).await {
                        println!("RECEIVED new overlay node {}", key_id);
                        ctx_test.overlay.add_public_peer(&ip, &node, &ctx_test.overlay_id).unwrap();
                        known_nodes.insert(key_id.clone());
                    }
                }
            }
        }
    })
}

fn test_overlay_broadcast_send(ctx_test: &TestContext) {
    ctx_test.rt.block_on(async move {
        let mut data = Vec::new();
        for _ in 0..40 {
            let chunk: [u8; 32] = rand::thread_rng().gen();
            data.extend_from_slice(&chunk)
        }
        let data = TaggedByteSlice {
            object: data.as_slice(),
            #[cfg(feature = "telemetry")]
            tag: 0,
        };
        assert!(
            ctx_test
                .overlay
                .broadcast(&ctx_test.overlay_id, &data, None, 0, AdnlSendMethod::Fast)
                .await
                .unwrap()
                .send_to
                > 0
        );
        sleep(Duration::from_millis(1000));
        let data: [u8; 15] = rand::thread_rng().gen();
        let data = TaggedByteSlice {
            object: &data[..],
            #[cfg(feature = "telemetry")]
            tag: 0,
        };
        assert!(
            ctx_test
                .overlay
                .broadcast(&ctx_test.overlay_id, &data, None, 0, AdnlSendMethod::Fast)
                .await
                .unwrap()
                .send_to
                > 0
        );
        sleep(Duration::from_millis(1000));
    })
}

#[test]
fn test_search_overlay_nodes() {
    let (ctx_test, _, _) = init_overlay_compatibility_test(
        "0.0.0.0:1",
        Some(20),
        #[cfg(feature = "dump")]
        None, //Some(".\\target\\01")
    );
    ctx_test.rt.block_on(async move {
        ctx_test.adnl.stop().await;
    })
}

#[test]
fn test_compatibility_overlay() {
    let (ctx_test, _, _) = init_overlay_compatibility_test(
        "0.0.0.0:1",
        None,
        #[cfg(feature = "dump")]
        None, //Some(".\\target\\01")
    );

    test_random_peers(&ctx_test);
    test_overlay_broadcast_receive(&ctx_test);
    test_overlay_broadcast_send(&ctx_test);

    // Stop
    ctx_test.rt.block_on(async move {
        ctx_test.adnl.stop().await;
    })
}

#[ignore]
#[test]
fn test_hang_broadcast_receive() {
    let (ctx_test, _, _) = init_overlay_compatibility_test(
        "0.0.0.0:1",
        None,
        #[cfg(feature = "dump")]
        None, //Some(".\\target\\01")
    );
    test_overlay_broadcast_receive(&ctx_test);
    ctx_test.rt.block_on(async move { ctx_test.adnl.stop().await });
    {
        let (ctx_test, _, _) = init_overlay_compatibility_test(
            "0.0.0.0:1",
            None,
            #[cfg(feature = "dump")]
            Some(".\\target\\02"),
        );
        ctx_test.rt.block_on(async move { ctx_test.adnl.stop().await })
    }
}

#[ignore]
#[test]
fn test_hang_broadcast_send() {
    loop {
        let (ctx_test, _, _) = init_overlay_compatibility_test(
            "0.0.0.0:1",
            None,
            #[cfg(feature = "dump")]
            None,
        );
        test_overlay_broadcast_send(&ctx_test);
        ctx_test.rt.block_on(async move {
            ctx_test.adnl.stop().await;
        });
        println!("NEXT NEXT");
    }
}

struct LocalNode {
    adnl: Arc<AdnlNode>,
    ip: String,
    overlay: Arc<OverlayNode>,
    overlay_id: Arc<OverlayShortId>,
    rt: tokio::runtime::Runtime,
}

fn init_local_node(ip: String, workers_pool: u8) -> LocalNode {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let zero_state_file_hash = base64_decode(ZERO_STATE).unwrap();
    let zero_state_file_hash = zero_state_file_hash.as_slice().try_into().unwrap();
    let mut config =
        rt.block_on(get_adnl_config("overlay", ip.as_str(), vec![KEY_TAG_OVERLAY], true)).unwrap();
    config.set_recv_worker_pools(Some(workers_pool), Some(75)).unwrap();
    // config.set_throughput(Some(10));
    let adnl = rt
        .block_on(AdnlNode::with_config(
            config,
            #[cfg(feature = "dump")]
            None,
        ))
        .unwrap();
    adnl.set_options(AdnlNode::OPTION_FORCE_COMPRESSION);
    let overlay =
        OverlayNode::with_params(adnl.clone(), zero_state_file_hash, KEY_TAG_OVERLAY).unwrap();
    let rldp = RldpNode::with_params(
        adnl.clone(),
        vec![overlay.clone()],
        None,
        #[cfg(feature = "debug")]
        None,
        #[cfg(feature = "debug")]
        None,
    )
    .unwrap();
    overlay.set_rldp(rldp.clone()).unwrap();
    let overlay_id = overlay.calc_overlay_short_id(-1i32, 0x8000000000000000u64 as i64).unwrap();
    rt.block_on(async {
        adnl.start_over_udp_tcp(vec![overlay.clone(), rldp.clone()]).await.unwrap();
        let params = OverlayParams {
            flags: 0,
            hops: None, /*Some(2)*/
            overlay_id: &overlay_id,
            runtime: None,
        };
        assert!(overlay.add_local_workchain_overlay(params).unwrap());
    });
    LocalNode { adnl, ip, overlay, overlay_id, rt }
}

/*
#[test]
fn test_overlay_broadcast_propagation() {
    loop {
        test_overlay_broadcast_propagation_()
    }
}
*/

const TIMEOUT_BROADCAST_SEC: u64 = 30;
const TIMEOUT_TEST_SEC: u64 = 5;
const SUCCESS_RATIO: f32 = 0.95;

fn get_time_share_ms(percentage: u8) -> u64 {
    TIMEOUT_TEST_SEC * 1000 * (percentage as u64) / 100
}

#[derive(Debug, Clone)]
enum Protocol {
    Fec,
    Simple,
    StreamSimple,
    TwostepFec,
    TwostepSimple,
}

struct RunResult {
    elapsed: u32,
    brcasts: u32,
    queries: u32,
    average: u32,
}

fn run_propagation(
    nodes: &[LocalNode],
    neighbours: &[Arc<Vec<Arc<KeyId>>>],
    protocol: Protocol,
) -> RunResult {
    const DATA_SIZE_FEC: usize = 750 * 1024;
    const DATA_SIZE_SIMPLE: usize = 512;
    const PART_SIZE_TWOSTEP_FEC: usize = 65535;
    const STEPS: usize = 1;

    let data_size = match &protocol {
        Protocol::Simple | Protocol::StreamSimple | Protocol::TwostepSimple => DATA_SIZE_SIMPLE,
        Protocol::TwostepFec => PART_SIZE_TWOSTEP_FEC * neighbours[0].len() / 2,
        _ => DATA_SIZE_FEC,
    };

    println!("\n==========\n\n");

    let nodes_len = nodes.len();
    let start = Arc::new(Instant::now());
    let ping = Arc::new(tokio::sync::Barrier::new(3 * nodes_len + 1));
    let sync = Arc::new(AtomicU32::new(0));
    let bcast_totally = Arc::new(AtomicU32::new(0));
    let bcast_success = Arc::new(AtomicU32::new(0));
    let query_totally = Arc::new(AtomicU32::new(0));
    let query_success = Arc::new(AtomicU32::new(0));
    let query_elapsed = Arc::new(AtomicU32::new(0));

    for i in 0..nodes_len {
        nodes[i].adnl.check();

        let node = nodes[i].overlay.clone();
        let overlay_id = nodes[i].overlay_id.clone();
        let adnl = nodes[i].adnl.clone();
        let adnl_id = adnl.key_by_tag(KEY_TAG_OVERLAY).unwrap().id().clone();
        let sync = sync.clone();
        let neighbours = neighbours[i].clone();
        let bcast_totally = bcast_totally.clone();
        let bcast_success = bcast_success.clone();
        let query_totally = query_totally.clone();
        let query_success = query_success.clone();
        let query_elapsed = query_elapsed.clone();

        let overlay_id_send = overlay_id.clone();
        let adnl_id_send = adnl_id.clone();
        let node_send = node.clone();
        let pong = ping.clone();
        let protocol = protocol.clone();
        let start_global = start.clone();
        let neighbours_len = neighbours.len();
        nodes[i].rt.spawn(async move {
            let mut data = Vec::new();
            data.resize(data_size, 0u8);
            for j in 0..STEPS {
                data[0] = i as u8;
                data[1] = j as u8;
                {
                    let mut rng = rand::thread_rng();
                    for k in 2..data_size - 32 {
                        if k % 16 == 0 {
                            data[k] = 0xFF
                        } else if k % 16 == 8 {
                            data[k] = 0x01
                        } else {
                            data[k] = rng.gen()
                        }
                    }
                }
                let hash = sha256_digest(&data[..data_size - 32]);
                for k in 0..32 {
                    data[data_size - 32 + k] = hash[k]
                }
                let data = TaggedByteSlice {
                    object: data.as_slice(),
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                };
                let info = match &protocol {
                    Protocol::Simple | Protocol::Fec => {
                        node_send
                            .broadcast(&overlay_id_send, &data, None, 0, AdnlSendMethod::Fast)
                            .await
                    }
                    Protocol::StreamSimple => {
                        node_send
                            .broadcast(&overlay_id_send, &data, None, 0, AdnlSendMethod::Safe)
                            .await
                    }
                    Protocol::TwostepSimple | Protocol::TwostepFec => {
                        node_send
                            .broadcast_twostep(&overlay_id_send, &data, None, 0, Vec::new())
                            .await
                    }
                }
                .unwrap();
                println!("{} {} {}", info.send_to, neighbours_len, nodes_len);
                assert_eq!(info.send_to as usize, min(neighbours_len, nodes_len - 1));
                println!(
                    "Broadcasting {}->{} packets by {adnl_id_send}/{}, step {j}\n",
                    info.packets,
                    info.send_to,
                    adnl.ip_address_adnl(),
                );
                bcast_totally.fetch_add(1, Ordering::Relaxed);
            }
            println!(
                "{} Broadcasting Finished {adnl_id_send}",
                start_global.elapsed().as_millis() as u32
            );
            pong.wait().await;
        });

        let overlay_id_query = overlay_id.clone();
        let adnl_id_query = adnl_id.clone();
        let node_query = node.clone();
        let neighbours_query = neighbours.clone();
        let pong = ping.clone();
        let start_global = start.clone();
        nodes[i].rt.spawn(async move {
            let query: TaggedTlObject = OverlayPing.into_tl_object().into();
            println!("{adnl_id_query} Sending overlay Ping query");
            for _j in 0..STEPS {
                let start = Instant::now();
                let time_share = get_time_share_ms(80);
                while (start.elapsed().as_millis() as u64) < time_share {
                    for neighbour in neighbours_query.iter() {
                        let start1 = start.elapsed().as_millis();
                        query_totally.fetch_add(1, Ordering::Relaxed);
                        let res =
                            node_query.query(neighbour, &query, &overlay_id_query, None).await;
                        let elapsed = (start.elapsed().as_millis() - start1) as u32;
                        query_elapsed.fetch_add(elapsed, Ordering::Relaxed);
                        if let Ok(Some(_)) = res {
                            query_success.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            let totally = query_totally.load(Ordering::Relaxed);
            let success = query_success.load(Ordering::Relaxed);
            println!(
                "{} Querying finished {adnl_id_query}, sent {totally} success {success}",
                start_global.elapsed().as_millis() as u32
            );
            pong.wait().await;
        });

        let pong = ping.clone();
        let start_global = start.clone();
        nodes[i].rt.spawn(async move {
            for j in 0..STEPS {
                let mut mask = 1u64 << i;
                for _ in 0..nodes_len - 1 {
                    tokio::select! {
                        recv = node.wait_for_broadcast(&overlay_id) => {
                            let recv = recv.unwrap().unwrap();
                            assert_eq!(recv.data.len(), data_size);
                            let y = recv.data[0];
                            assert_eq!(recv.data[1], j as u8); // jj as u8);
                            let hash = sha256_digest(&recv.data[..data_size - 32]);
                            for k in 0..32 {
                                assert_eq!(recv.data[data_size - 32 + k], hash[k])
                            }
                            mask |= 1 << (y & 0x0F);
                            println!("{:=<10}", "");
                            println!(
                                "Finished {adnl_id} with {} packets from {} in {} ms, \
                                step {j} tag {y:x}\n",
                                recv.packets,
                                recv.recv_from,
                                start_global.elapsed().as_millis(),
                            );
                        },
                        _ = tokio::time::sleep(
                            Duration::from_secs(TIMEOUT_BROADCAST_SEC)
                        ) => {},
                    }
                }
                sync.fetch_or(1 << i, Ordering::Relaxed);
                if mask == ((1 << nodes_len) - 1) {
                    bcast_success.fetch_add(1, Ordering::Relaxed);
                }
                println!(
                    "==========\nFinished {adnl_id} in {} ms, step {j} mask {mask:x}\n",
                    start_global.elapsed().as_millis()
                );
            }
            println!("{} Receiving Finished {adnl_id}", start_global.elapsed().as_millis() as u32);
            pong.wait().await;
        });

        /*
            rt.spawn(
                async move {
                    while sync_trace.load(Ordering::Relaxed) != (1 << NODES) -1 {
                        tokio::time::sleep(Duration::from_millis(2000)).await;
                        println!(
                            "==========\nTrace {}: {:x}\n",
                            adnl_id_trace,
                            node_trace.get_debug_trace(&overlay_id_trace).unwrap()
                        )
                    }
                }
            );
        */
    }

    nodes[0].rt.block_on(ping.wait());
    let elapsed = start.elapsed().as_millis() as u32;
    println!("==========\nBroadcast protocol {protocol:?}: elapsed {} ms\n", elapsed);
    let bcast_totally = bcast_totally.load(Ordering::Relaxed);
    let bcast_success = bcast_success.load(Ordering::Relaxed);
    let bcast_quality = bcast_success as f32 / bcast_totally as f32;
    let query_totally = query_totally.load(Ordering::Relaxed);
    let query_success = query_success.load(Ordering::Relaxed);
    let query_quality = query_success as f32 / query_totally as f32;
    println!(
        "==========\nBroadcasts {}/{} success ({})\n",
        bcast_success, bcast_totally, bcast_quality
    );
    println!(
        "==========\nQueries {}/{} success ({})\n",
        query_success, query_totally, query_quality
    );
    assert!(SUCCESS_RATIO < bcast_quality);
    assert!(SUCCESS_RATIO < query_quality);
    RunResult {
        elapsed,
        brcasts: bcast_totally,
        queries: query_totally,
        average: query_elapsed.load(Ordering::Relaxed) / query_totally,
    }
}

fn test_broadcast(
    n: usize,
    test: impl Fn(&[LocalNode], &[Arc<Vec<Arc<KeyId>>>], Protocol) -> RunResult,
    protocol: Protocol,
) {
    let min_neighbours = match protocol {
        Protocol::StreamSimple | Protocol::TwostepFec => return, /* Not ready yet */
        //Protocol::TwostepFec => 4,
        Protocol::TwostepSimple => 3,
        _ => 1,
    };

    init_test();
    let mut nodes = Vec::new();
    for i in 0..n {
        let port = BROADCAST_TEST_BASE_PORT + i as u16;
        let ip = format!("127.0.0.1:{port}");
        nodes.push(init_local_node(ip, 100 / n as u8));
    }

    let mut neighbours = Vec::new();
    for i in 0..n {
        let overlay_id = &nodes[i].overlay_id;
        let mut tmp = Vec::new();
        for j in 0..min(min_neighbours, n - 1) {
            let j = (j + i + 1) % n;
            assert!(j != i);
            let signed_node = nodes[j].overlay.get_signed_node(overlay_id, false).unwrap();
            let ip = IpAddress::from_versioned_string(nodes[j].ip.as_str(), None).unwrap();
            let dst =
                nodes[i].overlay.add_public_peer(&ip, &signed_node, overlay_id).unwrap().unwrap();
            nodes[i].rt.block_on(nodes[i].overlay.wait_for_peers(overlay_id)).unwrap();
            tmp.push(dst);
        }
        neighbours.push(Arc::new(tmp));
    }

    sleep(Duration::from_millis(get_time_share_ms(5)));
    let mut avg_elapsed = 0;
    let mut avg_brcasts = 0;
    let mut avg_queries = 0;
    let mut avg_average = 0;
    let mut avg_counted = 0;
    while avg_counted < 3 {
        let res = test(&nodes, &neighbours, protocol.clone());
        let elapsed = (res.elapsed as i32 - avg_elapsed) / (avg_counted + 1);
        let brcasts = (res.brcasts as i32 - avg_brcasts) / (avg_counted + 1);
        let queries = (res.queries as i32 - avg_queries) / (avg_counted + 1);
        let average = (res.average as i32 - avg_average) / (avg_counted + 1);
        avg_elapsed += elapsed;
        avg_brcasts += brcasts;
        avg_queries += queries;
        avg_average += average;
        avg_counted += 1;
        println!(
            "==== \nAverage({avg_counted} attempts): \
            elapsed {avg_elapsed} ms, broadcasts {avg_brcasts}, queries {avg_queries}, \
            average query time {avg_average} ms\n"
        );
        assert!((avg_elapsed as u64) < (TIMEOUT_BROADCAST_SEC + 1) * 1000);
        sleep(Duration::from_millis(get_time_share_ms(15)));
    }

    #[cfg(feature = "telemetry")]
    for node in nodes.iter() {
        node.overlay.stats().unwrap();
    }
    for node in nodes.iter() {
        node.rt.block_on(node.adnl.stop());
    }
}

#[test]
fn test_overlay_broadcast() {
    test_broadcast(5, run_propagation, Protocol::Simple);
    test_broadcast(5, run_propagation, Protocol::Fec);
    test_broadcast(5, run_propagation, Protocol::TwostepSimple);
    test_broadcast(5, run_propagation, Protocol::TwostepFec);
    test_broadcast(5, run_propagation, Protocol::StreamSimple);
}

#[ignore]
#[test]
fn test_overlay_ping() {
    const FILE: &str = "ping";

    async fn load_peer(
        overlay: &Arc<OverlayNode>,
        overlay_id: &Arc<OverlayShortId>,
        peer: &String,
    ) -> Result<Arc<KeyId>> {
        let data: Vec<&str> = peer.split(' ').collect();
        if data.len() != 2 {
            fail!("Bad saved peer {}", peer)
        }
        let node = deserialize_boxed(&base64_decode(data[0])?)?
            .downcast::<NodeV1Boxed>()
            .map_err(|_| error!("Bad node in saved peer {}", peer))?;
        let ip = IpAddress::from_versioned_string(data[1], None)?;
        test_peer(overlay, overlay_id, &ip, node).await
    }

    async fn test_peer(
        overlay: &Arc<OverlayNode>,
        overlay_id: &Arc<OverlayShortId>,
        ip: &IpAddress,
        node: NodeV1Boxed,
    ) -> Result<Arc<KeyId>> {
        if let Some(peer) = overlay.add_public_peer(
            ip,
            &OverlayNodeInfo::<_, NodeV2>::V1(node.only()),
            &overlay_id,
        )? {
            if overlay.wait_for_peers(overlay_id).await?.is_some() {
                return Ok(peer);
            }
        }
        fail!("Fail to add peer {}", ip)
    }

    println!("\ninitializing...");
    let (mut ctx_test, _, _) = init_overlay_compatibility_test(
        "0.0.0.0:1",
        None,
        #[cfg(feature = "dump")]
        None,
    );
    ctx_test.rt.block_on(async move {
        println!("gathering random peers...");
        let mut peers = Vec::new();
        if let Ok(file) = File::open(FILE) {
            for line in BufReader::new(file).lines() {
                if let Ok(line) = line {
                    if let Ok(peer) =
                        load_peer(&ctx_test.overlay, &ctx_test.overlay_id, &line).await
                    {
                        if !peers.contains(&peer) {
                            peers.push(peer)
                        }
                    }
                }
            }
        }
        loop {
            if peers.len() >= 48 {
                break;
            }
            let overlay_peers = loop {
                let overlay_peers =
                    ctx_test.overlay.wait_for_peers(&ctx_test.overlay_id).await.unwrap();
                if let Some(overlay_peers) = overlay_peers {
                    break overlay_peers;
                }
            };
            println!("received {} overlay peers:", overlay_peers.len());
            for node in overlay_peers {
                println!("{:?}", node);
                let OverlayNodeInfo::V1(node) = node else {
                    panic!("Unexpected V2 overlay node info")
                };
                let key: Arc<dyn KeyOption> = (&node.id).try_into().unwrap();
                let mut ctx_search =
                    AddressSearchContext::with_params(key.id(), DhtSearchPolicy::FastSearch(5))
                        .unwrap();
                match ctx_test.dht.find_address(&mut ctx_search).await {
                    Ok(Some((ip, _, _))) => {
                        println!("IP {}", ip);
                        let node = node.into_boxed();
                        let node_encoded = base64_encode(&serialize_boxed(&node).unwrap());
                        if let Ok(peer) =
                            test_peer(&ctx_test.overlay, &ctx_test.overlay_id, &ip, node).await
                        {
                            if !peers.contains(&peer) {
                                let mut file = OpenOptions::new()
                                    .create(true)
                                    .write(true)
                                    .append(true)
                                    .open(FILE)
                                    .unwrap();
                                writeln!(file, "{} {}", node_encoded, ip).unwrap();
                                peers.push(peer);
                            }
                        }
                    }
                    Ok(None) => println!("Address not found"),
                    Err(err) => println!("Error {}", err),
                }
            }
            if !peers.is_empty() {
                ctx_test.peer = peers[peers.len() - 1].clone()
            }
        }
        //  let ping = Arc::new(tokio::sync::Barrier::new(peers.len() + 1));
        let mut stat = Vec::new();
        let mark = Arc::new(Instant::now());
        for peer in peers {
            let peer = peer.clone();
            let overlay_id = ctx_test.overlay_id.clone();
            let overlay = ctx_test.overlay.clone();
            let mark = mark.clone();
            let pong = Arc::new(AtomicU64::new(mark.elapsed().as_millis() as u64));
            stat.push((peer.clone(), pong.clone()));
            tokio::spawn(async move {
                let query = TaggedTlObject {
                    object: GetCapabilities.into_tl_object(),
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                };
                loop {
                    #[allow(clippy::single_match)]
                    match overlay.query(&peer, &query, &overlay_id, None).await {
                        Ok(Some(_answer)) => {
                            // let caps: CapabilitiesBoxed = Query::parse(answer, &query).unwrap();
                            // println!("Got capabilities from {}: {:?}", peer, caps);
                        }
                        _ => (), //println!("No capabilities from {}", peer)
                    }
                    pong.store(mark.elapsed().as_millis() as u64, Ordering::Relaxed);
                }
            });
        }
        // ping.wait().await;
        loop {
            for s in stat.iter() {
                let (peer, pong) = s;
                let diff =
                    (mark.elapsed().as_millis() as i64 - pong.load(Ordering::Relaxed) as i64).abs();
                if diff > 5000 {
                    println!("Ping {} seems to be hang ({}ms)", peer, diff);
                }
            }
            sleep(Duration::from_millis(1000))
        }
        // adnl.stop().await;
    })
}

fn node(src: &str, dst: &str, sync: Arc<AtomicU16>, mask: u16, wait: u16) -> Result<()> {
    let src: SocketAddr = src.parse()?;
    let dst: SocketAddr = dst.parse()?;
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, None)?;
    //    socket.set_send_buffer_size(1 << 28)?;
    socket.set_recv_buffer_size(1 << 30)?;
    socket.set_nonblocking(true)?;
    socket.bind(&src.into())?;
    let sync_send = sync.clone();
    let socket_send = socket.try_clone()?;
    thread::spawn(move || {
        while sync_send.load(Ordering::Relaxed) != wait {
            thread::sleep(Duration::from_millis(1));
        }
        let dst: socket2::SockAddr = dst.into();
        let mut buf = [0; 1024];
        for i in 0..1000000 {
            buf[0] = (i >> 16) as u8;
            buf[1] = (i >> 8) as u8;
            buf[2] = (i >> 0) as u8;
            let size = loop {
                match socket_send.send_to(&buf, &dst) {
                    Ok(size) => break size,
                    Err(err) => match err.kind() {
                        std::io::ErrorKind::WouldBlock => continue,
                        _ => panic!("Error SEND {:?}", err),
                    },
                }
            };
            if size != 1024 {
                panic!("Bad send size {}, expected 1024", size);
            }
        }
    });
    let socket: std::net::UdpSocket = socket.into();
    sync.fetch_or(mask, Ordering::Relaxed);
    let mut buf = [0; 1024];
    //    let wait = Metric::with_name("wait recv, ns");
    for i in 0..1000000 {
        let size = loop {
            match socket.recv(&mut buf) {
                Ok(size) => break size,
                Err(err) => match err.kind() {
                    std::io::ErrorKind::WouldBlock => continue,
                    _ => panic!("Error RECV {:?}", err),
                },
            }
        };
        if size != 1024 {
            panic!("Bad recv size {}, expected 1024", size);
        }
        if buf[0] != ((i >> 16) as u8) {
            panic!("Bad seqno {:02x}{:02x}{:02x}, expected {:06x}", buf[0], buf[1], buf[2], i);
        }
        if buf[1] != ((i >> 8) as u8) {
            panic!("Bad seqno {:02x}{:02x}{:02x}, expected {:06x}", buf[0], buf[1], buf[2], i);
        }
        if buf[2] != ((i >> 0) as u8) {
            panic!("Bad seqno {:02x}{:02x}{:02x}, expected {:06x}", buf[0], buf[1], buf[2], i);
        }
    }
    Ok(())
}

#[ignore]
#[test]
fn test_network() {
    fn start(
        sync: &Arc<AtomicU16>,
        src: &'static str,
        dst: &'static str,
        mask: u16,
    ) -> JoinHandle<Result<()>> {
        let sync = sync.clone();
        thread::spawn(move || node(src, dst, sync, mask, 0x01FF))
    }

    fn check(h: JoinHandle<Result<()>>, name: &str) {
        match h.join() {
            Ok(k) => println!("{} ok {:?}", name, k),
            Err(e) => println!("{} failed {:?}", name, e),
        }
    }

    const NODE1: &str = "127.0.0.1:4191";
    const NODE2: &str = "127.0.0.1:4192";
    const NODE3: &str = "127.0.0.1:4193";
    const NODE4: &str = "127.0.0.1:4194";
    const NODE5: &str = "127.0.0.1:4195";
    const NODE6: &str = "127.0.0.1:4196";
    const NODE7: &str = "127.0.0.1:4197";
    const NODE8: &str = "127.0.0.1:4198";
    const NODE9: &str = "127.0.0.1:4199";

    let sync = Arc::new(AtomicU16::new(0));
    let begin = Instant::now();
    let h1 = start(&sync, NODE1, NODE2, 0x0001);
    let h2 = start(&sync, NODE2, NODE3, 0x0002);
    let h3 = start(&sync, NODE3, NODE4, 0x0004);
    let h4 = start(&sync, NODE4, NODE5, 0x0008);
    let h5 = start(&sync, NODE5, NODE6, 0x0010);
    let h6 = start(&sync, NODE6, NODE7, 0x0020);
    let h7 = start(&sync, NODE7, NODE8, 0x0040);
    let h8 = start(&sync, NODE8, NODE9, 0x0080);
    let h9 = start(&sync, NODE9, NODE1, 0x0100);

    println!();
    check(h1, "1st");
    check(h2, "2nd");
    check(h3, "3rd");
    check(h4, "4th");
    check(h5, "5th");
    check(h6, "6th");
    check(h7, "7th");
    check(h8, "8th");
    check(h9, "9th");
    println!("Elapsed {} ms", begin.elapsed().as_millis());
}

#[test]
fn test_metric() {
    init_test();
    const PERIOD: u64 = 5;
    let builder =
        MetricBuilder::with_metric_and_period(Metric::without_totals("metric", PERIOD), 1000000000);
    let printer = TelemetryPrinter::with_params(
        "Metric test",
        5,
        vec![TelemetryItem::MetricBuilder(builder.clone())],
    );
    for i in 0..39 {
        if i % 3 == 0 {
            sleep(Duration::from_millis(500));
            builder.update(1);
            sleep(Duration::from_millis(500));
        } else if i % 3 == 1 {
            sleep(Duration::from_millis(100));
            builder.update(1);
            sleep(Duration::from_millis(300));
            builder.update(1);
            sleep(Duration::from_millis(300));
            builder.update(2);
            sleep(Duration::from_millis(200));
            builder.update(1);
            sleep(Duration::from_millis(100));
        } else {
            sleep(Duration::from_millis(100));
            builder.update(1);
            sleep(Duration::from_millis(200));
            builder.update(1);
            sleep(Duration::from_millis(300));
            builder.update(1);
            sleep(Duration::from_millis(200));
            builder.update(1);
            sleep(Duration::from_millis(100));
            builder.update(2);
            sleep(Duration::from_millis(100));
        }
        printer.try_print();
        if i > PERIOD {
            match builder.metric().get_average() {
                3 | 4 => (),
                x => {
                    println!("Average {}, expected 3 or 4", x);
                    assert!(false)
                }
            }
            assert_eq!(builder.metric().maximum(), 6);
        }
    }
}

#[test]
fn test_stop() {
    let ctx_test = init_overlay_simple_compatibility_test(
        "0.0.0.0:1",
        #[cfg(feature = "dump")]
        None,
    );
    let overlay_id = KeyId::from_data([0xCC; 32]);
    let params = OverlayParams {
        flags: 0,
        hops: None,
        overlay_id: &overlay_id,
        runtime: Some(ctx_test.rt.handle().clone()),
    };
    let added = ctx_test
        .overlay
        .add_private_overlay(
            params,
            &ctx_test.adnl.key_by_tag(KEY_TAG_OVERLAY).unwrap(),
            &Vec::new(),
            false,
        )
        .unwrap();
    assert!(added);
    let overlay_cloned = ctx_test.overlay.clone();
    let overlay_id_cloned = overlay_id.clone();
    ctx_test.rt.spawn(async move {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        let dropped = overlay_cloned.delete_private_overlay(&overlay_id_cloned).unwrap();
        assert!(dropped)
    });
    ctx_test.rt.block_on(async move {
        let wait = ctx_test.overlay.wait_for_broadcast(&overlay_id).await.unwrap();
        assert!(wait.is_none())
    })
}

#[ignore]
#[test]
fn test_drop() {
    fn remove(map: Arc<lockfree::map::Map<u8, Arc<u8>>>) {
        let map_cloned = map.clone();
        if let Some(item) = map.get(&0) {
            thread::spawn(move || {
                if let Some(removed) = map_cloned.remove(&0) {
                    println!("drop1 {}", Arc::strong_count(removed.val()));
                }
            })
            .join()
            .ok();
            println!("drop2 {}", Arc::strong_count(item.val()));
        }
        //        if let Some(item) = map.get(&0) {
        //            println!("drop3 {}", Arc::strong_count(item.val()));
        //        }
    }

    let map = Arc::new(lockfree::map::Map::new());
    let item = Arc::new(0);
    map.insert(0, item.clone());
    remove(map.clone());
    /*
        map.insert(0, item.clone());
        remove(map.clone());
        map.insert(0, item.clone());
        remove(map.clone());
        map.insert(0, item.clone());
        remove(map.clone());
        map.insert(0, item.clone());
        remove(map.clone());
    */
    println!("drop final {}", Arc::strong_count(&item));
}

#[test]
fn test_new_broadcast() {
    const HOPS: u8 = 5;
    let src = Ed25519KeyOption::generate().unwrap();
    let bcast = BroadcastOrd {
        src: (&src).try_into().unwrap(),
        certificate: OverlayCertificate::Overlay_EmptyCertificate,
        flags: 0,
        data: vec![3; 100].into(),
        date: 0,
        signature: vec![2; 32].into(),
    }
    .into_boxed();
    let mut buf = serialize_boxed(&bcast).unwrap();
    buf.extend_from_slice(&[HOPS]);
    let (obj, pos) = deserialize_boxed_with_suffix(&buf).unwrap();
    println!("obj {:?}", obj);
    assert_eq!(&buf[pos..], &[HOPS]);
}

#[derive(Clone)]
struct PeerInfo<'a> {
    id: Arc<KeyId>,
    ip: &'a str,
    pub_key: Arc<dyn KeyOption>,
    key: Arc<dyn KeyOption>,
    adnl: Option<Arc<AdnlNode>>,
    overlay: Option<Arc<OverlayNode>>,
    received: Arc<AtomicU32>,
    received_bcasts: Arc<AtomicU32>,
    certificate: Option<MemberCertificate>,
}

#[tokio::test(flavor = "multi_thread")]
async fn test_overlay_semiprivate() -> Result<()> {
    async fn prepare_peer(ip: &str) -> Result<(AdnlNodeConfig, PeerInfo<'_>)> {
        let cfg = get_adnl_config("overlay", ip, vec![KEY_TAG_OVERLAY], true).await?;
        let id = cfg.key_by_tag(KEY_TAG_OVERLAY)?.id().clone();
        let pi = PeerInfo {
            id,
            ip,
            pub_key: Ed25519KeyOption::from_public_key(
                cfg.key_by_tag(KEY_TAG_OVERLAY)?.pub_key()?.try_into()?,
            ),
            key: cfg.key_by_tag(KEY_TAG_OVERLAY)?.clone(),
            adnl: None,
            overlay: None,
            received: Arc::new(AtomicU32::new(0)),
            received_bcasts: Arc::new(AtomicU32::new(0)),
            certificate: None,
        };
        Ok((cfg, pi))
    }

    async fn create_root(
        cfg: AdnlNodeConfig,
        roots: &[Arc<KeyId>],
        zero_state_file_hash: &[u8; 32],
        overlay_id: &Arc<OverlayShortId>,
        pi: &mut PeerInfo<'_>,
    ) -> Result<()> {
        let adnl = AdnlNode::with_config(
            cfg,
            #[cfg(feature = "dump")]
            None,
        )
        .await?;
        pi.adnl = Some(adnl.clone());
        let overlay =
            OverlayNode::with_params(adnl.clone(), &zero_state_file_hash, KEY_TAG_OVERLAY)?;
        pi.overlay = Some(overlay.clone());

        adnl.start_over_udp(vec![pi.overlay.clone().unwrap()]).await.unwrap();
        let params = OverlayParams::with_id_only(overlay_id);
        assert!(overlay.add_semiprivate_overlay(params, Some(&pi.key), roots, None, 1)?);
        overlay
            .add_consumer(overlay_id, Arc::new(TestConsumer { received: pi.received.clone() }))?;

        let received_bcasts = pi.received_bcasts.clone();
        let overlay_id = overlay_id.clone();
        tokio::spawn(async move {
            while let Ok(Some(_)) = overlay.wait_for_broadcast(&overlay_id).await {
                received_bcasts.fetch_add(1, Ordering::Relaxed);
            }
        });

        Ok(())
    }

    async fn create_slave(
        cfg: AdnlNodeConfig,
        roots: &[Arc<KeyId>],
        zero_state_file_hash: &[u8; 32],
        overlay_id: &Arc<OverlayShortId>,
        root: &PeerInfo<'_>,
        pi: &mut PeerInfo<'_>,
    ) -> Result<()> {
        let adnl = AdnlNode::with_config(
            cfg,
            #[cfg(feature = "dump")]
            None,
        )
        .await?;
        pi.adnl = Some(adnl.clone());
        let overlay =
            OverlayNode::with_params(adnl.clone(), &zero_state_file_hash, KEY_TAG_OVERLAY)?;
        pi.overlay = Some(overlay.clone());

        adnl.start_over_udp(vec![overlay.clone()]).await.unwrap();

        let utime = UnixTime::now();

        let cert_id = ton_api::ton::overlay::membercertificateid::MemberCertificateId {
            node: ton_api::ton::adnl::id::short::Short { id: UInt256::from_slice(pi.id.data()) },
            flags: 0,
            slot: 0,
            expire_at: (utime + 7) as i32,
        };
        let signature = root.key.sign(&serialize_boxed(&cert_id.clone().into_boxed())?)?;
        let cert = ton_api::ton::overlay::membercertificate::MemberCertificate {
            issued_by: (&root.pub_key).try_into()?,
            flags: cert_id.flags,
            slot: cert_id.slot,
            expire_at: cert_id.expire_at,
            signature,
        };

        pi.certificate = Some(cert.clone());
        let params = OverlayParams::with_id_only(overlay_id);
        assert!(overlay.add_semiprivate_overlay(params, Some(&pi.key), roots, Some(cert), 1)?);
        overlay
            .add_consumer(overlay_id, Arc::new(TestConsumer { received: pi.received.clone() }))?;

        let received_bcasts = pi.received_bcasts.clone();
        let overlay_id = overlay_id.clone();
        tokio::spawn(async move {
            while let Ok(Some(_)) = overlay.wait_for_broadcast(&overlay_id).await {
                received_bcasts.fetch_add(1, Ordering::Relaxed);
            }
        });

        Ok(())
    }

    std::env::set_var("RUST_BACKTRACE", "full");

    let zero_state_file_hash = base64_decode(ZERO_STATE)?;
    let zero_state_file_hash = zero_state_file_hash.as_slice().try_into()?;
    let overlay_id = OverlayUtils::calc_overlay_short_id(
        -1i32,
        0x8000000000000000u64 as i64,
        &zero_state_file_hash,
    )?;
    const ROOT1: &str = "127.0.0.1:4201";
    const ROOT2: &str = "127.0.0.1:4202";
    const ROOT3: &str = "127.0.0.1:4203";
    const SLAVE1: &str = "127.0.0.1:4204";
    const SLAVE2: &str = "127.0.0.1:4205";
    const SLAVE3: &str = "127.0.0.1:4206";
    const SLAVE4: &str = "127.0.0.1:4207";

    init_test_log();

    let mut peers = Vec::new();

    // Prepare root configs
    let mut cfgs = Vec::new();
    let mut root_ids = Vec::new();
    for ip in [ROOT1, ROOT2, ROOT3] {
        let (cfg, pi) = prepare_peer(ip).await?;
        println!("Root node {} with id {}", ip, pi.id);
        cfgs.push(cfg);
        root_ids.push(pi.id.clone());
        peers.push(pi);
    }

    // Create root nodes
    for (cfg, pi) in (cfgs.into_iter()).zip(peers.iter_mut()) {
        create_root(cfg, &root_ids, &zero_state_file_hash, &overlay_id, pi).await?;
    }

    // Prepare slaves
    let mut cfgs = Vec::new();
    for ip in [SLAVE1, SLAVE2, SLAVE3] {
        let (cfg, pi) = prepare_peer(ip).await?;
        println!("Slave node {} with id {}", ip, pi.id);
        cfgs.push(cfg);
        peers.push(pi);
    }

    // Create slaves
    for i in 0..3 {
        let root = peers[i].clone();
        create_slave(
            cfgs.remove(0),
            &root_ids,
            &zero_state_file_hash,
            &overlay_id,
            &root,
            &mut peers[3 + i],
        )
        .await?;
    }

    // Register all peers
    for pi in peers.iter() {
        let overlay = pi.overlay.as_ref().unwrap();
        for pi2 in peers.iter() {
            if pi.id != pi2.id {
                println!("Adding peer {} -> {}", pi.id, pi2.id);
                let node = pi2.overlay.as_ref().unwrap().get_signed_node(&overlay_id, true)?;
                let ip = IpAddress::from_versioned_string(pi2.ip, None).unwrap();
                overlay.add_public_peer(&ip, &node, &overlay_id)?;
            }
        }
    }

    // send/receive query each to each
    for pi in &peers {
        for pi2 in &peers {
            let overlay_node = pi.overlay.as_ref().unwrap();
            if pi.id != pi2.id {
                println!("Sending query  {} -> {}", pi.id, pi2.id);
                let query = TaggedTlObject {
                    object: GetCapabilities.into_tl_object(),
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                };
                assert!(overlay_node.query(&pi2.id, &query, &overlay_id, None).await?.is_some());
            }
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    for pi in &peers {
        assert!(pi.received.load(Ordering::Relaxed) >= peers.len() as u32 - 1);
    }

    // send/receive broadcasts
    let overlay = peers.last().unwrap().overlay.as_ref().unwrap();
    for len in [1, 26, 100, 1024, 5678] {
        let data = (0..len).map(|_| rand::random::<u8>()).collect::<Vec<u8>>();
        let obj = &TaggedByteSlice {
            object: &data,
            #[cfg(feature = "telemetry")]
            tag: 123,
        };
        let si = overlay.broadcast(&overlay_id, obj, None, 0, AdnlSendMethod::Fast).await?;
        println!("Sent broadcast {:?}", si);
    }
    tokio::time::sleep(Duration::from_secs(5)).await;
    for pi in &peers[..5] {
        let received_bcasts = pi.received_bcasts.load(Ordering::Relaxed);
        println!("Peer {} received {} broadcasts", pi.id, received_bcasts);
        assert!(received_bcasts >= 1);
    }

    // replace one slave by new
    tokio::time::sleep(Duration::from_secs(1)).await;
    let (cfg, mut pi) = prepare_peer(SLAVE4).await?;
    println!("New slave node {} with id {}", SLAVE4, pi.id);
    let root = peers[2].clone();
    create_slave(cfg, &root_ids, &zero_state_file_hash, &overlay_id, &root, &mut pi).await?;
    let node = pi.overlay.as_ref().unwrap().get_signed_node(&overlay_id, true)?;
    for pi1 in peers.iter() {
        let overlay = pi1.overlay.as_ref().unwrap();
        println!("Adding peer {} to {}", pi.id, pi1.id);
        let ip = IpAddress::from_versioned_string(pi.ip, None).unwrap();
        overlay.add_public_peer(&ip, &node, &overlay_id)?;
        println!("Adding peer {} to {}", pi1.id, pi.id);
        let overlay = pi.overlay.as_ref().unwrap();
        let node1 = pi1.overlay.as_ref().unwrap().get_signed_node(&overlay_id, true)?;
        let ip = IpAddress::from_versioned_string(pi1.ip, None).unwrap();
        overlay.add_public_peer(&ip, &node1, &overlay_id)?;
    }

    // send query from new slave
    // (not to all to check futher that add_public_peer_v2 replaces old peer correctly)
    let overlay_node = pi.overlay.as_ref().unwrap();
    for pi2 in &peers[0..2] {
        println!("Sending query  {} -> {}", pi.id, pi2.id);
        let query = TaggedTlObject {
            object: GetCapabilities.into_tl_object(),
            #[cfg(feature = "telemetry")]
            tag: 0,
        };
        assert!(overlay_node.query(&pi2.id, &query, &overlay_id, None).await?.is_some());
    }

    // check old slave can't send query (nobody answers)
    for pi2 in &peers[0..4] {
        let pi = &peers[5];
        let overlay_node = pi.overlay.as_ref().unwrap();
        if pi.id != pi2.id {
            println!("Sending query  {} -> {}", pi.id, pi2.id);
            let query = TaggedTlObject {
                object: GetCapabilities.into_tl_object(),
                #[cfg(feature = "telemetry")]
                tag: 0,
            };
            assert!(overlay_node.query(&pi2.id, &query, &overlay_id, None).await?.is_none());
        }
    }

    // wait
    tokio::time::sleep(Duration::from_secs(6)).await;

    // Try to send from expired slave
    let slave = &peers[3];
    let root = &peers[0];
    println!("Sending query  {} -> {}", slave.id, root.id);
    let query = TaggedTlObject {
        object: GetCapabilities.into_tl_object(),
        #[cfg(feature = "telemetry")]
        tag: 0,
    };
    let before = root.received.load(Ordering::Relaxed);
    let overlay_node = slave.overlay.as_ref().unwrap();
    assert!(overlay_node.query(&root.id, &query, &overlay_id, None).await?.is_none());
    tokio::time::sleep(Duration::from_secs(1)).await;
    // check nothing received
    assert_eq!(before, root.received.load(Ordering::Relaxed));

    Ok(())
}

#[test]
fn test_overlay_raptorq() {
    use rand::{seq::SliceRandom, SeedableRng};

    fn run(symbol: Option<u32>) {
        let seed: u64 = rand::random();
        println!("test_overlay_raptorq seed: {seed}");
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut data_size = 1400usize;
        let mut steps = 0usize;
        let mut retries_n1 = 0usize; // needed N+1 symbols
        let mut retries_n2 = 0usize; // needed N+2 symbols

        while data_size <= 1_400_000 {
            steps += 1;
            let data: Vec<u8> = (0..data_size).map(|_| rng.gen()).collect();

            // Encode: generate source_count * 1.5 total symbols
            let mut encoder = RaptorqEncoder::with_data(&data, symbol);
            let params = encoder.params().clone();
            let source_count = params.symbols_count as usize;
            let total_packets = source_count * 3 / 2;
            let repair_count = total_packets - source_count;

            let mut source_packets: Vec<(u32, Vec<u8>)> = Vec::new();
            let mut repair_packets: Vec<(u32, Vec<u8>)> = Vec::new();
            let mut seqno = 0u32;
            for i in 0..total_packets {
                let seqno_original = seqno;
                let chunk = encoder.encode(&mut seqno).unwrap();
                if i < source_count {
                    source_packets.push((seqno, chunk));
                } else {
                    repair_packets.push((seqno, chunk));
                }
                if seqno_original == seqno {
                    seqno += 1;
                }
            }

            // N = source_count (minimum symbols needed to decode)
            // Drop (total - N) packets: 90% from source, 10% from repair
            let to_drop = total_packets - source_count;
            let drop_from_source = to_drop * 9 / 10;
            let drop_from_repair = to_drop - drop_from_source;
            let keep_from_source = source_count - drop_from_source;
            let keep_from_repair = repair_count - drop_from_repair;

            // Randomly select which packets to keep; collect 2 spares from the dropped ones
            let mut source_indices: Vec<usize> = (0..source_packets.len()).collect();
            let mut repair_indices: Vec<usize> = (0..repair_packets.len()).collect();
            source_indices.shuffle(&mut rng);
            repair_indices.shuffle(&mut rng);

            let mut spares: Vec<(u32, Vec<u8>)> = Vec::new();
            for &idx in source_indices[keep_from_source..].iter() {
                spares.push(source_packets[idx].clone());
                if spares.len() >= 2 {
                    break;
                }
            }
            if spares.len() < 2 {
                for &idx in repair_indices[keep_from_repair..].iter() {
                    spares.push(repair_packets[idx].clone());
                    if spares.len() >= 2 {
                        break;
                    }
                }
            }

            source_indices.truncate(keep_from_source);
            repair_indices.truncate(keep_from_repair);

            let decode = |extras: &[(u32, Vec<u8>)]| {
                let mut decoder = RaptorqDecoder::with_params(params.clone()).unwrap();
                let base = source_indices
                    .iter()
                    .map(|&i| &source_packets[i])
                    .chain(repair_indices.iter().map(|&i| &repair_packets[i]));
                for (seq, chunk) in base.chain(extras.iter()) {
                    if let Some(result) = decoder.decode(*seq, chunk) {
                        return Some(result);
                    }
                }
                None
            };

            let mut decoded = decode(&[]);
            let mut extra_used = 0usize;
            if decoded.is_none() && !spares.is_empty() {
                retries_n1 += 1;
                extra_used = 1;
                decoded = decode(&spares[..1]);
            }
            if decoded.is_none() && spares.len() >= 2 {
                retries_n2 += 1;
                extra_used = 2;
                decoded = decode(&spares[..2]);
            }

            let suffix = match extra_used {
                1 => "+1(retry)",
                2 => "+2(retry)",
                _ => "",
            };
            println!(
                "data_size={:>7}, source_symbols={:>4}, kept {}/{}src + {}/{}rep = {}{}: {}",
                data_size,
                source_count,
                keep_from_source,
                source_packets.len(),
                keep_from_repair,
                repair_packets.len(),
                keep_from_source + keep_from_repair,
                suffix,
                if decoded.is_some() { "OK" } else { "FAILED" },
            );

            assert!(
                decoded.is_some(),
                "data_size={}, source_symbols={}: failed to decode even with N+2 symbols",
                data_size,
                source_count,
            );
            assert_eq!(
                decoded.unwrap(),
                data,
                "data_size={}, source_symbols={}: decoded data mismatch",
                data_size,
                source_count,
            );
            data_size += rng.gen_range(4000..6000);
        }

        assert!(
            retries_n1 * 20 <= steps,
            "Too many N+1 retries: {} / {} steps ({:.1}%)",
            retries_n1,
            steps,
            retries_n1 as f64 * 100.0 / steps as f64,
        );
        assert!(
            retries_n2 * 100 <= steps,
            "Too many N+2 retries: {} / {} steps ({:.1}%)",
            retries_n2,
            steps,
            retries_n2 as f64 * 100.0 / steps as f64,
        );
    }

    println!("--- symbol=None (alignment=8) ---");
    run(None);
    println!("--- symbol=Some(771) (alignment=1) ---");
    run(Some(771));
}

/// Test that RaptorQ encode/decode works with symbol_size > 65535 (u16 limit).
/// This matches the C++ behaviour where symbol_size is size_t.
/// Simulates TwostepFec for 800KB data with 10 parties:
///   k = (10*2-2)/3 = 6, part_size = ceil(819200/6) = 136534
#[test]
fn test_raptorq_large_symbol_size() {
    use adnl::{RaptorqDecoder, RaptorqEncoder};
    use rand::Rng;

    const DATA_SIZE: usize = 800 * 1024;

    for num_parties in [5u32, 10, 20] {
        let k = ((num_parties as usize) * 2 - 2) / 3;
        let part_size = (DATA_SIZE + k - 1) / k;

        println!(
            "--- parties={num_parties}, k={k}, part_size={part_size} (>65535: {}) ---",
            part_size > 65535
        );

        // Generate random data
        let mut rng = rand::thread_rng();
        let data: Vec<u8> = (0..DATA_SIZE).map(|_| rng.gen()).collect();

        // Encode with large symbol_size
        let mut encoder = RaptorqEncoder::with_data(&data, Some(part_size as u32));
        let params = encoder.params().clone();
        println!(
            "  params: data_size={}, symbol_size={}, symbols_count={}",
            params.data_size, params.symbol_size, params.symbols_count
        );
        assert_eq!(params.data_size, DATA_SIZE as i32);
        assert_eq!(params.symbol_size, part_size as i32);

        // Collect source + repair symbols
        let source_count = params.symbols_count as usize;
        let mut packets: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut seqno = 0u32;
        for _ in 0..(source_count * 3 / 2) {
            let chunk = encoder.encode(&mut seqno).unwrap();
            packets.push((seqno, chunk));
            seqno += 1;
        }
        assert!(
            packets.len() >= source_count,
            "Not enough packets generated: {} < {source_count}",
            packets.len()
        );

        // Decode using exactly source_count symbols (minimum required)
        let mut decoder = RaptorqDecoder::with_params(params.clone())
            .expect("decoder creation must succeed for large symbol_size");

        let mut decoded = None;
        for (seq, chunk) in packets.iter().take(source_count + 2) {
            if let Some(result) = decoder.decode(*seq, chunk) {
                decoded = Some(result);
                break;
            }
        }

        let result = decoded.expect("decode must succeed");
        assert_eq!(result.len(), DATA_SIZE, "decoded size mismatch");
        assert_eq!(result, data, "decoded data mismatch");
        println!("  OK: encode/decode verified for symbol_size={part_size}");
    }
}

/// Verify that the RaptorQ encoder produces valid FEC symbols for large
/// part_size values (>65535) that match the C++ TwostepFec behaviour.
#[test]
fn test_twostep_fec_encoder_large_symbols() {
    let data = vec![0x42u8; 800 * 1024];
    for neighbours in [5u32, 10, 20] {
        let k = ((neighbours as usize) * 2 - 2) / 3;
        let part_size = (data.len() + k - 1) / k;

        let mut encoder = RaptorqEncoder::with_data(&data, Some(part_size as u32));
        let params = encoder.params().clone();
        assert_eq!(params.data_size, data.len() as i32);
        assert_eq!(params.symbol_size, part_size as i32);

        // Generate one symbol per neighbour (like broadcast-twostep.cpp)
        let mut seqno = 0u32;
        for _ in 0..neighbours {
            let chunk = encoder.encode(&mut seqno).unwrap();
            assert_eq!(chunk.len(), part_size, "symbol size mismatch");
        }
    }
}
