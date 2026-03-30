/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "debug")]
use adnl::rldp::{Chunk, LossFn};
use adnl::{
    common::{
        AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice, TaggedByteVec,
    },
    node::AdnlNode,
    RldpNode,
};
use rand::Rng;
#[cfg(feature = "debug")]
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
#[cfg(not(feature = "debug"))]
use std::time::Instant;
use ton_api::{
    serialize_bare,
    ton::{testobject::TestInt, TestObject},
    AnyBoxedSerialize, TLObject,
};
use ton_block::{crc32_digest, KeyOption};

include!("../../common/src/config.rs");
include!("../../common/src/test.rs");

const KEY_1: usize = 0;
const KEY_2: usize = 1;

// RLDP query mockup
struct Mockup;

impl Mockup {
    fn build_query(l: usize) -> Vec<u8> {
        serialize_bare(&TestInt { value: l as i32 }).unwrap()
    }

    fn build_reply(l: usize) -> Vec<u8> {
        let mut reply = vec![0u8; l];
        if l > 4 {
            rand::thread_rng().fill(&mut reply[..l - 4]);
            let crc = crc32_digest(&reply[..l - 4]);
            reply[l - 4] = (crc >> 24) as u8;
            reply[l - 3] = (crc >> 16) as u8;
            reply[l - 2] = (crc >> 8) as u8;
            reply[l - 1] = (crc >> 0) as u8;
        } else {
            rand::thread_rng().fill(&mut reply[..]);
        }
        reply
    }
}

#[async_trait::async_trait]
impl Subscriber for Mockup {
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        match object.downcast::<TestObject>() {
            Ok(TestObject::TestInt(object)) => {
                let reply = TaggedByteVec {
                    object: Self::build_reply(object.value as usize),
                    #[cfg(feature = "telemetry")]
                    tag: 0,
                };
                Ok(QueryResult::Consumed(QueryAnswer::Ready(Some(Answer::Raw(reply)))))
            }
            Ok(object) => Ok(QueryResult::Rejected(object.into_tl_object())),
            Err(object) => Ok(QueryResult::Rejected(object)),
        }
    }
}

async fn bench_scenario(
    peers: AdnlPeers,
    v2: bool,
    rldp1: Arc<RldpNode>,
    #[cfg(feature = "debug")] rldp2: Arc<RldpNode>,
) {
    let sizes = vec![1, 1024, 1 << 20, 2 << 20, 3 << 20, 10 << 20, 16 << 20];
    //let sizes = vec![16 << 20];
    for size in sizes {
        print!("Testing delivering data of size {:<9}...", size);
        #[cfg(not(feature = "debug"))]
        let start = Arc::new(Instant::now());
        #[cfg(feature = "debug")]
        rldp1.reset_timestamp();
        #[cfg(feature = "debug")]
        rldp2.reset_timestamp();
        let data = TaggedByteSlice {
            object: &Mockup::build_query(size),
            #[cfg(feature = "telemetry")]
            tag: 0,
        };
        let res = rldp1.query(&data, Some(size as u64 + 1024), &peers, v2, None).await.unwrap();
        let (Some(reply), _) = res else {
            println!(" failed: empty response");
            break;
        };
        let l = reply.len();
        if l > 4 {
            let crc1 = crc32_digest(&reply[..l - 4]);
            let crc2 = ((reply[l - 4] as u32) << 24)
                | ((reply[l - 3] as u32) << 16)
                | ((reply[l - 2] as u32) << 8)
                | ((reply[l - 1] as u32) << 0);
            if crc1 != crc2 {
                println!(" failed: CRC mismatch");
                break;
            }
        }
        #[cfg(not(feature = "debug"))]
        println!(" success. Time = {}s", (start.elapsed().as_micros() as f32) / 1000000.0);
        #[cfg(feature = "debug")]
        rldp1.check_time(" success");
    }
}

fn create_adnl_node(port: &str) -> (tokio::runtime::Runtime, Arc<AdnlNode>) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = rt
        .block_on(get_adnl_config(
            "rldp",
            format!("127.0.0.1:{}", port).as_str(),
            vec![KEY_1, KEY_2],
            true,
        ))
        .unwrap();
    let adnl = rt.block_on(AdnlNode::with_config(config)).unwrap();
    (rt, adnl)
}

