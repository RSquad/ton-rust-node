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
// TO REMOVE AFTER FULL REBRANDING
extern crate adnl as dht;
extern crate adnl as overlay;
extern crate adnl as rldp;

use adnl::{
    common::{
        AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice, TaggedByteVec,
    },
    node::{AdnlNode, IpAddress},
};
use dht::{DhtSearchPolicy, OverlayNodesSearchContext};
use overlay::OverlayUtils;
use rldp::RldpNode;
#[cfg(feature = "debug")]
use rldp::{Chunk, LossFn};
#[cfg(feature = "debug")]
use std::sync::atomic::{AtomicU32, Ordering};
use std::{convert::TryInto, io::Write, path::Path, sync::Arc, time::Instant};
use ton_api::{
    serialize_boxed, serialize_boxed_append,
    ton::{
        overlay::node::Node as NodeOverlay,
        rpc::{overlay::Query as OverlayQuery, ton_node::DownloadKeyBlockProof},
    },
    BoxedSerialize, TLObject,
};
use ton_block::{
    base64_decode, read_boc, BlockIdExt, BlockProof, Deserializable, KeyId, KeyOption, Result,
    ShardIdent, UInt256,
};

#[path = "./test_utils.rs"]
mod test_utils;
use test_utils::{
    find_overlay_peer, get_adnl_config, init_compatibility_test, init_test, TestContext,
};

const KEY_TAG: usize = 0;
const TARGET: &str = "rldp";
const CONFIG_TESTNET_FILE: &str = "tests/config/testnet.json";
const ZEROSTATE_FILE_HASH: &str = "XplPz01CXAps5qeSWUtxcyBfdAo5zVb1N979KLSKD24=";
//const ZEROSTATE_ROOT_HASH: &str = "F6OpKZKqvqeFp6CQmFomXNMfMj2EnaUSOXN+Mh+wVWk=";

fn init_rldp_compatibility_test(
    local_ip_template: &str,
) -> (TestContext, Arc<AdnlNode>, Arc<RldpNode>) {
    let ctx_test = init_compatibility_test(
        local_ip_template,
        4190,
        "rldp",
        KEY_TAG,
        KEY_TAG,
        ZEROSTATE_FILE_HASH,
        CONFIG_TESTNET_FILE,
        true,
        false,
        #[cfg(feature = "dump")]
        None,
    );
    let ours_ip = format!("{}", ctx_test.adnl.ip_address_adnl());
    let pos = ours_ip.find(":").unwrap();
    let ours_ip = format!("{}:{}", &ours_ip[..pos], ctx_test.adnl.ip_address_adnl().port() + 1);
    let config =
        ctx_test.rt.block_on(get_adnl_config("rldp", &ours_ip, vec![KEY_TAG], true)).unwrap();
    let adnl = ctx_test.rt.block_on(AdnlNode::with_config(config)).unwrap();
    let rldp = RldpNode::with_params(
        adnl.clone(),
        vec![],
        None,
        #[cfg(feature = "debug")]
        None,
        #[cfg(feature = "debug")]
        None,
    )
    .unwrap();
    ctx_test.rt.block_on(adnl.start_over_udp(vec![rldp.clone()])).unwrap();
    (ctx_test, adnl, rldp)
}

fn find_rldp_peer(
    adnl: &Arc<AdnlNode>,
    overlay_peers: &mut Vec<(IpAddress, NodeOverlay)>,
    ctx_search: &mut OverlayNodesSearchContext,
    ctx_test: &mut TestContext,
) -> (Arc<KeyId>, Arc<KeyId>) {
    let (peer_ip, peer_node) = find_overlay_peer(overlay_peers, ctx_search, ctx_test, TARGET);
    let ours_id = adnl.key_by_tag(KEY_TAG).unwrap().id().clone();
    let peer_key: Arc<dyn KeyOption> = (&peer_node.id).try_into().unwrap();
    let peer_id = adnl.add_peer(&ours_id, &peer_ip, None, &peer_key).unwrap().unwrap();
    (peer_id, ours_id)
}

struct Mockup {
    pong: Arc<tokio::sync::Barrier>,
    reply: Vec<u8>,
}

impl Mockup {
    const OVERLAY_ID: [u8; 32] = [0xAAu8; 32];

    fn new(pong: Arc<tokio::sync::Barrier>) -> Self {
        Self { pong, reply: Self::build_reply() }
    }

