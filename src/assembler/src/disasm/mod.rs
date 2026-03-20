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
use self::loader::Loader;
use ton_block::{Result, SliceData};

pub mod codedict;
pub mod fmt;
mod handlers;
pub mod loader;
pub mod types;

pub fn disasm(slice: &mut SliceData) -> Result<String> {
    disasm_ex(slice, false)
}

pub fn disasm_ex(slice: &mut SliceData, collapsed: bool) -> Result<String> {
    let mut loader = Loader::new(collapsed);
    let mut code = loader.load(slice, false)?;
    code.elaborate_dictpushconst_dictugetjmp();
    Ok(code.print("", true, 0))
}
