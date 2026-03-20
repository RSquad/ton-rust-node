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
macro_rules! declare {
    ($mnemonic:ident, $value:expr) => {
        pub(super) const $mnemonic: u16 = $value;
    };
}

// Microfunction stuff ********************************************************

// How to address TVM objects using macros:
//
// CC                   - engine.cc
// ctrl!(i)             - c(i)
// savelist!(x, i)      - x.savelist(i), x is addressed independently
//                        and supposed to be a continuation
// stack!(i)            - cc.stack(i)
// var!(i)              - engine.current_command.vars(i)

#[macro_export]
macro_rules! address_tag {
    ($code:expr) => {
        $code & 0x0F00
    };
}

#[macro_export]
macro_rules! ctrl {
    ($index:expr) => {
        CTRL | ($index as u16)
    };
}

#[macro_export]
macro_rules! savelist {
    ($storage:expr, $index:expr) => {
        $storage | SAVELIST | (($index as u16) << 12)
    };
}

#[macro_export]
macro_rules! savelist_index {
    ($code:expr) => {
        (($code & 0xF000) >> 12) as usize
    };
}

#[macro_export]
macro_rules! storage_index {
    ($code:expr) => {
        ($code & 0x000F) as usize
    };
}

#[macro_export]
macro_rules! stack {
    ($index:expr) => {
        STACK | (($index & 0xFF) as u16)
    };
}

#[macro_export]
macro_rules! stack_index {
    ($code:expr) => {
        ($code & 0x00FF) as usize
    };
}

#[macro_export]
macro_rules! var {
    ($index:expr) => {
        VAR | ($index as u16)
    };
}

// Address tags
declare!(CC, 0x0000); // Current continuation
declare!(CTRL, 0x0100); // Control register
declare!(STACK, 0x0200); // Data stack
declare!(VAR, 0x0300); // Instruction variable
declare!(SAVELIST, 0x0800); // Savelist

// Data tags
declare!(BUILDER, 0x0000);
declare!(CELL, 0x0001);
declare!(CONTINUATION, 0x0002);
//declare!(INTEGER,      0x0003);
declare!(SLICE, 0x0004);

pub(super) const CC_SAVELIST: u16 = CC | SAVELIST;
pub(super) const CTRL_SAVELIST: u16 = CTRL | SAVELIST;
pub(super) const VAR_SAVELIST: u16 = VAR | SAVELIST;
