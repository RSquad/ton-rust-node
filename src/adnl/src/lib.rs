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
use crate::adnl::node::AdnlNode;
use std::sync::Arc;
use ton_block::Result;

mod adnl;
pub use crate::adnl::*;
mod dht;
pub use crate::dht::*;
mod overlay;
pub use crate::overlay::*;
mod quic;
pub use crate::quic::*;
mod rldp;
pub use crate::rldp::*;

pub struct NetworkStack {
    pub adnl: Arc<AdnlNode>,
    pub dht: Arc<DhtNode>,
    pub overlay: Arc<OverlayNode>,
    pub rldp: Arc<RldpNode>,
    pub quic: Option<Arc<QuicNode>>,
}

impl NetworkStack {
    pub async fn start_over_udp_tcp(&self) -> Result<()> {
        self.adnl
            .start_over_udp_tcp(vec![self.dht.clone(), self.overlay.clone(), self.rldp.clone()])
            .await
    }
    pub fn is_tcp_available(&self) -> bool {
        self.adnl.check_options(AdnlNode::OPTION_UDP_TCP)
    }
}

include!("../../common/src/info.rs");
