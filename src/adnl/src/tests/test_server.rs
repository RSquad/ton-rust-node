/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::{AdnlServerThread, TcpAuthState};
use crate::common::{AdnlPeers, AdnlStream, AdnlStreamCrypto, TaggedAdnlMessage, Timeouts};
use rand::{Rng, RngCore};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
#[cfg(feature = "telemetry")]
use std::time::Instant;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc,
    time::{timeout, Duration},
};
use ton_api::{
    deserialize_boxed, serialize_boxed, serialize_boxed_append,
    ton::{
        adnl::{
            message::message::{Answer as AdnlAnswerMessage, Query as AdnlQuery},
            pong::Pong as AdnlPong,
            Message as AdnlMessage,
        },
        rpc::adnl::Ping as AdnlPing,
    },
    AnyBoxedSerialize, IntoBoxed,
};
use ton_block::{fail, KeyId, Result, UInt256};

const TOTAL_QUERIES: usize = 64;
const BUNDLE_LEN: usize = 1024;
const REPLY_DELAY_MS: u64 = 1;
const TEST_TIMEOUT_SEC: u64 = 10;

fn build_query_bundle(seed: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(BUNDLE_LEN * 16);
    for i in 0..BUNDLE_LEN {
        let ping = AdnlPing { value: (seed + i as u64) as i64 }.into_tl_object();
        serialize_boxed_append(&mut buf, &ping)?;
    }
    Ok(buf)
}

fn build_answer(query_id: UInt256, value: u64) -> Result<TaggedAdnlMessage> {
    let pong = AdnlPong { value: value as i64 }.into_boxed().into_tl_object();
    let answer = serialize_boxed(&pong)?;
    Ok(TaggedAdnlMessage {
        object: AdnlAnswerMessage { query_id, answer: answer.into() }.into_boxed(),
        #[cfg(feature = "telemetry")]
        tag: 0,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_adnl_server_cancel_safe_reply_race() -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (client_tcp, server_tcp) = tokio::try_join!(TcpStream::connect(addr), async {
        listener.accept().await.map(|(s, _)| s)
    })?;
    let timeouts = Timeouts::default();
    let mut client_stream = AdnlStream::from_stream_with_timeouts(client_tcp, &timeouts);
    let server_stream = AdnlStream::from_stream_with_timeouts(server_tcp, &timeouts);

    let mut nonce = [0u8; 160];
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut client_crypto = AdnlStreamCrypto::with_nonce_as_client(&nonce);
    let mut server_nonce = nonce;
    let server_crypto = AdnlStreamCrypto::with_nonce_as_server(&mut server_nonce);

    let auth = TcpAuthState { nonce: None, remote_id: None };
    let peer = KeyId::from_data([0u8; 32]);
    let mut context = AdnlServerThread {
        auth,
        crypto: server_crypto,
        peers: AdnlPeers::with_keys(peer.clone(), peer),
        recv_buf: Vec::with_capacity(1024),
        send_buf: Vec::with_capacity(1024),
        stream: server_stream,
        subscribers: Arc::new(Vec::new()),
        #[cfg(feature = "telemetry")]
        start: Instant::now(),
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<(AdnlQuery, TaggedAdnlMessage)>();
    let pending = Arc::new(AtomicUsize::new(0));

    let server_task = {
        let pending = pending.clone();
        tokio::spawn(async move {
            let mut processed = 0usize;
            loop {
                tokio::select! {
                    biased;
                    msg = rx.recv() => {
                        if let Some((query, reply)) = msg {
                            context.reply(query, reply).await?;
                            pending.fetch_sub(1, Ordering::Relaxed);
                            if processed >= TOTAL_QUERIES && pending.load(Ordering::Relaxed) == 0 {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                    res = context.recv() => {
                        res?;
                        let msg = deserialize_boxed(&context.recv_buf[..])?;
                        let msg = match msg.downcast::<AdnlMessage>() {
                            Ok(msg) => msg,
                            Err(msg) => fail!("Unexpected ADNL message {msg:?}"),
                        };
                        let AdnlMessage::Adnl_Message_Query(query) = msg else {
                            fail!("Unexpected ADNL message {msg:?}");
                        };
                        processed += 1;
                        let query_id = query.query_id.clone();
                        let reply = build_answer(query_id.clone(), processed as u64)?;
                        let tx = tx.clone();
                        pending.fetch_add(1, Ordering::Relaxed);
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(REPLY_DELAY_MS)).await;
                            let _ = tx.send((
                                AdnlQuery { query_id, query: Vec::<u8>::new().into() },
                                reply,
                            ));
                        });
                    }
                }
            }
            Ok::<_, ton_block::Error>(())
        })
    };

    for i in 0..TOTAL_QUERIES {
        let query = build_query_bundle(i as u64)?;
        let query_id: [u8; 32] = rand::thread_rng().gen();
        let msg = AdnlQuery { query_id: UInt256::with_array(query_id), query: query.into() };
        let msg = msg.into_boxed();
        let mut buf = serialize_boxed(&msg)?;
        client_crypto.send(&mut client_stream, &mut buf).await?;
    }

    let mut replies = 0usize;
    let mut recv_buf = Vec::new();
    let result = timeout(Duration::from_secs(TEST_TIMEOUT_SEC), async {
        while replies < TOTAL_QUERIES {
            client_crypto.receive(&mut recv_buf, &mut client_stream).await?;
            if recv_buf.is_empty() {
                continue;
            }
            let msg = deserialize_boxed(&recv_buf[..])?;
            let msg = match msg.downcast::<AdnlMessage>() {
                Ok(msg) => msg,
                Err(msg) => fail!("Unexpected ADNL message {msg:?}"),
            };
            if let AdnlMessage::Adnl_Message_Answer(answer) = msg {
                let _ = deserialize_boxed(&answer.answer)?;
                replies += 1;
            }
        }
        Ok::<_, ton_block::Error>(())
    })
    .await;
    if result.is_err() {
        fail!("timeout waiting for replies");
    }
    result.unwrap()?;

    server_task.await??;
    Ok(())
}