    fn build_query() -> Vec<u8> {
        let msg = OverlayQuery { overlay: UInt256::with_array(Self::OVERLAY_ID) };
        serialize_boxed(&msg).unwrap()
    }

    fn build_reply() -> Vec<u8> {
        let max = 512 * 1024;
        let mut reply = Vec::with_capacity(max);
        for i in 0..max {
            reply.push(i as u8)
        }
        reply
    }
}

#[async_trait::async_trait]
impl Subscriber for Mockup {
    async fn try_consume_custom(&self, data: &[u8], _peers: &AdnlPeers) -> Result<bool> {
        assert_eq!(&self.reply, data);
        self.pong.wait().await;
        Ok(true)
    }
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        match object.downcast::<OverlayQuery>() {
            Ok(msg) => {
                assert_eq!(*msg.overlay.as_slice(), Self::OVERLAY_ID);
                let reply = TaggedByteVec {
                    object: self.reply.clone(),
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                };
                Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Raw(reply)))))
            }
            Err(object) => Ok(QueryResult::Rejected(object)),
        }
    }
}

struct RldpContext {
    adnl: Arc<AdnlNode>,
    peer: Arc<KeyId>,
    rldp: Arc<RldpNode>,
}

fn init_local_test(
    pong: Arc<tokio::sync::Barrier>,
    #[cfg(feature = "debug")] loss_fn: Option<LossFn>,
    #[cfg(feature = "debug")] min_timeout_ms: Option<u64>,
) -> (tokio::runtime::Runtime, RldpContext, RldpContext) {
    let rt = init_test();
    let config1 =
        rt.block_on(get_adnl_config("rldp", "127.0.0.1:5191", vec![KEY_TAG], true)).unwrap();
    let config2 =
        rt.block_on(get_adnl_config("rldp", "127.0.0.1:5192", vec![KEY_TAG], true)).unwrap();
    let adnl1 = rt.block_on(AdnlNode::with_config(config1)).unwrap();
    let rldp1 = RldpNode::with_params(
        adnl1.clone(),
        vec![Arc::new(Mockup::new(pong.clone()))],
        None,
        #[cfg(feature = "debug")]
        loss_fn,
        #[cfg(feature = "debug")]
        min_timeout_ms,
    )
    .unwrap();
    rt.block_on(adnl1.start_over_udp(vec![rldp1.clone()])).unwrap();
    let adnl2 = rt.block_on(AdnlNode::with_config(config2)).unwrap();
    let rldp2 = RldpNode::with_params(
        adnl2.clone(),
        vec![Arc::new(Mockup::new(pong))],
        None,
        #[cfg(feature = "debug")]
        loss_fn,
        #[cfg(feature = "debug")]
        min_timeout_ms,
    )
    .unwrap();
    rt.block_on(adnl2.start_over_udp(vec![rldp2.clone()])).unwrap();
    let peer1 = adnl1.key_by_tag(KEY_TAG).unwrap();
    let peer2 = adnl2.key_by_tag(KEY_TAG).unwrap();
    adnl1.add_peer(peer1.id(), adnl2.ip_address_adnl(), None, &peer2).unwrap();
    adnl2.add_peer(peer2.id(), adnl1.ip_address_adnl(), None, &peer1).unwrap();
    let ctx1 = RldpContext { adnl: adnl1, peer: peer1.id().clone(), rldp: rldp1 };
    let ctx2 = RldpContext { adnl: adnl2, peer: peer2.id().clone(), rldp: rldp2 };
    (rt, ctx1, ctx2)
}

async fn download_by_block_id<T: BoxedSerialize>(
    rldp: &Arc<RldpNode>,
    peer: &Arc<KeyId>,
    ours: &Arc<KeyId>,
    root: &str,
    file: &str,
    seqno: i32,
    prefix: &[u8],
    v2: bool,
    callback: impl Fn(BlockIdExt) -> (T, String),
) -> bool {
    let root_hash = base64_decode(root).unwrap();
    let file_hash = base64_decode(file).unwrap();
    let file_hash: [u8; 32] = file_hash.try_into().unwrap();
    let mut query = prefix.to_vec();
    let block = BlockIdExt {
        shard_id: ShardIdent::with_tagged_prefix(-1, 0x8000000000000000u64 as i64 as u64).unwrap(),
        seq_no: seqno as u32,
        root_hash: UInt256::with_array(root_hash.try_into().unwrap()),
        file_hash: UInt256::with_array(file_hash.clone()),
    };
    let (message, title) = callback(block.clone());
    serialize_boxed_append(&mut query, &message).unwrap();
    send_rldp_query(rldp, peer, ours, &query, &title, Some(block), v2).await
}

