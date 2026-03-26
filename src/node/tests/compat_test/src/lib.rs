/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Cross-Implementation Compatibility Test Library
//!
//! This crate provides utilities for testing compatibility between the rust
//! and cpp ADNL/overlay implementations.

use base64::Engine;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{self, Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{channel, Receiver, RecvTimeoutError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub mod overlay_id;
pub mod test_helpers;

/// Error type for compatibility tests
#[derive(thiserror::Error, Debug)]
pub enum CompatTestError {
    #[error("C++ binary not found: {0}")]
    BinaryNotFound(String),

    #[error("C++ node failed to start: {0}")]
    NodeStartFailed(String),

    #[error("Command failed: {0}")]
    CommandFailed(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Timeout waiting for response")]
    Timeout,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("Node not ready")]
    NotReady,
}

pub type Result<T> = std::result::Result<T, CompatTestError>;

/// Default paths to look for the C++ test binary
const DEFAULT_CPP_BINARY_PATHS: &[&str] =
    &["cpp_src/build/compat_test_node", "../compat_test/cpp_src/build/compat_test_node"];

/// Timeout for waiting for C++ node to become ready
const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for individual command responses
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Check if C++ test binary is available
pub fn cpp_binary_available() -> bool {
    get_cpp_binary_path().is_ok()
}

/// Get path to C++ test binary
pub fn get_cpp_binary_path() -> Result<String> {
    // First check environment variable
    if let Ok(path) = std::env::var("CPP_COMPAT_TEST_BIN") {
        if Path::new(&path).exists() {
            return Ok(path);
        }
    }

    // Try default paths relative to CARGO_MANIFEST_DIR
    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        for rel_path in DEFAULT_CPP_BINARY_PATHS {
            let full_path = Path::new(&manifest_dir).join(rel_path);
            if full_path.exists() {
                return Ok(full_path.to_string_lossy().to_string());
            }
        }
    }

    // Try default paths relative to current directory
    for rel_path in DEFAULT_CPP_BINARY_PATHS {
        if Path::new(rel_path).exists() {
            return Ok(rel_path.to_string());
        }
    }

    Err(CompatTestError::BinaryNotFound(
        "C++ binary not found. Set CPP_COMPAT_TEST_BIN or build cpp_src/build/compat_test_node"
            .to_string(),
    ))
}

fn b64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(s: &str) -> std::result::Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(s)
}

/// Command to send to C++ node (JSON over stdin)
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "cmd")]
pub enum CppCommand {
    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "get_info")]
    GetInfo,

    #[serde(rename = "compute_overlay_id")]
    ComputeOverlayId {
        /// base64-encoded overlay name bytes
        name: String,
    },

    #[serde(rename = "add_peer")]
    AddPeer {
        /// base64-encoded TL-serialized public key
        pubkey: String,
        ip: String,
        port: u16,
    },

    #[serde(rename = "create_overlay")]
    CreateOverlay {
        /// "public", "private", or "semiprivate"
        #[serde(rename = "type")]
        overlay_type: String,
        /// base64-encoded overlay name bytes
        overlay_name: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        peers: Vec<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        root_pub_keys: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        certificate: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_slaves: Option<i32>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        #[serde(default)]
        enable_twostep: bool,
    },

    #[serde(rename = "delete_overlay")]
    DeleteOverlay { overlay_id: String },

    #[serde(rename = "get_overlay_node_info")]
    GetOverlayNodeInfo { overlay_id: String },

    #[serde(rename = "send_broadcast")]
    SendBroadcast {
        overlay_id: String,
        /// base64-encoded data
        data: String,
        #[serde(default)]
        use_fec: bool,
    },

    #[serde(rename = "send_query")]
    SendQuery {
        overlay_id: String,
        peer_adnl_id: String,
        /// base64-encoded query data
        data: String,
        timeout_ms: i64,
    },

    #[serde(rename = "send_rldp_query")]
    SendRldpQuery {
        overlay_id: String,
        peer_adnl_id: String,
        /// base64-encoded query data
        data: String,
        max_answer_size: u64,
        #[serde(default)]
        v2: bool,
    },

    #[serde(rename = "set_query_handler")]
    SetQueryHandler {
        overlay_id: String,
        /// "echo", "capabilities", or "reject"
        mode: String,
    },

    #[serde(rename = "set_broadcast_validator")]
    SetBroadcastValidator {
        overlay_id: String,
        /// "accept_all" or "reject_all"
        mode: String,
    },

    #[serde(rename = "get_received_broadcasts")]
    GetReceivedBroadcasts { overlay_id: String },

    #[serde(rename = "clear_received_broadcasts")]
    ClearReceivedBroadcasts { overlay_id: String },

    #[serde(rename = "send_message")]
    SendMessage {
        overlay_id: String,
        peer_adnl_id: String,
        /// base64-encoded data
        data: String,
    },

    #[serde(rename = "get_received_messages")]
    GetReceivedMessages { overlay_id: String },

    #[serde(rename = "clear_received_messages")]
    ClearReceivedMessages { overlay_id: String },

    #[serde(rename = "compress_boc")]
    CompressBoc {
        /// base64-encoded standard BOC data
        data: String,
        /// "baseline" or "improved"
        algorithm: String,
    },

    #[serde(rename = "decompress_boc")]
    DecompressBoc {
        /// base64-encoded compressed BOC data
        data: String,
        /// Maximum decompressed size in bytes
        max_size: u32,
    },

    #[serde(rename = "compute_candidate_id_to_sign")]
    ComputeCandidateIdToSign {
        slot: i32,
        /// 32-byte candidate hash as hex
        hash: String,
    },

    #[serde(rename = "enable_quic")]
    EnableQuic {},

    #[serde(rename = "send_quic_message")]
    SendQuicMessage {
        peer_adnl_id: String,
        /// base64-encoded data
        data: String,
    },

    #[serde(rename = "send_quic_query")]
    SendQuicQuery {
        peer_adnl_id: String,
        /// base64-encoded data
        data: String,
        timeout_ms: i64,
    },

    #[serde(rename = "shutdown")]
    Shutdown,
}

