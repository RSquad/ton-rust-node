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
use crate::common::{AdnlHandshake, AdnlStream, AdnlStreamCrypto, Query, TaggedTlObject, Timeouts};
use rand::{Rng, RngCore};
use std::{
    convert::TryInto,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use ton_api::{
    deserialize_boxed, deserialize_typed, serialize_boxed, serialize_boxed_inplace,
    ton::{
        adnl::{Message as AdnlMessage, Pong as AdnlPongBoxed},
        rpc::adnl::Ping as AdnlPing,
        tcp::{
            message::{Authentificate, AuthentificationComplete},
            Message as TcpMessage,
        },
        PublicKey,
    },
    AnyBoxedSerialize, IntoBoxed, TLObject,
};
use ton_block::{error, fail, Ed25519KeyOption, KeyOption, KeyOptionJson, Result};

#[derive(serde::Deserialize, serde::Serialize)]
pub struct AdnlClientConfigJson {
    client_key: Option<KeyOptionJson>,
    server_address: String,
    server_key: KeyOptionJson,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    timeouts: Option<Timeouts>,
}

impl AdnlClientConfigJson {
    pub fn with_params(
        server: &str,
        server_key: KeyOptionJson,
        timeouts: Option<Timeouts>,
        client_key: Option<KeyOptionJson>,
    ) -> Self {
        AdnlClientConfigJson {
            client_key,
            server_address: server.to_string(),
            server_key,
            timeouts,
        }
    }
}

/// ADNL client configuration
pub struct AdnlClientConfig {
    client_key: Option<Arc<dyn KeyOption>>,
    server_address: SocketAddr,
    server_key: Arc<dyn KeyOption>,
    timeouts: Timeouts,
}

impl AdnlClientConfig {
    pub fn new(
        client_key: Option<Arc<dyn KeyOption>>,
        server_address: SocketAddr,
        server_key: Arc<dyn KeyOption>,
        timeouts: Timeouts,
    ) -> Self {
        Self { client_key, server_address, server_key, timeouts }
    }

    /// Costructs new configuration from JSON string
    pub fn from_json(json: &str) -> Result<(Option<AdnlClientConfigJson>, Self)> {
        let json_config: AdnlClientConfigJson = serde_json::from_str(json)?;
        Self::from_json_config(&json_config)
    }

    /// Costructs new configuration from JSON data
    pub fn from_json_config(
        json_config: &AdnlClientConfigJson,
    ) -> Result<(Option<AdnlClientConfigJson>, Self)> {
        let server_key = Ed25519KeyOption::from_public_key_json(&json_config.server_key)?;
        let mut result_config = None;
        let client_key = if let Some(key) = &json_config.client_key {
            Some(Ed25519KeyOption::from_private_key_json(key)?)
        } else {
            let (json, key) = Ed25519KeyOption::generate_with_json()?;
            result_config = Some(AdnlClientConfigJson {
                client_key: Some(json),
                server_address: json_config.server_address.clone(),
                server_key: json_config.server_key.clone(),
                timeouts: json_config.timeouts.clone(),
            });
            Some(key)
        };
        let ret = AdnlClientConfig {
            client_key,
            server_address: json_config.server_address.parse()?,
            server_key,
            timeouts: if let Some(timeouts) = &json_config.timeouts {
                timeouts.clone()
            } else {
                Timeouts::default()
            },
        };
        Ok((result_config, ret))
    }

    /// Get timeouts
    pub fn timeouts(&self) -> &Timeouts {
        &self.timeouts
    }
}

/// ADNL client
pub struct AdnlClient {
    crypto: AdnlStreamCrypto,
    stream: AdnlStream,
}

impl AdnlClient {
    /// Connect to server
    pub async fn connect(config: &AdnlClientConfig) -> Result<Self> {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None)?;
        socket.set_reuse_address(true)?;
        socket.set_linger(Some(Duration::from_secs(0)))?;
        //socket.bind(&"0.0.0.0:0".parse::<SocketAddr>()?.into())?;
        socket.connect_timeout(&config.server_address.into(), config.timeouts.write())?;
        socket.set_nonblocking(true)?;

        let mut stream = AdnlStream::from_stream_with_timeouts(
            tokio::net::TcpStream::from_std(socket.into())?,
            config.timeouts(),
        );

        let mut crypto = Self::send_init_packet(&mut stream, config).await?;
        if let Some(client_key) = &config.client_key {
            Self::tcp_auth_handshake(&mut crypto, &mut stream, client_key).await?;
        }
        Ok(Self { crypto, stream })
    }

    /// Like [`Self::connect`], but uses `tokio::net::TcpStream::connect` so the
    /// runtime worker is not parked while the kernel waits on an unresponsive peer.
    pub async fn timeout_connect(config: &AdnlClientConfig) -> Result<Self> {
        let connect_timeout = config.timeouts.write();
        let tcp = tokio::time::timeout(
            connect_timeout,
            tokio::net::TcpStream::connect(config.server_address),
        )
        .await
        .map_err(|_| {
            error!(
                "ADNL TCP connect to {} timed out after {:?}",
                config.server_address, connect_timeout,
            )
        })?
        .map_err(|e| error!("ADNL TCP connect to {} failed: {}", config.server_address, e))?;

        socket2::SockRef::from(&tcp).set_linger(Some(Duration::from_secs(0)))?;

        let mut stream = AdnlStream::from_stream_with_timeouts(tcp, config.timeouts());

        let mut crypto = Self::send_init_packet(&mut stream, config).await?;
        if let Some(client_key) = &config.client_key {
            Self::tcp_auth_handshake(&mut crypto, &mut stream, client_key).await?;
        }
        Ok(Self { crypto, stream })
    }

    /// Ping server
    pub async fn ping(&mut self) -> Result<u64> {
        let now = Instant::now();
        let value = rand::thread_rng().gen();
        let query = AdnlPing { value }.into_tl_object().into();
        let answer: AdnlPongBoxed = Query::parse(self.query(&query).await?, &query.object)?;
        if answer.value() != &value {
            fail!("Bad reply to ADNL ping")
        }
        Ok(now.elapsed().as_secs())
    }

    /// Shutdown client
    pub async fn shutdown(mut self) -> Result<()> {
        self.stream.shutdown().await?;
        Ok(())
    }

    /// Query server
    pub async fn query(&mut self, query: &TaggedTlObject) -> Result<TLObject> {
        let (query_id, msg) = Query::build(None, query)?;
        let mut buf = serialize_boxed(&msg.object)?;
        self.crypto.send(&mut self.stream, &mut buf).await?;
        loop {
            self.crypto.receive(&mut buf, &mut self.stream).await?;
            if !buf.is_empty() {
                break;
            }
        }
        match deserialize_typed(buf)? {
            AdnlMessage::Adnl_Message_Answer(answer) => {
                if &query_id == answer.query_id.as_slice() {
                    deserialize_boxed(&answer.answer)
                } else {
                    fail!("Query ID mismatch {:?} vs {:?}", query.object, answer)
                }
            }
            answer => fail!("Unexpected answer to query {:?}: {:?}", query.object, answer),
        }
    }

    async fn send_init_packet(
        stream: &mut AdnlStream,
        config: &AdnlClientConfig,
    ) -> Result<AdnlStreamCrypto> {
        let mut buf = vec![0u8; 160];
        rand::thread_rng().fill(buf.as_mut_slice());
        let nonce = buf.as_slice().try_into()?;
        let mut ret = AdnlStreamCrypto::with_nonce_as_client(nonce);
        if let Some(client_key) = &config.client_key {
            AdnlHandshake::build_packet(&mut buf, client_key, &config.server_key, None)?
        } else {
            AdnlHandshake::build_packet(
                &mut buf,
                &Ed25519KeyOption::generate()?,
                &config.server_key,
                None,
            )?
        }
        stream.write(&mut buf).await?;
        ret.receive(&mut buf, stream).await?;
        if !buf.is_empty() {
            fail!("Not empty init ACK packet")
        }
        Ok(ret)
    }

    async fn tcp_auth_handshake(
        crypto: &mut AdnlStreamCrypto,
        stream: &mut AdnlStream,
        client_key: &Arc<dyn KeyOption>,
    ) -> Result<()> {
        const CLIENT_NONCE_LEN: usize = 32;
        let mut client_nonce = vec![0u8; CLIENT_NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut client_nonce);

        let auth = Authentificate { nonce: client_nonce.clone() }.into_boxed();
        let mut buf = Vec::new();
        serialize_boxed_inplace(&mut buf, &auth)?;
        crypto.send(stream, &mut buf).await?;

        // wait tcp.authentificationNonce{ nonce: S }
        let server_nonce = loop {
            buf.clear();
            crypto.receive(&mut buf, stream).await?;
            if buf.is_empty() {
                continue;
            }
            let msg = deserialize_boxed(&buf[..])?
                .downcast::<TcpMessage>()
                .map_err(|_| error!("TCP auth: unexpected non-TCP message during handshake"))?;
            match msg {
                TcpMessage::Tcp_AuthentificationNonce(msg) => {
                    let s = msg.nonce;
                    if s.is_empty() || s.len() > 512 {
                        fail!("TCP auth: bad nonce size {} from server", s.len());
                    }
                    break s;
                }
                msg => fail!("Unexpected TCP auth message {msg:?}"),
            }
        };

        // [C || S]
        let mut full = Vec::with_capacity(client_nonce.len() + server_nonce.len());
        full.extend_from_slice(&client_nonce);
        full.extend_from_slice(&server_nonce);
        let complete = AuthentificationComplete {
            key: PublicKey::try_from(client_key)?,
            signature: client_key.sign(&full)?,
        }
        .into_boxed();
        serialize_boxed_inplace(&mut buf, &complete)?;
        crypto.send(stream, &mut buf).await
    }
}