async fn send_rldp_query(
    rldp: &Arc<RldpNode>,
    dst: &Arc<KeyId>,
    src: &Arc<KeyId>,
    query: &[u8],
    msg: &str,
    id: Option<BlockIdExt>,
    v2: bool,
) -> bool {
    const ATTEMPTS: u32 = 3;
    let mut i: u32 = 0;
    let now = Instant::now();
    for _ in 0..ATTEMPTS {
        i += 1;
        log::info!(target:TARGET, "{}, attempt {}", msg, i);
        let peers = AdnlPeers::with_keys(src.clone(), dst.clone());
        if let (Some(answer), _) = rldp
            .query(
                &TaggedByteSlice {
                    object: query,
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                },
                None,
                &peers,
                v2,
                None,
            )
            .await
            .unwrap()
        {
            log::info!(target:TARGET, "Totally received {} bytes", answer.len());
            let path = Path::new("../target").join(msg);
            let mut file = std::fs::File::create(path).unwrap();
            file.write_all(&answer[..]).unwrap();
            let root = read_boc(answer).unwrap().withdraw_single_root().unwrap();
            if let Some(id) = &id {
                let proof = BlockProof::construct_from_cell(root).unwrap();
                assert_eq!(&proof.proof_for, id);
            }
            break;
        }
    }
    if i < ATTEMPTS {
        log::info!(
            target:TARGET,
            "RLDP {} elapsed {}ms",
            if v2 {
                "v2"
            } else {
                "v1"
            },
            now.elapsed().as_millis()
        );
        true
    } else {
        false
    }
}

#[test]
fn rldp_compatibility() {
    let (mut ctx_test, adnl, rldp) = init_rldp_compatibility_test("0.0.0.0:1");
    let mut overlay_peers = Vec::new();
    let mut ctx_search = OverlayNodesSearchContext::with_params(
        &ctx_test.overlay_id,
        DhtSearchPolicy::FastSearch(5),
    )
    .unwrap();

    loop {
        let (peer, ours) =
            find_rldp_peer(&adnl, &mut overlay_peers, &mut ctx_search, &mut ctx_test);

        /*
            let ours = adnl.key_by_tag(KEY_TAG).unwrap().id().clone();
            let key = base64_decode("WtYCFK04u/rfV8KEnMgRPUzQNVnOoV+7SFTByrhIFwM=").unwrap();
            let key: [u8; 32] = key[..32].try_into().unwrap();
            let peer_key: Arc<dyn KeyOption> = ton_block::Ed25519KeyOption::from_public_key(&key);
            let ip = IpAddress::from_versioned_string("91.134.31.11:30000", None).unwrap();
            let peer = adnl.add_peer(&ours, &ip, &peer_key).unwrap().unwrap();
        */
        /*
            let key = hex::decode("9668c407a317ae95d07c76506c7d576bbcfed539d69d0d3f0b7da140c2d29925").unwrap();
            let key: [u8; 32] = key[..32].try_into().unwrap();
            let peer_key: Arc<dyn KeyOption> = ton_block::Ed25519KeyOption::from_private_key(&key).unwrap();
            let ip = IpAddress::from_versioned_string("31.222.227.12:4091", None).unwrap();
            let peer = adnl.add_peer(&ours, &ip, &peer_key).unwrap().unwrap();
        */

        let rldp = rldp.clone();
        let ok = ctx_test.rt.block_on(async move {
            // Overlay ID
            let file_hash = base64_decode(ZEROSTATE_FILE_HASH).unwrap();
            let file_hash = file_hash.as_slice().try_into().unwrap();
            let overlay_id =
                OverlayUtils::calc_overlay_short_id(-1, 0x8000000000000000u64 as i64, file_hash)
                    .unwrap();
            let query = OverlayQuery { overlay: UInt256::with_array(overlay_id.data().clone()) };
            let query = serialize_boxed(&query).unwrap();

            /*
                // Zerostate
                if !download_by_block_id(
                    &rldp,
                    &peer,
                    &ours,
                    ZEROSTATE_ROOT_HASH,
                    ZEROSTATE_FILE_HASH,
                    0,
                    &query,
                    |block| (DownloadZeroState{ block }, "MC Zerostate".to_string())
                ).await {
                    return false
                }
            */

            // Key (init) block proof
            for v2 in [false, true] {
                let ver = if v2 { "v2" } else { "v1" };
                if !download_by_block_id(
                    &rldp,
                    &peer,
                    &ours,
                    "ekhXUomsiq86Psre5pnGObmGjCyl2bdq68O9L9QXkD4=",
                    "Oud5fGjUgGDzTcNTARl9cS7Bfuris1EHdz/WenzVd14=",
                    44818481,
                    &query,
                    v2,
                    |block| {
                        let title = format!("RLDP {} MC keyblock {} proof", ver, block.seq_no);
                        (DownloadKeyBlockProof { block }, title)
                    },
                )
                .await
                {
                    return false;
                }
            }

            true
        });

        if ok {
            break;
        }
    }

    ctx_test.rt.block_on(async move {
        adnl.stop().await;
        ctx_test.adnl.stop().await;
    })
}

