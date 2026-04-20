/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use adnl::{
    common::{
        AdnlPeers, Answer, AtomicPair, QueryAnswer, QueryResult, Subscriber, TaggedByteSlice,
        TaggedTlObject, TimedAnswer,
    },
    node::{AdnlNode, AdnlSendMethod, AdnlSendMethodDetailed, AdnlStatus, DataCompression},
    server::{AdnlServerConfig, AdnlServerConfigJson},
};
use std::{
    fmt::{Display, Formatter},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use ton_api::{
    ton::{
        adnl::{pong::Pong as AdnlPong, Pong as AdnlPongBoxed},
        rpc::adnl::Ping as AdnlPing,
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};
use ton_block::{base64_decode, UnixTime};

include!("../../common/src/config.rs");
include!("../../common/src/test.rs");

const CHANNEL_RESET_TIMEOUT_SEC: u8 = 5;
const KEY_TAG: usize = 0;

const MOCK_MESSAGE: &[u8] = &[5, 6, 7, 8, 12, 9];
const MOCK_BIG_MESSAGE: [u8; 131078] = [0x55; 131078];

fn get_tagged_slice(slice: &[u8]) -> TaggedByteSlice<'_> {
    TaggedByteSlice {
        object: slice,
        #[cfg(feature = "telemetry")]
        tag: 0,
    }
}

async fn init_node(
    ip: &str,
    udp_only: bool,
    ping: &Arc<tokio::sync::Barrier>,
    drop: Option<Arc<AtomicBool>>,
) -> Arc<AdnlNode> {
    let config = get_adnl_config("adnl", ip, vec![KEY_TAG], true).await.unwrap();
    let node = AdnlNode::with_config(config).await.unwrap();
    let subscribers: Vec<Arc<dyn Subscriber>> =
        vec![Arc::new(Mockup::with_params(ping.clone(), drop))];
    if udp_only {
        node.start_over_udp(subscribers).await.unwrap();
    } else {
        node.start_over_udp_tcp(subscribers).await.unwrap();
    }
    node.set_channel_reset_timeout(CHANNEL_RESET_TIMEOUT_SEC).await;
    node
}

async fn wait(ping: &Arc<tokio::sync::Barrier>) -> bool {
    tokio::select! {
        _ = ping.wait() => true,
        _ = tokio::time::sleep(Duration::from_secs(2)) => false
    }
}

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

#[test]
fn test_adnl_compression() {
    struct TestValue<'a> {
        uncompressed: &'a str,
        compressed: &'a str,
    }

    const TEST_VECTOR: [TestValue; 3] = [
        // [0xFF; 350]
        TestValue {
            uncompressed: "\
                /////////////////////////////////////////////////////////////////////////\
                /////////////////////////////////////////////////////////////////////////\
                /////////////////////////////////////////////////////////////////////////\
                /////////////////////////////////////////////////////////////////////////\
                /////////////////////////////////////////////////////////////////////////\
                /////////////////////////////////////////////////////////////////////////\
                ////////////////////////////8=\
            ",
            compressed: "H/8BAP9GUP//////",
        },
        // [0x01, 0x02... ; 350]
        TestValue {
            uncompressed: "\
                AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1N\
                jc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG\
                1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2en6ChoqO\
                kpaanqKmqq6ytrq+wsbKztLW2t7i5uru8vb6/wMHCw8TFxsfIycrLzM3Oz9DR0tPU1dbX2Nna\
                29zd3t/g4eLj5OXm5+jp6uvs7e7v8PHy8/T19vf4+fr7/P3+/wABAgMEBQYHCAkKCwwNDg8QE\
                RITFBUWFxgZGhscHR4fICEiIyQlJicoKSorLC0uLzAxMjM0NTY3ODk6Ozw9Pj9AQUJDREVGR0\
                hJSktMTU5PUFFSU1RVVldYWVpbXF0=\
            ",
            compressed: "\
                //EAAQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyAhIiMkJSYnKCkqKywtLi8wMTIzN\
                DU2Nzg5Ojs8PT4/QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl9gYWJjZGVmZ2hpam\
                tsbW5vcHFyc3R1dnd4eXp7fH1+f4CBgoOEhYaHiImKi4yNjo+QkZKTlJWWl5iZmpucnZ6foKG\
                io6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr/AwcLDxMXGx8jJysvMzc7P0NHS09TV1tfY\
                2drb3N3e3+Dh4uPk5ebn6Onq6+zt7u/w8fLz9PX29/j5+vv8/f7/AAFGUFlaW1xd\
            ",
        },
        // [0xFF, 0xFE... ; 350]
        TestValue {
            uncompressed: "\
                //79/Pv6+fj39vX08/Lx8O/u7ezr6uno5+bl5OPi4eDf3t3c29rZ2NfW1dTT0tHQz87NzMvKy\
                cjHxsXEw8LBwL++vby7urm4t7a1tLOysbCvrq2sq6qpqKempaSjoqGgn56dnJuamZiXlpWUk5\
                KRkI+OjYyLiomIh4aFhIOCgYB/fn18e3p5eHd2dXRzcnFwb25tbGtqaWhnZmVkY2JhYF9eXVx\
                bWllYV1ZVVFNSUVBPTk1MS0pJSEdGRURDQkFAPz49PDs6OTg3NjU0MzIxMC8uLSwrKikoJyYl\
                JCMiISAfHh0cGxoZGBcWFRQTEhEQDw4NDAsKCQgHBgUEAwIBAP/+/fz7+vn49/b19PPy8fDv7\
                u3s6+rp6Ofm5eTj4uHg397d3Nva2djX1tXU09LR0M/OzczLysnIx8bFxMPCwcC/vr28u7q5uL\
                e2tbSzsrGwr66trKuqqainpqWko6I=\
            ",
            compressed: "\
                //H//v38+/r5+Pf29fTz8vHw7+7t7Ovq6ejn5uXk4+Lh4N/e3dzb2tnY19bV1NPS0dDPzs3My\
                8rJyMfGxcTDwsHAv769vLu6ubi3trW0s7KxsK+urayrqqmop6alpKOioaCfnp2cm5qZmJeWlZ\
                STkpGQj46NjIuKiYiHhoWEg4KBgH9+fXx7enl4d3Z1dHNycXBvbm1sa2ppaGdmZWRjYmFgX15\
                dXFtaWVhXVlVUU1JRUE9OTUxLSklIR0ZFRENCQUA/Pj08Ozo5ODc2NTQzMjEwLy4tLCsqKSgn\
                JiUkIyIhIB8eHRwbGhkYFxYVFBMSERAPDg0MCwoJCAcGBQQDAgEAAAFGUKalpKOi\
            ",
        },
    ];

    let data = [0xFF; 250];
    let compressed = DataCompression::compress_raw(&data).unwrap();
    let decompressed = DataCompression::decompress_raw(&compressed);
    assert_eq!(&compressed, &data);
    assert!(decompressed.is_none());

    for test in TEST_VECTOR {
        let uncompressed = base64_decode(test.uncompressed).unwrap();
        let expected = base64_decode(test.compressed).unwrap();
        let compressed = DataCompression::compress_raw(&uncompressed).unwrap();
        assert_ne!(&compressed[4..compressed.len() - 1], &uncompressed);
        assert_eq!(&compressed[4..compressed.len() - 1], &expected);
        let decompressed = DataCompression::decompress_raw(&compressed);
        if let Some(decompressed) = decompressed {
            assert_eq!(&decompressed, &uncompressed)
        } else {
            assert!(false)
        }
    }
}

