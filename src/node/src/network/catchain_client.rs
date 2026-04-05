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
use crate::network::node_network::NetworkContext;
use adnl::{
    common::{
        AdnlPeers, Answer, CountedObject, Counter, QueryAnswer, QueryResult, Subscriber,
        TaggedByteSlice, TimedAnswer, Wait,
    },
    declare_counted,
    node::{AdnlNode, AdnlSendMethod},
    CatchainData, OverlayNode, OverlayParams, PrivateOverlayShortId,
};
use catchain::{
    BlockPayloadPtr, CatchainNode, CatchainOverlay, CatchainOverlayListenerPtr, PublicKeyHash,
    QueryResponseCallback,
};
#[cfg(feature = "telemetry")]
use std::sync::atomic::Ordering;
use std::{
    collections::HashMap,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
    time::Instant,
};
use ton_api::{
    deserialize_boxed, serialize_boxed, serialize_boxed_append, ton::ton_node::Broadcast,
    BoxedSerialize, IntoBoxed, TLObject,
};
use ton_block::{error, fail, KeyId, KeyOption, Result};

declare_counted!(
    pub struct CatchainClient {
        runtime_handle: tokio::runtime::Handle,
        overlay_id: Arc<PrivateOverlayShortId>,
        network_context: Arc<NetworkContext>,
        local_validator_key: Arc<dyn KeyOption>,
        validator_keys: HashMap<Arc<KeyId>, Arc<KeyId>>,
        consumer: Arc<CatchainClientConsumer>,
        is_stop: Arc<AtomicBool>,
    }
);

