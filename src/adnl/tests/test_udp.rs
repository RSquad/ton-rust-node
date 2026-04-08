/*
 * Copyright (C) 2019-2021 TON Labs. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#![cfg(feature = "node")]

use adnl::{
    common::{
        AdnlCryptoUtils, AdnlPeers, Answer, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice,
        TaggedTlObject, TimedAnswer, Version,
    },
    node::{AdnlNode, IpAddress},
};
use std::{
    convert::TryInto,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use ton_api::{
    ton::{
        adnl::{pong::Pong as AdnlPong, Pong as AdnlPongBoxed},
        rpc::adnl::Ping as AdnlPing,
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};

include!("../../common/src/config.rs");
include!("../../common/src/test.rs");

#[cfg(feature = "debug")]
const IP_ADDR1: &str = "127.0.0.1:5191";
#[cfg(not(feature = "debug"))]
const IP_ADDR1: &str = "127.0.0.1:4191";
#[cfg(feature = "debug")]
const IP_ADDR2: &str = "127.0.0.1:5192";
#[cfg(not(feature = "debug"))]
const IP_ADDR2: &str = "127.0.0.1:4192";
const IP_ADDRX: &str = "1.2.3.4:4191";

pub const MOCK_MESSAGE: &[u8] = &[5, 6, 7, 8, 12, 9];
pub const MOCK_BIG_MESSAGE: [u8; 4196] = [0x55; 4196];

pub struct Mockup {
    pong: Arc<tokio::sync::Barrier>,
    drop: Option<Arc<AtomicBool>>,
}

impl Mockup {
    pub fn with_params(pong: Arc<tokio::sync::Barrier>, drop: Option<Arc<AtomicBool>>) -> Self {
        Self { pong, drop }
    }
    fn compare(mock: &[u8], data: &[u8]) -> bool {
        let xor = data[0] ^ mock[0];
        for i in 1..data.len() {
            if mock[i] != data[i] ^ xor {
                return false;
            }
        }
        return true;
    }
}

#[async_trait::async_trait]
impl Subscriber for Mockup {
    async fn try_consume_custom(&self, data: &[u8], _peers: &AdnlPeers) -> Result<bool> {
        if data.len() == MOCK_MESSAGE.len() {
            assert!(Mockup::compare(MOCK_MESSAGE, data))
        } else if data.len() == MOCK_BIG_MESSAGE.len() {
            assert!(Mockup::compare(&MOCK_BIG_MESSAGE, data))
        } else {
            assert!(false)
        }
        self.pong.wait().await;
        Ok(true)
    }
    async fn try_consume_query(&self, object: TLObject, _peers: &AdnlPeers) -> Result<QueryResult> {
        let drop = if let Some(drop) = &self.drop { drop.load(Ordering::Relaxed) } else { false };
        if drop {
            Ok(QueryResult::Consumed(QueryAnswer::Ready(None)))
        } else {
            match object.downcast::<AdnlPing>() {
                Ok(ping) => {
                    if ping.value < 10000 {
                        QueryResult::consume(
                            AdnlPong { value: ping.value },
                            #[cfg(feature = "telemetry")]
                            None,
                        )
                    } else {
                        let handle = tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            let answer = TaggedTlObject {
                                object: AdnlPong { value: ping.value }
                                    .into_boxed()
                                    .into_tl_object(),
                                #[cfg(feature = "telemetry")]
                                tag: 0,
                            };
                            Ok(TimedAnswer {
                                answer: Some(Answer::Object(answer)),
                                #[cfg(feature = "telemetry")]
                                actual_start_at: None,
                            })
                        });
                        Ok(QueryResult::Consumed(QueryAnswer::Pending(handle)))
                    }
                }
                Err(object) => Ok(QueryResult::Rejected(object)),
            }
        }
    }
}

const KEY_TAG: usize = 0;

async fn init_node(
    ip: &str,
    ping: &Arc<tokio::sync::Barrier>,
    drop: Option<Arc<AtomicBool>>,
) -> Arc<AdnlNode> {
    let config = get_adnl_config("adnl", ip, vec![KEY_TAG], true).await.unwrap();
    let node = AdnlNode::with_config(config).await.unwrap();
    node.start_over_udp(vec![Arc::new(Mockup::with_params(ping.clone(), drop))]).await.unwrap();
    node
}

fn get_tagged_slice(slice: &[u8]) -> TaggedByteSlice<'_> {
    TaggedByteSlice {
        object: slice,
        #[cfg(feature = "telemetry")]
        tag: 0,
    }
}

async fn wait(ping: &Arc<tokio::sync::Barrier>) -> bool {
    tokio::select! {
        _ = ping.wait() => true,
        _ = tokio::time::sleep(Duration::from_secs(2)) => false
    }
}

async fn test_address(
    ip: &str,
    version: i32,
    node1: &Arc<AdnlNode>,
    node2: &Arc<AdnlNode>,
    ping: &Arc<tokio::sync::Barrier>,
    expect_ok: bool,
) {
    let src = node1.key_by_tag(KEY_TAG).unwrap();
    let peer = node1
        .add_peer(
            src.id(),
            &IpAddress::from_versioned_string(ip, Some(version)).unwrap(),
            &node2.key_by_tag(KEY_TAG).unwrap(),
        )
        .unwrap()
        .unwrap();
    if expect_ok {
        let peers = AdnlPeers::with_keys(src.id().clone(), peer);
        node1.send_custom(&get_tagged_slice(&MOCK_MESSAGE), &peers).await.unwrap();
        ping.wait().await;
    } else {
        let node_clone = node1.clone();
        let ping_clone = ping.clone();
        tokio::spawn(async move {
            let peers = AdnlPeers::with_keys(src.id().clone(), peer);
            node_clone.send_custom(&get_tagged_slice(&MOCK_MESSAGE), &peers).await.unwrap();
            ping_clone.wait().await;
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        ping.wait().await;
    }
}

#[test]
fn node_address_change() {
    let rt = init_test();
    let ping = Arc::new(tokio::sync::Barrier::new(2));
    rt.block_on(async move {
        let node1 = init_node(IP_ADDR1, &ping, None).await;
        let node2 = init_node(IP_ADDR2, &ping, None).await;
        let mut version = Version::get();
        // Ensure that channel establishment passed over to avoid correct IP restore
        test_address(IP_ADDR2, version, &node1, &node2, &ping, true).await;
        test_address(IP_ADDR2, version, &node1, &node2, &ping, true).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        for _ in 0..5 {
            version += 1;
            test_address("1.2.3.4:1000", version, &node1, &node2, &ping, false).await;
            version += 1;
            test_address(IP_ADDR2, version, &node1, &node2, &ping, true).await;
        }
        node1.stop().await;
        node2.stop().await;
    })
}

#[test]
fn node_loopback() {
    let rt = init_test();
    let ping = Arc::new(tokio::sync::Barrier::new(2));
    rt.block_on(async move {
        let node = init_node(IP_ADDRX, &ping, None).await;
        let peers = AdnlPeers::with_keys(
            node.key_by_tag(KEY_TAG).unwrap().id().clone(),
            node.key_by_tag(KEY_TAG).unwrap().id().clone(),
        );
        let mut data = Vec::with_capacity(MOCK_MESSAGE.len());
        data.extend_from_slice(MOCK_MESSAGE);
        let data = get_tagged_slice(&data);
        for i in 0..10 {
            let query = AdnlPing { value: i };
            let answer = node
                .clone()
                .query(
                    &TaggedTlObject {
                        object: query.into_tl_object(),
                        #[cfg(feature = "telemetry")]
                        tag: 0,
                    },
                    &peers,
                    None,
                )
                .await
                .unwrap()
                .unwrap();
            let answer = answer.downcast::<AdnlPongBoxed>().unwrap();
            assert_eq!(answer.value(), &i);
            node.send_custom(&data, &peers).await.unwrap();
            assert!(wait(&ping).await);
        }
        node.stop().await;
    })
}

#[test]
fn node_versioning() {
    let id = [0xAA; 32];
    let key = [0xBB; 32];
    let data: Vec<u8> = (1..100).collect();
    assert!(AdnlCryptoUtils::decode_version(
        &data[32..36].try_into().unwrap(),
        &data[..32],
        &data[36..68]
    )
    .is_none());
    let mut buf = Vec::new();
    buf.resize(100, 0xCC);
    AdnlCryptoUtils::encode_header(&mut buf, &id, None, None);
    assert!(AdnlCryptoUtils::decode_version(
        &buf[32..36].try_into().unwrap(),
        &buf[..32],
        &buf[36..68]
    )
    .is_none());
    AdnlCryptoUtils::encode_header(&mut buf, &id, None, Some(0));
    assert_eq!(
        AdnlCryptoUtils::decode_version(&buf[32..36].try_into().unwrap(), &buf[..32], &buf[36..68]),
        Some(0)
    );
    AdnlCryptoUtils::encode_header(&mut buf, &id, Some(&key), None);
    assert!(AdnlCryptoUtils::decode_version(
        &buf[64..68].try_into().unwrap(),
        &buf[..64],
        &buf[68..100]
    )
    .is_none());
    AdnlCryptoUtils::encode_header(&mut buf, &id, Some(&key), Some(0));
    assert_eq!(
        AdnlCryptoUtils::decode_version(
            &buf[64..68].try_into().unwrap(),
            &buf[..64],
            &buf[68..100]
        ),
        Some(0)
    );
}

#[test]
fn node_options() {
    let rt = init_test();
    let ping = Arc::new(tokio::sync::Barrier::new(2));
    rt.block_on(async move {
        let node1 = init_node(IP_ADDR1, &ping, None).await;
        node1.set_options(AdnlNode::OPTION_FORCE_COMPRESSION | AdnlNode::OPTION_FORCE_VERSIONING);
        let node2 = init_node(IP_ADDR2, &ping, None).await;
        node1.set_options(AdnlNode::OPTION_FORCE_COMPRESSION | AdnlNode::OPTION_FORCE_VERSIONING);
        let mut version = Version::get();
        // Ensure that channel establishment passed over to avoid correct IP restore
        test_address(IP_ADDR2, version, &node1, &node2, &ping, true).await;
        version += 1;
        test_address(IP_ADDR2, version, &node1, &node2, &ping, true).await;
        tokio::time::sleep(Duration::from_millis(500)).await;
        for _ in 0..5 {
            version += 1;
            test_address(IP_ADDR2, version, &node1, &node2, &ping, true).await;
        }
        node1.stop().await;
        node2.stop().await;
    })
}

#[test]
fn node_async_query() {
    let rt = init_test();
    let ping = Arc::new(tokio::sync::Barrier::new(2));
    rt.block_on(async move {
        let node1 = init_node(IP_ADDR1, &ping, None).await;
        let node2 = init_node(IP_ADDR2, &ping, None).await;
        let peer1 = node2
            .add_peer(
                node2.key_by_tag(KEY_TAG).unwrap().id(),
                node1.ip_address(),
                &node1.key_by_tag(KEY_TAG).unwrap(),
            )
            .unwrap()
            .unwrap();
        let peer2 = node1
            .add_peer(
                node1.key_by_tag(KEY_TAG).unwrap().id(),
                node2.ip_address(),
                &node2.key_by_tag(KEY_TAG).unwrap(),
            )
            .unwrap()
            .unwrap();
        let peers = AdnlPeers::with_keys(peer1.clone(), peer2.clone());
        for i in 0..5 {
            let query = AdnlPing { value: i * 10000 };
            let answer = node1
                .query(
                    &TaggedTlObject {
                        object: query.into_tl_object(),
                        #[cfg(feature = "telemetry")]
                        tag: 0,
                    },
                    &peers,
                    None,
                )
                .await
                .unwrap()
                .unwrap();
            let answer = answer.downcast::<AdnlPongBoxed>().unwrap();
            assert_eq!(answer.value(), &(i * 10000));
        }
        node1.stop().await;
        node2.stop().await;
    })
}