#[test]
fn test_adnl_atomic_pair() {
    let ap = Arc::new(AtomicPair::new(0, 0));
    let mut threads = Vec::new();

    for i in 1..100 {
        let ap = ap.clone();
        let th = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            for _ in 0..10000 {
                if !ap.update(i, i * 2, |_, _| true) {
                    panic!("Failed update")
                }
                let (x, y) = ap.get();
                if x * 2 != y {
                    panic!("Broken")
                }
            }
        });
        threads.push(th)
    }
    for thread in threads {
        thread.join().expect("Join failed")
    }

    let (x, y) = ap.get();
    println!("Final atomic pair {} {}", x, y);
}

#[test]
fn test_adnl_server_config() {
    const SERVER_KEY: &str = r#"{
        "type_id": 1209251014,
        "pvt_key": "cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk="
    }"#;
    const CLIENT_KEY_1: &str = r#"{
        "type_id": 1209251014,
        "pub_key": "RYokIiD5AFkzfTBgC6NhtAGFKm0+gwhN4suTzaW0Sjw="
    }"#;
    const CLIENT_KEY_2: &str = r#"{
        "type_id": 1209251014,
        "pub_key": "cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk="
    }"#;
    const ADNL_SERVER_CONFIGS: [&str; 4] = [
        "{\
            \"address\":\"127.0.0.1:3000\",\
            \"clients\":{\
                \"list\":[\
                    {\
                        \"type_id\":1209251014,\
                        \"pub_key\":\"RYokIiD5AFkzfTBgC6NhtAGFKm0+gwhN4suTzaW0Sjw=\"\
                    },\
                    {\
                        \"type_id\":1209251014,\
                        \"pub_key\":\"cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk=\"\
                    }\
                ]\
            },\
            \"server_key\":{\
                \"type_id\":1209251014,\
                \"pvt_key\":\"cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk=\"\
            }\
        }",
        "{\
            \"address\":\"127.0.0.1:3000\",\
            \"clients\":{\
                \"list\":[\
                    {\
                        \"type_id\":1209251014,\
                        \"pub_key\":\"RYokIiD5AFkzfTBgC6NhtAGFKm0+gwhN4suTzaW0Sjw=\"\
                    }\
                ]\
            },\
            \"server_key\":{\
                \"type_id\":1209251014,\
                \"pvt_key\":\"cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk=\"\
            }\
        }",
        "{\
            \"address\":\"127.0.0.1:3000\",\
            \"clients\":{\
                \"list\":[\
                ]\
            },\
            \"server_key\":{\
                \"type_id\":1209251014,\
                \"pvt_key\":\"cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk=\"\
            }\
        }",
        "{\
            \"address\":\"127.0.0.1:3000\",\
            \"server_key\":{\
                \"type_id\":1209251014,\
                \"pvt_key\":\"cJIxGZviebMQWL726DRejqVzRTSXPv/1sO/ab6XOZXk=\"\
            }\
        }",
    ];

    let configs = [
        AdnlServerConfigJson::with_params(
            "127.0.0.1:3000".to_string(),
            serde_json::from_str(SERVER_KEY).unwrap(),
            Some(vec![
                serde_json::from_str(CLIENT_KEY_1).unwrap(),
                serde_json::from_str(CLIENT_KEY_2).unwrap(),
            ]),
            None,
        ),
        AdnlServerConfigJson::with_params(
            "127.0.0.1:3000".to_string(),
            serde_json::from_str(SERVER_KEY).unwrap(),
            Some(vec![serde_json::from_str(CLIENT_KEY_1).unwrap()]),
            None,
        ),
        AdnlServerConfigJson::with_params(
            "127.0.0.1:3000".to_string(),
            serde_json::from_str(SERVER_KEY).unwrap(),
            Some(vec![]),
            None,
        ),
        AdnlServerConfigJson::with_params(
            "127.0.0.1:3000".to_string(),
            serde_json::from_str(SERVER_KEY).unwrap(),
            None,
            None,
        ),
    ];
    for i in 0..configs.len() {
        let config_json = ADNL_SERVER_CONFIGS[i];
        AdnlServerConfig::from_json(config_json).unwrap();
        AdnlServerConfig::from_json_config(&configs[i]).unwrap();
        assert_eq!(config_json, serde_json::to_string(&configs[i]).unwrap().as_str());
    }
}