/// Ready response from C++ node
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReadyResponse {
    pub status: String,
    pub adnl_id: String,
    pub pubkey: String,
    pub udp_port: u16,
}

/// Response from C++ node
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)]
pub enum CppResponse {
    Ready(ReadyResponse),
    Result { result: serde_json::Value },
    Error { error: String },
}

/// Received broadcast record from C++ node
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReceivedBroadcast {
    pub source: String,
    pub size: usize,
    pub data: String, // base64 encoded
    pub timestamp: i32,
    pub accepted: bool,
}

/// Received message record from C++ node (point-to-point overlay messages)
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReceivedMessage {
    pub source: String,
    pub size: usize,
    pub data: String, // base64 encoded
    pub timestamp: i32,
}

/// Info about the C++ node
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub adnl_id: String,
    pub pubkey: String,
    pub udp_port: u16,
}

/// Handle to a running C++ test node
pub struct CppTestNode {
    process: Child,
    stdin: ChildStdin,
    response_rx: Receiver<String>,
    _reader_thread: Option<JoinHandle<()>>,
    info: NodeInfo,
}

impl CppTestNode {
    /// Spawn a new C++ test node on the given UDP port
    pub fn spawn(udp_port: u16) -> Result<Self> {
        let binary_path = get_cpp_binary_path()?;

        let db_path = format!("/tmp/compat_test_cpp_{}", udp_port);

        // Clean up old database
        let _ = std::fs::remove_dir_all(&db_path);

        let mut process = Command::new(&binary_path)
            .arg("--port")
            .arg(udp_port.to_string())
            .arg("--db")
            .arg(&db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| CompatTestError::NodeStartFailed(e.to_string()))?;

        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| CompatTestError::NodeStartFailed("Failed to get stdin".to_string()))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| CompatTestError::NodeStartFailed("Failed to get stdout".to_string()))?;

        // Spawn a reader thread to avoid blocking on stdout
        let (tx, rx) = channel();
        let reader_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(line).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut node = Self {
            process,
            stdin,
            response_rx: rx,
            _reader_thread: Some(reader_thread),
            info: NodeInfo { adnl_id: String::new(), pubkey: String::new(), udp_port },
        };

        // Wait for ready message
        node.wait_ready()?;

        Ok(node)
    }

    /// Read one line from the response channel with a timeout
    fn recv_line(&self, timeout: Duration) -> Result<String> {
        self.response_rx.recv_timeout(timeout).map_err(|e| match e {
            RecvTimeoutError::Timeout => CompatTestError::Timeout,
            RecvTimeoutError::Disconnected => CompatTestError::InvalidResponse(
                "Reader thread disconnected (process may have crashed)".to_string(),
            ),
        })
    }

    /// Wait for the node to be ready
    fn wait_ready(&mut self) -> Result<()> {
        let line = self.recv_line(DEFAULT_READY_TIMEOUT).map_err(|e| {
            CompatTestError::NodeStartFailed(format!(
                "Timed out waiting for C++ node to become ready: {}",
                e
            ))
        })?;

        let response: CppResponse = serde_json::from_str(&line)?;

        match response {
            CppResponse::Ready(ready) => {
                if ready.status != "ready" {
                    return Err(CompatTestError::NodeStartFailed(format!(
                        "Unexpected status: {}",
                        ready.status
                    )));
                }
                self.info.adnl_id = ready.adnl_id;
                self.info.pubkey = ready.pubkey;
                self.info.udp_port = ready.udp_port;
                Ok(())
            }
            _ => Err(CompatTestError::NodeStartFailed(format!(
                "Unexpected response: {:?}",
                response
            ))),
        }
    }

    /// Send a command and get response
    pub fn send_command(&mut self, cmd: &CppCommand) -> Result<CppResponse> {
        let json = serde_json::to_string(cmd)?;
        writeln!(self.stdin, "{}", json)?;
        self.stdin.flush()?;

        let line = self.recv_line(DEFAULT_COMMAND_TIMEOUT)?;

        if line.is_empty() {
            return Err(CompatTestError::InvalidResponse(
                "Empty response (process may have crashed)".to_string(),
            ));
        }

        let response: CppResponse = serde_json::from_str(&line)?;
        Ok(response)
    }

    /// Extract result value, returning error if response is an error
    fn expect_result(&mut self, cmd: &CppCommand) -> Result<serde_json::Value> {
        let response = self.send_command(cmd)?;
        match response {
            CppResponse::Result { result } => Ok(result),
            CppResponse::Error { error } => Err(CompatTestError::CommandFailed(error)),
            _ => Err(CompatTestError::InvalidResponse("Unexpected response type".to_string())),
        }
    }

    // ---- Info ----

    /// Get node info
    pub fn info(&self) -> &NodeInfo {
        &self.info
    }

    /// Get local ADNL ID (hex)
    pub fn adnl_id(&self) -> &str {
        &self.info.adnl_id
    }

    /// Get local public key (base64 TL)
    pub fn pubkey(&self) -> &str {
        &self.info.pubkey
    }

    /// Get UDP port
    pub fn udp_port(&self) -> u16 {
        self.info.udp_port
    }

    // ---- Basic commands ----

    /// Ping the node
    pub fn ping(&mut self) -> Result<()> {
        let result = self.expect_result(&CppCommand::Ping)?;
        if result.as_str() == Some("pong") {
            Ok(())
        } else {
            Err(CompatTestError::InvalidResponse(format!("{:?}", result)))
        }
    }

    /// Get full info from running node
    pub fn get_info(&mut self) -> Result<NodeInfo> {
        let result = self.expect_result(&CppCommand::GetInfo)?;
        Ok(NodeInfo {
            adnl_id: result["adnl_id"].as_str().unwrap_or_default().to_string(),
            pubkey: result["pubkey"].as_str().unwrap_or_default().to_string(),
            udp_port: result["udp_port"].as_u64().unwrap_or_default() as u16,
        })
    }

    // ---- Overlay ID ----

    /// Compute overlay ID from name bytes (raw bytes, will be base64-encoded)
    pub fn compute_overlay_id(&mut self, name: &[u8]) -> Result<String> {
        let result =
            self.expect_result(&CppCommand::ComputeOverlayId { name: b64_encode(name) })?;
        result["overlay_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected overlay_id".to_string()))
    }

    // ---- Peer management ----

    /// Add a peer to the ADNL peer table
    pub fn add_peer(&mut self, pubkey_tl_b64: &str, ip: &str, port: u16) -> Result<String> {
        let result = self.expect_result(&CppCommand::AddPeer {
            pubkey: pubkey_tl_b64.to_string(),
            ip: ip.to_string(),
            port,
        })?;
        result["peer_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected peer_id".to_string()))
    }

    // ---- Overlay creation ----

    /// Create a public overlay
    pub fn create_public_overlay(&mut self, overlay_name: &[u8]) -> Result<String> {
        let result = self.expect_result(&CppCommand::CreateOverlay {
            overlay_type: "public".to_string(),
            overlay_name: b64_encode(overlay_name),
            peers: vec![],
            root_pub_keys: vec![],
            certificate: None,
            max_slaves: None,
            enable_twostep: false,
        })?;
        result["overlay_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected overlay_id".to_string()))
    }

    /// Create a private overlay with given peer ADNL IDs (hex)
    pub fn create_private_overlay(
        &mut self,
        overlay_name: &[u8],
        peers: Vec<String>,
    ) -> Result<String> {
        let result = self.expect_result(&CppCommand::CreateOverlay {
            overlay_type: "private".to_string(),
            overlay_name: b64_encode(overlay_name),
            peers,
            root_pub_keys: vec![],
            certificate: None,
            max_slaves: None,
            enable_twostep: false,
        })?;
        result["overlay_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected overlay_id".to_string()))
    }

    /// Create a private overlay with TwostepFec enabled
    pub fn create_private_overlay_twostep(
        &mut self,
        overlay_name: &[u8],
        peers: Vec<String>,
    ) -> Result<String> {
        let result = self.expect_result(&CppCommand::CreateOverlay {
            overlay_type: "private".to_string(),
            overlay_name: b64_encode(overlay_name),
            peers,
            root_pub_keys: vec![],
            certificate: None,
            max_slaves: None,
            enable_twostep: true,
        })?;
        result["overlay_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected overlay_id".to_string()))
    }

    /// Create a semiprivate overlay
    pub fn create_semiprivate_overlay(
        &mut self,
        overlay_name: &[u8],
        peers: Vec<String>,
        root_pub_keys: Vec<String>,
        certificate: Option<&[u8]>,
        max_slaves: Option<i32>,
    ) -> Result<String> {
        let result = self.expect_result(&CppCommand::CreateOverlay {
            overlay_type: "semiprivate".to_string(),
            overlay_name: b64_encode(overlay_name),
            peers,
            root_pub_keys,
            certificate: certificate.map(b64_encode),
            max_slaves,
            enable_twostep: false,
        })?;
        result["overlay_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected overlay_id".to_string()))
    }

    /// Delete an overlay
    pub fn delete_overlay(&mut self, overlay_id: &str) -> Result<()> {
        self.expect_result(&CppCommand::DeleteOverlay { overlay_id: overlay_id.to_string() })?;
        Ok(())
    }

    // ---- Overlay node info ----

    /// Get TL-serialized overlay.node for this node in the given overlay
    /// Returns base64-encoded TL bytes
    pub fn get_overlay_node_info(&mut self, overlay_id: &str) -> Result<String> {
        let result = self.expect_result(&CppCommand::GetOverlayNodeInfo {
            overlay_id: overlay_id.to_string(),
        })?;
        result["node_tl"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected node_tl".to_string()))
    }

    // ---- Broadcasts ----

    /// Send a broadcast (optionally FEC)
    pub fn send_broadcast(&mut self, overlay_id: &str, data: &[u8], use_fec: bool) -> Result<()> {
        self.expect_result(&CppCommand::SendBroadcast {
            overlay_id: overlay_id.to_string(),
            data: b64_encode(data),
            use_fec,
        })?;
        Ok(())
    }

    /// Get received broadcasts for an overlay
    pub fn get_received_broadcasts(&mut self, overlay_id: &str) -> Result<Vec<ReceivedBroadcast>> {
        let result = self.expect_result(&CppCommand::GetReceivedBroadcasts {
            overlay_id: overlay_id.to_string(),
        })?;
        let broadcasts: Vec<ReceivedBroadcast> = serde_json::from_value(result)?;
        Ok(broadcasts)
    }

    /// Clear received broadcasts for an overlay
    pub fn clear_received_broadcasts(&mut self, overlay_id: &str) -> Result<()> {
        self.expect_result(&CppCommand::ClearReceivedBroadcasts {
            overlay_id: overlay_id.to_string(),
        })?;
        Ok(())
    }

    /// Set broadcast validator mode
    pub fn set_broadcast_validator(&mut self, overlay_id: &str, mode: &str) -> Result<()> {
        self.expect_result(&CppCommand::SetBroadcastValidator {
            overlay_id: overlay_id.to_string(),
            mode: mode.to_string(),
        })?;
        Ok(())
    }

    // ---- Queries ----

    /// Send an overlay query, returns answer bytes
    pub fn send_query(
        &mut self,
        overlay_id: &str,
        peer_adnl_id: &str,
        data: &[u8],
        timeout_ms: i64,
    ) -> Result<Vec<u8>> {
        let result = self.expect_result(&CppCommand::SendQuery {
            overlay_id: overlay_id.to_string(),
            peer_adnl_id: peer_adnl_id.to_string(),
            data: b64_encode(data),
            timeout_ms,
        })?;
        let answer_b64 = result["answer"]
            .as_str()
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected answer".to_string()))?;
        b64_decode(answer_b64)
            .map_err(|e| CompatTestError::InvalidResponse(format!("Invalid base64 answer: {}", e)))
    }

    /// Send an RLDP query via overlay
    pub fn send_rldp_query(
        &mut self,
        overlay_id: &str,
        peer_adnl_id: &str,
        data: &[u8],
        max_answer_size: u64,
        v2: bool,
    ) -> Result<Vec<u8>> {
        let result = self.expect_result(&CppCommand::SendRldpQuery {
            overlay_id: overlay_id.to_string(),
            peer_adnl_id: peer_adnl_id.to_string(),
            data: b64_encode(data),
            max_answer_size,
            v2,
        })?;
        let answer_b64 = result["answer"]
            .as_str()
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected answer".to_string()))?;
        b64_decode(answer_b64)
            .map_err(|e| CompatTestError::InvalidResponse(format!("Invalid base64 answer: {}", e)))
    }

    // ---- Point-to-point messages ----

    /// Send a point-to-point overlay message (not broadcast)
    pub fn send_message(
        &mut self,
        overlay_id: &str,
        peer_adnl_id: &str,
        data: &[u8],
    ) -> Result<()> {
        self.expect_result(&CppCommand::SendMessage {
            overlay_id: overlay_id.to_string(),
            peer_adnl_id: peer_adnl_id.to_string(),
            data: b64_encode(data),
        })?;
        Ok(())
    }

    /// Get received messages for an overlay
    pub fn get_received_messages(&mut self, overlay_id: &str) -> Result<Vec<ReceivedMessage>> {
        let result = self.expect_result(&CppCommand::GetReceivedMessages {
            overlay_id: overlay_id.to_string(),
        })?;
        let messages: Vec<ReceivedMessage> = serde_json::from_value(result)?;
        Ok(messages)
    }

    /// Clear received messages for an overlay
    pub fn clear_received_messages(&mut self, overlay_id: &str) -> Result<()> {
        self.expect_result(&CppCommand::ClearReceivedMessages {
            overlay_id: overlay_id.to_string(),
        })?;
        Ok(())
    }

    // ---- Queries ----

    /// Set query handler mode
    pub fn set_query_handler(&mut self, overlay_id: &str, mode: &str) -> Result<()> {
        self.expect_result(&CppCommand::SetQueryHandler {
            overlay_id: overlay_id.to_string(),
            mode: mode.to_string(),
        })?;
        Ok(())
    }

    // ---- BOC Compression ----

    /// Compress BOC data on the C++ side.
    /// Takes base64-encoded standard BOC, returns base64-encoded compressed data.
    pub fn compress_boc(&mut self, boc_b64: &str, algorithm: &str) -> Result<String> {
        let result = self.expect_result(&CppCommand::CompressBoc {
            data: boc_b64.to_string(),
            algorithm: algorithm.to_string(),
        })?;
        result["compressed"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected 'compressed'".to_string()))
    }

    /// Decompress BOC data on the C++ side.
    /// Takes base64-encoded compressed data, returns base64-encoded standard BOC.
    pub fn decompress_boc(&mut self, compressed_b64: &str, max_size: u32) -> Result<String> {
        let result = self.expect_result(&CppCommand::DecompressBoc {
            data: compressed_b64.to_string(),
            max_size,
        })?;
        result["boc"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected 'boc'".to_string()))
    }

    /// Build serialized TL bytes for consensus.candidateId(slot, hash) on C++ side.
    pub fn compute_candidate_id_to_sign(&mut self, slot: i32, hash_hex: &str) -> Result<Vec<u8>> {
        let result = self.expect_result(&CppCommand::ComputeCandidateIdToSign {
            slot,
            hash: hash_hex.to_string(),
        })?;
        let data_b64 = result["data"]
            .as_str()
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected 'data'".to_string()))?;
        b64_decode(data_b64)
            .map_err(|e| CompatTestError::InvalidResponse(format!("Invalid base64 data: {}", e)))
    }

    // ---- QUIC ----

    /// Enable QUIC transport (creates QuicSender, listens on udp_port + 1000)
    pub fn enable_quic(&mut self) -> Result<u16> {
        let result = self.expect_result(&CppCommand::EnableQuic {})?;
        let quic_port = result["quic_port"]
            .as_u64()
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected quic_port".to_string()))?;
        Ok(quic_port as u16)
    }

    /// Send a message via QUIC transport (bypasses overlay, goes through ADNL)
    pub fn send_quic_message(&mut self, peer_adnl_id: &str, data: &[u8]) -> Result<()> {
        self.expect_result(&CppCommand::SendQuicMessage {
            peer_adnl_id: peer_adnl_id.to_string(),
            data: b64_encode(data),
        })?;
        Ok(())
    }

    /// Send a query via QUIC transport, returns answer bytes
    pub fn send_quic_query(
        &mut self,
        peer_adnl_id: &str,
        data: &[u8],
        timeout_ms: i64,
    ) -> Result<Vec<u8>> {
        let result = self.expect_result(&CppCommand::SendQuicQuery {
            peer_adnl_id: peer_adnl_id.to_string(),
            data: b64_encode(data),
            timeout_ms,
        })?;
        let answer_b64 = result["answer"]
            .as_str()
            .ok_or_else(|| CompatTestError::InvalidResponse("Expected answer".to_string()))?;
        b64_decode(answer_b64)
            .map_err(|e| CompatTestError::InvalidResponse(format!("Invalid base64 answer: {}", e)))
    }

    // ---- Lifecycle ----

    /// Shutdown the node
    pub fn shutdown(&mut self) -> Result<()> {
        let _ = self.send_command(&CppCommand::Shutdown);
        let _ = self.process.wait();
        Ok(())
    }
}

impl Drop for CppTestNode {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Default test timeout in seconds. Can be overridden via TEST_TIMEOUT env var.
const DEFAULT_TEST_TIMEOUT_SECS: u64 = 90;

/// Guard that aborts the test process if it exceeds the timeout.
/// Create at the start of each test; the watchdog thread is cancelled on drop.
pub struct TestTimeout {
    cancel: Arc<AtomicBool>,
}

impl TestTimeout {
    /// Create a new test timeout guard.
    /// `timeout_secs` — maximum duration for the test; 0 means use the default (90s).
    /// The timeout can also be overridden globally via the `TEST_TIMEOUT` env var.
    pub fn new(timeout_secs: u64) -> Self {
        let secs = std::env::var("TEST_TIMEOUT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(if timeout_secs == 0 { DEFAULT_TEST_TIMEOUT_SECS } else { timeout_secs });

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = cancel.clone();
        let thread_name = thread::current().name().unwrap_or("unknown").to_string();

        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while Instant::now() < deadline {
                if cancel_clone.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            if !cancel_clone.load(Ordering::Relaxed) {
                eprintln!(
                    "\n\x1b[1;31mTEST TIMEOUT: '{}' exceeded {}s limit — aborting process\x1b[0m",
                    thread_name, secs
                );
                process::exit(1);
            }
        });

        Self { cancel }
    }
}

impl Drop for TestTimeout {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Skip test if C++ binary is not available
#[macro_export]
macro_rules! skip_if_no_cpp {
    () => {
        if !$crate::cpp_binary_available() {
            eprintln!("Skipping test: CPP_COMPAT_TEST_BIN not set");
            return;
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpp_binary_check() {
        // This just verifies the check works
        let _ = cpp_binary_available();
    }
}
