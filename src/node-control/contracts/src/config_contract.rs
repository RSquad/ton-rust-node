/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
/// Configuration contract implementation
mod config_impl;
/// Messages for configuration contract
pub mod messages;
/// Wrapper trait for configuration contract get-methods
mod wrapper;

pub use config_impl::ConfigContractImpl;
pub use wrapper::{ConfigContractWrapper, ConfigProposal, ProposalHash, ProposedParam};