fn test_rldp_session_with_loss(
    #[cfg(feature = "debug")] loss_fn: Option<LossFn>,
    #[cfg(feature = "debug")] min_timeout_ms: Option<u64>,
) {
    let ping = Arc::new(tokio::sync::Barrier::new(2));
    let (rt, ctx1, ctx2) = init_local_test(
        ping.clone(),
        #[cfg(feature = "debug")]
        loss_fn,
        #[cfg(feature = "debug")]
        min_timeout_ms,
    );
    rt.block_on(async move {
        // Extend inbound RLDP cap for test
        ctx1.rldp.change_inbound_cap_for_peers(&[ctx2.peer.clone()], 1).unwrap();
        ctx2.rldp.change_inbound_cap_for_peers(&[ctx1.peer.clone()], 1).unwrap();

        let data_send = Mockup::build_query();
        let data_recv = Mockup::build_reply();
        let max_answer_size = Some(513 * 1024);
        let data_query = TaggedByteSlice {
            object: data_send.as_slice(),
            #[cfg(feature = "telemetry")]
            tag: 0,
        };
        let data_message = TaggedByteSlice {
            object: data_recv.as_slice(),
            #[cfg(feature = "telemetry")]
            tag: 0,
        };
        let peers = AdnlPeers::with_keys(ctx1.peer.clone(), ctx2.peer.clone());
        let (data, _) =
            ctx1.rldp.query(&data_query, max_answer_size, &peers, false, None).await.unwrap();
        assert_eq!(data.as_ref().unwrap(), &data_recv);
        let (data, _) =
            ctx1.rldp.query(&data_query, max_answer_size, &peers, true, None).await.unwrap();
        assert_eq!(data.as_ref().unwrap(), &data_recv);
        ctx1.rldp.message(&data_message, &peers, false, None).await.unwrap();
        ping.wait().await;
        ctx1.rldp.message(&data_message, &peers, true, None).await.unwrap();
        ping.wait().await;
        let peers = AdnlPeers::with_keys(ctx2.peer.clone(), ctx1.peer.clone());
        let (data, _) =
            ctx2.rldp.query(&data_query, max_answer_size, &peers, false, None).await.unwrap();
        assert_eq!(data.as_ref().unwrap(), &data_recv);
        let (data, _) =
            ctx2.rldp.query(&data_query, max_answer_size, &peers, true, None).await.unwrap();
        assert_eq!(data.as_ref().unwrap(), &data_recv);
        ctx2.rldp.message(&data_message, &peers, false, None).await.unwrap();
        ping.wait().await;
        ctx2.rldp.message(&data_message, &peers, true, None).await.unwrap();
        ping.wait().await;
        ctx1.adnl.stop().await;
        ctx2.adnl.stop().await;
    })
}

#[test]
fn test_rldp_session() {
    #[cfg(feature = "debug")]
    fn loss_fn(_chunk: &Chunk) -> bool {
        static CNT: AtomicU32 = AtomicU32::new(0);
        let cnt = CNT.fetch_add(1, Ordering::Relaxed);
        (cnt % 2) != 0 // 50% loss
    }
    test_rldp_session_with_loss(
        #[cfg(feature = "debug")]
        None,
        #[cfg(feature = "debug")]
        None,
    );
    #[cfg(feature = "debug")]
    test_rldp_session_with_loss(Some(loss_fn), Some(1000));
}
