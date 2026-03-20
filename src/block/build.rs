/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::env;

mod common {
    include!("../common/build/build.rs");
    pub(crate) fn build() {
        main();
    }
}

fn main() {
    // Take care on wasm cross-compilation
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    if target_arch.ne("wasm32") {
        println!("cargo:rustc-cfg=feature=\"std\"");
    }
    common::build();
}