#[derive(Clone, PartialEq)]
enum SendMode {
    Fast,
    Mixed,
    Safe,
}

impl Display for SendMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SendMode::Fast => write!(f, "fast"),
            SendMode::Mixed => write!(f, "mixed"),
            SendMode::Safe => write!(f, "safe"),
        }
    }
}

fn run_adnl_session(mode: SendMode, ip1: &str, ip2: &str) {
    const ATTEMPTS: i64 = 100;

    fn now() -> u64 {
        UnixTime::now()
    }

    fn check_status(
        status: &AdnlStatus,
        expected_method: AdnlSendMethodDetailed,
        reset_timeout: Option<u64>,
        mode: &SendMode,
        phase: &str,
    ) {
        assert!(
            status.method == expected_method,
            "unexpected send method {} vs. {expected_method} in {mode} test mode, {phase}",
            status.method
        );
        if let Some(reset_at) = status.reset_at {
            if let Some(reset_timeout) = reset_timeout {
                let expected = now() + reset_timeout;
                assert!(
                    (reset_at + 1) >= expected, // current second tolerance
                    "unexpected reset time {reset_at} vs. {expected} in {mode} test mode, {phase}"
                );
            } else {
                assert!(
                    reset_at == 0,
                    "unexpected non-zero reset time {reset_at} in {mode} test mode, {phase}"
                );
            };
        }
    }

    let start = Instant::now();
    let packet_size = loop {
        let rt = init_test();
        let ping = Arc::new(tokio::sync::Barrier::new(2));
        let drop1 = Arc::new(AtomicBool::new(false));
        let drop2 = Arc::new(AtomicBool::new(false));
        let mode = mode.clone();
        let udp_only = mode == SendMode::Fast;
        let packet_size = rt.block_on(async move {
            let node1 = init_node(ip1, udp_only, &ping, Some(drop1.clone())).await;
            let node2 = init_node(ip2, udp_only, &ping, Some(drop2.clone())).await;
            let reset_timeout1 = node1.get_channel_reset_timeout();
            let reset_timeout2 = node2.get_channel_reset_timeout();
            let peer1 = node2
                .add_peer(
                    node2.key_by_tag(KEY_TAG).unwrap().id(),
                    node1.ip_address_adnl(),
                    None,
                    &node1.key_by_tag(KEY_TAG).unwrap(),
                )
                .unwrap()
                .unwrap();
            let peer2 = node1
                .add_peer(
                    node1.key_by_tag(KEY_TAG).unwrap().id(),
                    node2.ip_address_adnl(),
                    None,
                    &node2.key_by_tag(KEY_TAG).unwrap(),
                )
                .unwrap()
                .unwrap();
            let peers1 = AdnlPeers::with_keys(peer1.clone(), peer2.clone());
            let peers2 = AdnlPeers::with_keys(peer2.clone(), peer1.clone());
            for p in 0..2 {
                for n in 0..ATTEMPTS {
                    for k in 0..2 {
                        let mock = if k == 0 { MOCK_MESSAGE } else { &MOCK_BIG_MESSAGE };
                        let mut data = Vec::with_capacity(mock.len());
                        data.extend_from_slice(mock);
                        for item in data.iter_mut() {
                            *item ^= n as u8
                        }
                        let (send_method1, method1_sent, send_method2, method2_sent) = match mode {
                            SendMode::Fast => (
                                AdnlSendMethod::Fast,
                                AdnlSendMethodDetailed::FastNormal,
                                AdnlSendMethod::Fast,
                                AdnlSendMethodDetailed::FastNormal,
                            ),
                            SendMode::Mixed => {
                                if (n & 0x01) == 1 {
                                    (
                                        AdnlSendMethod::Safe,
                                        AdnlSendMethodDetailed::Safe,
                                        AdnlSendMethod::Fast,
                                        AdnlSendMethodDetailed::FastNormal,
                                    )
                                } else {
                                    (
                                        AdnlSendMethod::Fast,
                                        AdnlSendMethodDetailed::FastNormal,
                                        AdnlSendMethod::Safe,
                                        AdnlSendMethodDetailed::Safe,
                                    )
                                }
                            }
                            SendMode::Safe => (
                                AdnlSendMethod::Safe,
                                AdnlSendMethodDetailed::Safe,
                                AdnlSendMethod::Safe,
                                AdnlSendMethodDetailed::Safe,
                            ),
                        };
                        let data = get_tagged_slice(&data);
                        let status = node1
                            .send_custom_get_status(&data, &peers1, send_method1)
                            .await
                            .unwrap();
                        assert!(wait(&ping).await);
                        if n > 0 {
                            // Skip first iteration to avoid channel setup side effect
                            check_status(
                                &status,
                                method1_sent,
                                Some(reset_timeout1),
                                &mode,
                                "msg 1->2",
                            );
                        }
                        let status = node2
                            .send_custom_get_status(&data, &peers2, send_method2)
                            .await
                            .unwrap();
                        assert!(wait(&ping).await);
                        if n > 0 {
                            // Skip first iteration to avoid channel setup side effect
                            check_status(
                                &status,
                                method2_sent,
                                Some(reset_timeout2),
                                &mode,
                                "msg 2->1",
                            );
                        }
                    }
                }
                if p == 1 {
                    break;
                }
                // Wait for reset channel due to no feedback
                let mut data = Vec::with_capacity(MOCK_MESSAGE.len());
                data.extend_from_slice(MOCK_MESSAGE);
                println!("Waiting for channel reset...");
                let start = now();
                let mut reset_at = 0;
                loop {
                    let status = node1
                        .send_custom_get_status(
                            &get_tagged_slice(&data),
                            &peers1,
                            AdnlSendMethod::Fast,
                        )
                        .await
                        .unwrap();
                    assert!(wait(&ping).await);
                    assert!(status.method == AdnlSendMethodDetailed::FastNormal);
                    if let Some(will_reset_at) = status.reset_at {
                        if reset_at == 0 {
                            reset_at = will_reset_at;
                            assert!(reset_at >= start + reset_timeout1);
                        } else if (will_reset_at > reset_at) && (reset_at < now()) {
                            break;
                        }
                    }
                }
            }
            for p in 0..3 {
                for n in 0..ATTEMPTS {
                    let query = TaggedTlObject {
                        object: AdnlPing { value: n }.into_tl_object(),
                        #[cfg(feature = "telemetry")]
                        tag: 0,
                    };
                    let (send_method1, method1_sent, send_method2, method2_sent) = match mode {
                        SendMode::Fast => (
                            AdnlSendMethod::Fast,
                            AdnlSendMethodDetailed::FastUrgent,
                            AdnlSendMethod::Fast,
                            AdnlSendMethodDetailed::FastUrgent,
                        ),
                        SendMode::Mixed => {
                            if (n & 0x01) == 1 {
                                (
                                    AdnlSendMethod::Safe,
                                    AdnlSendMethodDetailed::Safe,
                                    AdnlSendMethod::Fast,
                                    AdnlSendMethodDetailed::FastUrgent,
                                )
                            } else {
                                (
                                    AdnlSendMethod::Fast,
                                    AdnlSendMethodDetailed::FastUrgent,
                                    AdnlSendMethod::Safe,
                                    AdnlSendMethodDetailed::Safe,
                                )
                            }
                        }
                        SendMode::Safe => (
                            AdnlSendMethod::Safe,
                            AdnlSendMethodDetailed::Safe,
                            AdnlSendMethod::Safe,
                            AdnlSendMethodDetailed::Safe,
                        ),
                    };
                    let res =
                        node1.query_get_status(&query, &peers1, None, send_method1).await.unwrap();
                    if n > 1 {
                        // Skip first iterations to avoid channel setup side effect
                        // Queries may invoke message repeats
                        check_status(&res.status, method1_sent, None, &mode, "query 1->2");
                    }
                    match res.reply.unwrap().downcast::<AdnlPongBoxed>() {
                        Ok(pong) => assert!(pong.value() == &n),
                        Err(_) => assert!(false),
                    }
                    let res =
                        node2.query_get_status(&query, &peers2, None, send_method2).await.unwrap();
                    if n > 1 {
                        // Skip first iterations to avoid channel setup side effect
                        // Queries may invoke message repeats
                        check_status(&res.status, method2_sent, None, &mode, "query 2->1");
                    }
                    match res.reply.unwrap().downcast::<AdnlPongBoxed>() {
                        Ok(pong) => assert!(pong.value() == &n),
                        Err(_) => assert!(false),
                    }
                }
                if p == 2 {
                    break;
                }
                // Reset channel due to no feedback
                drop1.store(p == 1, Ordering::Relaxed);
                drop2.store(p == 0, Ordering::Relaxed);
                println!("Waiting for channel reset...");
                let start = now();
                let mut reset_at = 0;
                loop {
                    let (send_method, method_sent) = if mode == SendMode::Safe {
                        (AdnlSendMethod::Safe, AdnlSendMethodDetailed::Safe)
                    } else {
                        (AdnlSendMethod::Fast, AdnlSendMethodDetailed::FastUrgent)
                    };
                    let query = TaggedTlObject {
                        object: AdnlPing { value: 0 }.into_tl_object(),
                        #[cfg(feature = "telemetry")]
                        tag: 0,
                    };
                    let (node, peers) = if p == 0 { (&node1, &peers1) } else { (&node2, &peers2) };
                    let res =
                        node.query_get_status(&query, peers, Some(500), send_method).await.unwrap();
                    if let Some(will_reset_at) = res.status.reset_at {
                        if reset_at == 0 {
                            reset_at = will_reset_at;
                            assert!(reset_at >= start + node1.get_channel_reset_timeout());
                        } else if (will_reset_at > reset_at) && (reset_at < now()) {
                            break;
                        }
                    } else {
                        assert!(
                            res.status.method == method_sent,
                            "{} vs {method_sent}",
                            res.status.method
                        );
                    }
                }
                drop1.store(false, Ordering::Relaxed);
                drop2.store(false, Ordering::Relaxed);
            }
            #[cfg(feature = "telemetry")]
            let size = {
                let size = node1.telemetry().packet_size.maximum();
                size.max(node2.telemetry().packet_size.maximum())
            };
            #[cfg(not(feature = "telemetry"))]
            let size = 0;
            node1.stop().await;
            node2.stop().await;
            size
        });
        // #[cfg(not(feature = "debug"))]
        break packet_size;
    };
    println!(
        "ADNL session test, send mode {mode}, running time: {:.02}s, packet upto {} bytes",
        start.elapsed().as_millis() as f32 / 1000.0,
        packet_size
    );
}

#[test]
fn test_adnl_session_fast() {
    run_adnl_session(SendMode::Fast, "127.0.0.1:4191", "127.0.0.1:4192");
}

#[test]
fn test_adnl_session_mixed() {
    run_adnl_session(SendMode::Mixed, "127.0.0.1:4193", "127.0.0.1:4194");
}

#[test]
fn test_adnl_session_safe() {
    run_adnl_session(SendMode::Safe, "127.0.0.1:4195", "127.0.0.1:4196");
}