impl CatchainClient {
    const TARGET: &'static str = "catchain_network";

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        runtime_handle: &tokio::runtime::Handle,
        overlay_id: &Arc<PrivateOverlayShortId>,
        network_context: &Arc<NetworkContext>,
        nodes: &[CatchainNode],
        local_adnl_key: &Arc<dyn KeyOption>,
        local_validator_key: Arc<dyn KeyOption>,
        catchain_listener: CatchainOverlayListenerPtr,
        broadcast_hops: Option<u8>,
    ) -> Result<Self> {
        let mut keys = HashMap::new();
        let mut peers = Vec::new();
        let runtime_handle = runtime_handle.clone();

        for node in nodes {
            if node.public_key.id() == local_adnl_key.id() {
                continue;
            }
            keys.insert(node.adnl_id.clone(), node.public_key.id().clone());
            peers.push(node.adnl_id.clone());
        }

        let id_local_key = local_adnl_key.id();
        log::debug!(
            "new catchain_client: overlay_id {:x?}, id_local_key {:x?}",
            &overlay_id,
            &id_local_key
        );

        let params = OverlayParams {
            flags: 0,
            hops: broadcast_hops.or(network_context.broadcast_hops),
            overlay_id,
            runtime: Some(runtime_handle.clone()),
        };
        network_context.stack.overlay.add_private_overlay(params, local_adnl_key, &peers, false)?;
        let consumer = Arc::new(CatchainClientConsumer::new(overlay_id.clone(), catchain_listener));
        network_context.stack.overlay.add_consumer(overlay_id, consumer.clone())?;

        let ret = CatchainClient {
            runtime_handle,
            overlay_id: overlay_id.clone(),
            network_context: network_context.clone(),
            local_validator_key,
            validator_keys: keys,
            consumer,
            is_stop: Arc::new(AtomicBool::new(false)),
            counter: network_context.engine_allocated.catchain_clients.clone().into(),
        };
        #[cfg(feature = "telemetry")]
        network_context
            .engine_telemetry
            .catchain_clients
            .update(network_context.engine_allocated.catchain_clients.load(Ordering::Relaxed));
        Ok(ret)
    }

    pub async fn stop(&self) {
        match self.network_context.stack.overlay.delete_private_overlay(&self.overlay_id) {
            Err(e) => log::warn!("{:?}", e),
            _ => (),
        }
        self.is_stop.store(true, atomic::Ordering::Relaxed);
        self.consumer.is_stop.store(true, atomic::Ordering::Relaxed);
        for worker in self.consumer.worker_waiters.iter() {
            worker.val().respond(None);
        }
        log::debug!("Overlay {} stopped.", &self.overlay_id);
    }

    pub fn catchain_listener(&self) -> &CatchainOverlayListenerPtr {
        &self.consumer.catchain_listener
    }

    pub fn validator_keys(&self) -> &HashMap<Arc<KeyId>, Arc<KeyId>> {
        &self.validator_keys
    }

    pub fn message(&self, receiver_id: &PublicKeyHash, message: &BlockPayloadPtr) -> Result<()> {
        let overlay = self.network_context.stack.overlay.clone();
        let overlay_id = self.overlay_id.clone();
        let msg = message.clone();
        let receiver = receiver_id.clone();
        let is_stop_state = self.is_stop.clone();
        self.runtime_handle.spawn(async move {
            let is_stop = is_stop_state.load(atomic::Ordering::Relaxed);
            if is_stop {
                log::warn!("Overlay {} was stopped!", &overlay_id);
                return;
            }
            let buf = &msg.data();
            let tag = if buf.len() < 4 {
                0
            } else {
                ((buf[3] as u32) << 24)
                    | ((buf[2] as u32) << 16)
                    | ((buf[1] as u32) << 8)
                    | (buf[0] as u32)
            };
            let msg = TaggedByteSlice {
                object: buf,
                #[cfg(feature = "telemetry")]
                tag: 0x80000001, // Catchain one-way messages
            };
            let res = overlay.message(&receiver, &msg, &overlay_id).await;
            log::trace!(
                target: Self::TARGET,
                "<send_message> (overlay: {overlay_id}, data: {tag:08x}, key_id: {receiver}): {}",
                res.map_err(|e| e.to_string()).err().unwrap_or("OK".to_string())
            );
        });
        Ok(())
    }

    pub async fn query(
        overlay_id: &Arc<PrivateOverlayShortId>,
        overlay: &Arc<OverlayNode>,
        receiver_id: &PublicKeyHash,
        timeout: std::time::Duration,
        message: &BlockPayloadPtr,
    ) -> Result<BlockPayloadPtr> {
        let query = deserialize_boxed(message.data())?.into();
        let now = Instant::now();
        let result = overlay
            .query(
                receiver_id,
                &query,
                overlay_id,
                Some(AdnlNode::calc_timeout(Some(timeout.as_millis() as u64))),
            )
            .await?;
        let elapsed = now.elapsed();
        log::trace!(
            target: Self::TARGET,
            "<send_query> result (overlay: {}, data: {:08x}, key_id: {}): {:?}({}ms)",
            &overlay_id,
            query.object.bare_object().constructor(),
            &receiver_id,
            result,
            elapsed.as_millis()
        );
        metrics::histogram!("ton_node_network_catchain_overlay_query_seconds").record(elapsed);
        let result = result.ok_or_else(|| error!("answer is None!"))?;
        let data = serialize_boxed(&result)?;
        let data = catchain::CatchainFactory::create_block_payload(data);
        Ok(data)
    }

    async fn query_via_rldp(
        overlay_id: &Arc<PrivateOverlayShortId>,
        network: &Arc<NetworkContext>,
        _timeout: std::time::SystemTime,
        receiver_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) -> Result<BlockPayloadPtr> {
        let query_body = deserialize_boxed(message.data())?;
        let mut query = network.stack.overlay.get_query_prefix(overlay_id)?;
        serialize_boxed_append(&mut query, &query_body)?;
        let (data, _) = network
            .stack
            .overlay
            .query_via_rldp(
                receiver_id,
                &TaggedByteSlice {
                    object: &query[..],
                    #[cfg(feature = "telemetry")]
                    tag: query_body.bare_object().constructor(),
                },
                overlay_id,
                Some(max_answer_size as u64),
                v2,
                None,
            )
            .await?;
        let data = data.ok_or_else(|| error!("answer is None!"))?;
        Ok(catchain::CatchainFactory::create_block_payload(data))
    }

    pub fn run_wait_broadcast(
        self: Arc<Self>,
        runtime_handle: &tokio::runtime::Handle,
        private_overlay_id: &Arc<PrivateOverlayShortId>,
        overlay_node: &Arc<OverlayNode>,
        validator_keys: &HashMap<Arc<KeyId>, Arc<KeyId>>,
        catchain_listener: &CatchainOverlayListenerPtr,
    ) {
        let overlay_id = private_overlay_id.clone();
        let self1 = self.clone();
        let self2 = self.clone();
        let overlay = overlay_node.clone();
        let keys = validator_keys.clone();
        let listener = catchain_listener.clone();
        runtime_handle.spawn(async move {
            if let Err(e) =
                CatchainClient::wait_broadcasts(self1, &overlay_id, overlay, &keys, &listener).await
            {
                log::warn!(target: Self::TARGET, "ERROR: {}", e)
            }
        });
        let overlay_id = private_overlay_id.clone();
        let overlay = overlay_node.clone();
        let keys = validator_keys.clone();
        let listener = catchain_listener.clone();
        runtime_handle.spawn(async move {
            if let Err(e) = CatchainClient::wait_catchain_broadcast(
                self2,
                &overlay_id,
                overlay,
                &keys,
                &listener,
            )
            .await
            {
                log::warn!(target: Self::TARGET, "ERROR: {}", e)
            }
        });
    }

    async fn wait_broadcasts(
        self: Arc<Self>,
        overlay_id: &Arc<PrivateOverlayShortId>,
        overlay: Arc<OverlayNode>,
        _validator_keys: &HashMap<Arc<KeyId>, Arc<KeyId>>,
        catchain_listener: &CatchainOverlayListenerPtr,
    ) -> Result<()> {
        let receiver = overlay.clone();
        let result: Option<Box<Broadcast>> = None;
        let catchain_listener = catchain_listener.clone();

        while result.is_none() {
            if self.is_stop.load(atomic::Ordering::Relaxed) {
                break;
            };
            let message = receiver.wait_for_broadcast(overlay_id).await;
            match message {
                Ok(Some(message)) => {
                    log::trace!(target: Self::TARGET, "private overlay broadcast (successed)");
                    // let src_id = validator_keys.get(&message.1).ok_or_else(|| error!("unknown key!"))?;
                    if let Some(listener) = catchain_listener.upgrade() {
                        listener.on_broadcast(
                            message.recv_from,
                            &catchain::CatchainFactory::create_block_payload(message.data),
                        ); // Test id!
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    log::error!(target: Self::TARGET, "private overlay broadcast err: {}", e);
                }
            };
        }
        let _result =
            result.ok_or_else(|| error!("Failed to receive a private overlay broadcast!"))?;
        Ok(())
    }

    async fn wait_catchain_broadcast(
        self: Arc<Self>,
        overlay_id: &Arc<PrivateOverlayShortId>,
        overlay: Arc<OverlayNode>,
        _validator_keys: &HashMap<Arc<KeyId>, Arc<KeyId>>,
        catchain_listener: &CatchainOverlayListenerPtr,
    ) -> Result<()> {
        let receiver = overlay.clone();
        let result: Option<Box<Broadcast>> = None;
        let catchain_listener = catchain_listener.clone();

        while result.is_none() {
            if self.is_stop.load(atomic::Ordering::Relaxed) {
                break;
            };
            let message = receiver.wait_for_catchain(overlay_id).await;
            match message {
                Ok(Some((catchain_block_update, inner_update, source_id))) => {
                    log::trace!(
                        target: Self::TARGET,
                        "private overlay broadcast ValidatorSession_BlockUpdate (successed)"
                    );
                    if let Some(listener) = catchain_listener.upgrade() {
                        let mut data: catchain::RawBuffer = catchain::RawBuffer::default();
                        let mut serializer = ton_api::Serializer::new(&mut data);
                        serializer.write_boxed(&catchain_block_update.into_boxed())?;
                        match inner_update {
                            CatchainData::Catchain(upd) => {
                                serializer.write_boxed(&upd.into_boxed())?
                            }
                            CatchainData::ValidatorSession(upd) => {
                                serializer.write_boxed(&upd.into_boxed())?
                            }
                        };
                        let data = catchain::CatchainFactory::create_block_payload(data);
                        listener.on_message(source_id, &data);
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    log::error!(target: Self::TARGET, "private overlay broadcast err: {}", e);
                }
            };
        }
        let _result =
            result.ok_or_else(|| error!("Failed to receive a private overlay broadcast!"))?;
        Ok(())
    }
}

impl CatchainOverlay for CatchainClient {
    fn get_impl(&self) -> &dyn std::any::Any {
        self
    }

    /// Send message
    fn send_message(
        &self,
        receiver_id: &PublicKeyHash,
        _sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        let now = Instant::now();
        match self.message(receiver_id, message) {
            Ok(_) => { /*log::trace!("send_message success!");*/ }
            Err(e) => {
                log::warn!(target: Self::TARGET, "send_message err: {:?}", e);
            }
        }

        let elapsed = now.elapsed();
        if elapsed.as_micros() > 500 {
            log::trace!(target: Self::TARGET, "message elapsed: {}", elapsed.as_millis());
        };
        metrics::histogram!("ton_node_network_catchain_send_seconds").record(elapsed);
    }

    /// Send message to multiple sources
    fn send_message_multicast(
        &self,
        receiver_ids: &[PublicKeyHash],
        _sender_id: &PublicKeyHash,
        message: &BlockPayloadPtr,
        _is_retransmission: bool,
    ) {
        for receiver_id in receiver_ids.iter() {
            if let Err(e) = self.message(receiver_id, message) {
                log::error!(target: Self::TARGET, "send_message err: {:?}", e);
            }
        }
    }

    /// Send query
    fn send_query(
        &self,
        receiver_id: &PublicKeyHash,
        _sender_id: &PublicKeyHash,
        _name: &str,
        timeout: std::time::Duration,
        message: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        let receiver = receiver_id.clone();
        let msg = message.clone();
        let overlay_id = self.overlay_id.clone();
        let overlay = self.network_context.stack.overlay.clone();
        let is_stop_state = self.is_stop.clone();
        self.runtime_handle.spawn(async move {
            let is_stop = is_stop_state.load(atomic::Ordering::Relaxed);
            if is_stop {
                log::warn!(target: Self::TARGET, "Overlay {} was stopped!", &overlay_id);
                return;
            }
            let result =
                CatchainClient::query(&overlay_id, &overlay, &receiver, timeout, &msg).await;
            response_callback(result);
        });
    }

    /// Send query via RLDP (ADNL ID of the current node should be registered for the query)
    fn send_query_via_rldp(
        &self,
        dst: PublicKeyHash,
        _name: String,
        response_callback: QueryResponseCallback,
        timeout: std::time::SystemTime,
        query: BlockPayloadPtr,
        max_answer_size: u64,
        v2: bool,
    ) {
        let overlay_id = self.overlay_id.clone();
        let network = self.network_context.clone();
        self.runtime_handle.spawn(async move {
            let result = CatchainClient::query_via_rldp(
                &overlay_id,
                &network,
                timeout,
                &dst,
                &query,
                max_answer_size,
                v2,
            )
            .await;
            log::info!(target: Self::TARGET, "send_query_via_rldp: {:?}", result);
            response_callback(result);
        });
    }

    /// Send broadcast
    fn send_broadcast_fec_ex(
        &self,
        _sender_id: &PublicKeyHash,
        _send_as: &PublicKeyHash,
        payload: BlockPayloadPtr,
        _extra: Option<Vec<u8>>,
    ) {
        let msg = payload.clone();
        let overlay_id = self.overlay_id.clone();
        let overlay = self.network_context.stack.overlay.clone();
        let local_validator_key = self.local_validator_key.clone();
        self.runtime_handle.spawn(async move {
            let msg = TaggedByteSlice {
                object: msg.data(),
                #[cfg(feature = "telemetry")]
                tag: 0x80000002, // Catchain broadcast
            };
            let result = overlay
                .broadcast(&overlay_id, &msg, Some(&local_validator_key), 0, AdnlSendMethod::Fast)
                .await;
            log::debug!(target: Self::TARGET, "send_broadcast_fec_ex status: {:?}", result);
        });
    }
}

type WorkerAwaitersPtr = Arc<lockfree::map::Map<u128, Arc<Wait<Result<Option<Answer>>>>>>;
struct CatchainClientConsumer {
    catchain_listener: CatchainOverlayListenerPtr,
    is_stop: AtomicBool,
    overlay_id: Arc<PrivateOverlayShortId>,
    worker_waiters: WorkerAwaitersPtr,
}

impl CatchainClientConsumer {
    fn new(
        overlay_id: Arc<PrivateOverlayShortId>,
        catchain_listener: CatchainOverlayListenerPtr,
    ) -> Self {
        Self {
            catchain_listener,
            is_stop: AtomicBool::new(false),
            overlay_id,
            worker_waiters: Arc::new(lockfree::map::Map::new()),
        }
    }
}

#[async_trait::async_trait]
impl Subscriber for CatchainClientConsumer {
    async fn try_consume_object(&self, _object: TLObject, _peers: &AdnlPeers) -> Result<bool> {
        Ok(false)
    }

    async fn try_consume_query(&self, query: TLObject, peers: &AdnlPeers) -> Result<QueryResult> {
        let is_stop = self.is_stop.load(atomic::Ordering::Relaxed);
        if is_stop {
            log::warn!(target: CatchainClient::TARGET, "Overlay {} was stopped!", &self.overlay_id);
            fail!("Overlay {} was stopped!", &self.overlay_id);
        }
        let now = Instant::now();
        let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;

        let data = match serialize_boxed(&query) {
            Ok(query) => query,
            Err(e) => {
                log::warn!("query is bad: {:?}", e);
                fail!(e)
            }
        };

        // Spawn the task to let catchain query processed asynchronously
        let catchain_listener = self.catchain_listener.clone();
        let peers = peers.clone();
        let worker_waiters = self.worker_waiters.clone();
        let handle = tokio::spawn(async move {
            let (wait, mut queue_reader) = Wait::new();
            worker_waiters.insert(id.as_nanos(), wait.clone());
            if let Some(listener) = catchain_listener.upgrade() {
                let wait = wait.clone();
                wait.request();
                listener.on_query(
                    peers.other().clone(),
                    &catchain::CatchainFactory::create_block_payload(data),
                    Box::new(move |result: Result<BlockPayloadPtr>| {
                        let result = match result {
                            Ok(answer) => match deserialize_boxed(answer.data()) {
                                Ok(answer) => {
                                    let ret = Answer::Object(answer.into());
                                    Ok(Some(ret))
                                }
                                Err(e) => Err(e),
                            },
                            Err(e) => Err(e),
                        };
                        wait.respond(Some(result));
                    }),
                );
            }
            let res = match wait.wait(&mut queue_reader, true).await {
                Some(None) => fail!("Answer was not set!"),
                Some(Some(answer)) => answer,
                None => {
                    log::warn!(
                        target: CatchainClient::TARGET,
                        "Waiting returned an internal error (query: {:?})",
                        query
                    );
                    fail!("Waiting returned an internal error!");
                }
            };
            if log::log_enabled!(log::Level::Trace) {
                let elapsed = now.elapsed();
                log::trace!(
                    target: CatchainClient::TARGET,
                    "query elapsed: {}",
                    elapsed.as_millis()
                );
            };
            metrics::histogram!("ton_node_network_catchain_client_query_seconds")
                .record(now.elapsed());
            worker_waiters.remove(&id.as_nanos());
            Ok(TimedAnswer {
                answer: res?,
                #[cfg(feature = "telemetry")]
                actual_start_at: None,
            })
        });

        Ok(QueryResult::Consumed(QueryAnswer::Pending(handle)))
    }
}
