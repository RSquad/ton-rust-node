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
mod core;
pub(in crate::executor) mod data;
mod handlers;
#[macro_use]
pub(in crate::executor) mod storage;

pub use self::core::*;

#[cfg(test)]
#[path = "../../tests/test_microfunctions.rs"]
mod tests;