fn create_rldp_node(
    adnl: &Arc<AdnlNode>,
    key_tag: usize,
    #[cfg(feature = "debug")] loss_percentage: Option<u8>,
) -> (Arc<RldpNode>, Arc<dyn KeyOption>) {
    #[cfg(feature = "debug")]
    static LOSS_PERCENTAGE: AtomicU8 = AtomicU8::new(0);

    #[cfg(feature = "debug")]
    fn loss_function(_chunk: &Chunk) -> bool {
        static CNT: AtomicU32 = AtomicU32::new(0);
        let cnt = CNT.fetch_add(1, Ordering::Relaxed);
        let loss = LOSS_PERCENTAGE.load(Ordering::Relaxed);
        if loss > 0 {
            (cnt % (100u32 / (loss as u32))) == 0
        } else {
            false
        }
    }

    #[cfg(feature = "debug")]
    let (loss_fn, timeout) = loss_percentage.map_or((None, None), |loss_percentage| {
        LOSS_PERCENTAGE.store(loss_percentage, Ordering::Relaxed);
        let loss_fn = Some(loss_function as LossFn);
        (loss_fn, Some(1000))
    });

    let key = adnl.key_by_tag(key_tag).unwrap();
    let rldp = RldpNode::with_params(
        adnl.clone(),
        vec![Arc::new(Mockup)],
        Some(key.id().clone()),
        #[cfg(feature = "debug")]
        loss_fn,
        #[cfg(feature = "debug")]
        timeout,
    )
    .unwrap();
    (rldp, key)
}

fn bench_rldp(loopback: bool, v2: bool, #[cfg(feature = "debug")] loss_percentage: Option<u8>) {
    //init_test_log();
    let (rt1, adnl1) = create_adnl_node(if loopback { "5190" } else { "5191" });
    let (rldp1, key1) = create_rldp_node(
        &adnl1,
        KEY_1,
        #[cfg(feature = "debug")]
        loss_percentage,
    );
    let (rt2, adnl2) = if loopback {
        (None, adnl1.clone())
    } else {
        let (rt2, adnl2) = create_adnl_node("5192");
        (Some(rt2), adnl2)
    };
    let (rldp2, key2) = create_rldp_node(
        &adnl2,
        KEY_2,
        #[cfg(feature = "debug")]
        loss_percentage,
    );
    adnl1.add_peer(key1.id(), adnl2.ip_address(), &key2).unwrap();
    adnl2.add_peer(key2.id(), adnl1.ip_address(), &key1).unwrap();
    if let Some(rt2) = &rt2 {
        rt1.block_on(adnl1.start_over_udp(vec![rldp1.clone()])).unwrap();
        rt2.block_on(adnl2.start_over_udp(vec![rldp2.clone()])).unwrap();
    } else {
        rt1.block_on(adnl1.start_over_udp(vec![rldp1.clone(), rldp2.clone()])).unwrap();
    }
    let peers = AdnlPeers::with_keys(key1.id().clone(), key2.id().clone());

    let msg = if loopback { "loopback" } else { "network" };
    #[cfg(feature = "debug")]
    let msg = loss_percentage
        .map_or(msg.into(), |loss_percentage| format!("{msg}, loss percentage {loss_percentage}%"));
    println!(
        "\nSending RLDP {} query {} -> {} via {msg}",
        if v2 { "V2" } else { "V1" },
        key1.id(),
        key2.id()
    );
    let run = rt1.spawn(async move {
        bench_scenario(
            peers,
            v2,
            rldp1,
            #[cfg(feature = "debug")]
            rldp2,
        )
        .await
    });
    rt1.block_on(async move { run.await }).unwrap();
    rt1.block_on(async move { adnl1.stop().await });
    if let Some(rt2) = rt2 {
        rt2.block_on(async move { adnl2.stop().await });
    }
}

fn main() {
    for loopback in [true, false] {
        for v2 in [false, true] {
            bench_rldp(
                loopback,
                v2,
                #[cfg(feature = "debug")]
                None,
            );
            #[cfg(feature = "debug")]
            bench_rldp(
                loopback,
                v2,
                #[cfg(feature = "debug")]
                Some(30),
            );
        }
    }
}
