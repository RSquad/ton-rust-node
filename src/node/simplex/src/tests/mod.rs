/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Internal tests for Simplex consensus
//!
//! This module contains private unit tests that have access to internal crate symbols.
//! Tests are organized into submodules:
//!
//! - `test_crypto` - Threshold calculations, signature tests, hash computation
//! - `test_block` - Block candidate types and resolution
//! - `test_certificate` - Certificate types and TL serialization
//! - `test_restart` - Startup recovery, restart recommit strategies
//! - `test_simplex_state` - SimplexState FSM tests (included directly from `simplex_state.rs`
//!   via `#[path]` attribute to access private fields without pub(crate))
//! - `test_candidate_resolver` - CandidateResolverCache tests (included directly from `receiver.rs`
//!   via `#[path]` attribute to access private struct)
//!
//! ## Usage
//!
//! Run all simplex tests:
//! ```bash
//! cargo test -p simplex
//! ```
//!
//! Run specific test module:
//! ```bash
//! cargo test -p simplex tests::test_crypto::
//! cargo test -p simplex tests::test_block::
//! cargo test -p simplex tests::test_restart::
//! cargo test -p simplex simplex_state::tests::
//! cargo test -p simplex receiver::tests::
//! ```

#[cfg(test)]
mod test_block;
#[cfg(test)]
mod test_certificate;
#[cfg(test)]
mod test_crypto;
#[cfg(test)]
mod test_receiver;
#[cfg(test)]
mod test_restart;
#[cfg(test)]
mod test_session_description;
