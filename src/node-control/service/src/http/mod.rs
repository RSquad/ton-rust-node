/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod config_handlers;
pub mod http_server_task;
pub(crate) mod login_rate_limiter;

pub use http_server_task::run;

#[cfg(test)]
mod auth_tests;
