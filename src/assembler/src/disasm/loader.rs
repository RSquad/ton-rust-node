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
use super::{
    handlers::Handlers,
    types::{Code, Instruction, InstructionParameter, OperationBehavior},
};
use num_traits::Zero;
use std::{cmp::Ordering, collections::HashMap, ops::Not};
use ton_block::{fail, Cell, Result, SliceData, UInt256};

macro_rules! create_handler_1 {
    ($func_name:ident, $opc:literal, $mnemonic:literal) => {
        pub(super) fn $func_name(&mut self, slice: &mut SliceData) -> Result<Instruction> {
            let opc = slice.get_next_int(8)?;
            if opc != $opc {
                fail!("invalid opcode");
            }
            Ok(Instruction::new($mnemonic))
        }
    };
}

macro_rules! create_handler_1t {
    ($func_name:ident, $opc:literal, $mnemonic:literal) => {
        pub(super) fn $func_name<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
        where
            T: OperationBehavior,
        {
            let opc = slice.get_next_int(8)?;
            if opc != $opc {
                fail!("invalid opcode");
            }
            Ok(T::insn(Instruction::new($mnemonic)))
        }
    };
}

macro_rules! create_handler_2 {
    ($func_name:ident, $opc:literal, $mnemonic:literal) => {
        pub(super) fn $func_name(&mut self, slice: &mut SliceData) -> Result<Instruction> {
            let opc = slice.get_next_int(16)?;
            if opc != $opc {
                fail!("invalid opcode");
            }
            Ok(Instruction::new($mnemonic))
        }
    };
}

macro_rules! create_handler_2t {
    ($func_name:ident, $opc:literal, $mnemonic:literal) => {
        pub(super) fn $func_name<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
        where
            T: OperationBehavior,
        {
            let opc = slice.get_next_int(16)?;
            if opc != $opc {
                fail!("invalid opcode");
            }
            Ok(T::insn(Instruction::new($mnemonic)))
        }
    };
}

macro_rules! create_handler_2r {
    ($func_name:ident, $opc:literal, $mnemonic:literal) => {
        pub(super) fn $func_name(&mut self, slice: &mut SliceData) -> Result<Instruction> {
            let opc = slice.get_next_int(16)?;
            if opc != $opc {
                fail!("invalid opcode");
            }
            let cell = slice.checked_drain_reference().ok();
            let code = self.load_cell(&cell)?;
            Ok(Instruction::new($mnemonic)
                .with_refs(1)
                .with_param(InstructionParameter::Code { code, cell }))
        }
    };
}

macro_rules! create_handler_3r {
    ($func_name:ident, $opc:literal, $mnemonic:literal) => {
        pub(super) fn $func_name(&mut self, slice: &mut SliceData) -> Result<Instruction> {
            let opc = slice.get_next_int(16)?;
            if opc != $opc {
                fail!("invalid opcode");
            }
            let cell1 = slice.checked_drain_reference().ok();
            let cell2 = slice.checked_drain_reference().ok();
            let code1 = self.load_cell(&cell1)?;
            let code2 = self.load_cell(&cell2)?;
            Ok(Instruction::new($mnemonic)
                .with_refs(2)
                .with_param(InstructionParameter::Code { code: code1, cell: cell1 })
                .with_param(InstructionParameter::Code { code: code2, cell: cell2 }))
        }
    };
}

macro_rules! check {
    ($expr:expr) => {
        if !$expr {
            ton_block::fail!("check failed {}:{}", file!(), line!())
        }
    };
}

macro_rules! check_eq {
    ($lhs:expr, $rhs:literal) => {
        if $lhs != $rhs {
            ton_block::fail!("check failed {}:{}", file!(), line!())
        }
    };
}

fn comment_missing_bits_and_refs(insn: &mut Instruction, missing_bits: usize, missing_refs: usize) {
    if missing_bits > 0 || missing_refs > 0 {
        let mut comment = String::from("missing");
        if missing_bits > 0 {
            let plural = if missing_bits > 1 { "s" } else { "" };
            comment += &format!(" {} bit{}", missing_bits, plural);
        }
        if missing_bits > 0 && missing_refs > 0 {
            comment += " and";
        }
        if missing_refs > 0 {
            let plural = if missing_refs > 1 { "s" } else { "" };
            comment += &format!(" {} ref{}", missing_refs, plural);
        }
        insn.set_comment(comment)
    }
}

pub struct Loader {
    handlers: Handlers,
    collapse: bool,
    history: HashMap<UInt256, Code>,
}

impl Loader {
    pub fn new(collapse: bool) -> Self {
        Self { handlers: Handlers::new_code_page_0(), collapse, history: HashMap::new() }
    }
    pub fn load(&mut self, slice: &mut SliceData, inline: bool) -> Result<Code> {
        let orig_slice = slice.clone();
        let mut code = match self.load_slice(slice) {
            Ok(code) => code,
            Err(_) => {
                // failed to load the slice - emit it as-is
                let mut insns = Code::single(
                    Instruction::new(".blob")
                        .with_param(InstructionParameter::Slice(orig_slice.clone())),
                );
                for i in 0..orig_slice.remaining_references() {
                    insns.push(Instruction::new(".cell").with_param(InstructionParameter::Cell {
                        cell: orig_slice.reference(i).ok(),
                        collapsed: false,
                    }))
                }
                return Ok(insns);
            }
        };
        match slice.remaining_references().cmp(&1) {
            Ordering::Less => (),
            Ordering::Equal => {
                let next_cell = slice.checked_drain_reference().ok();
                let mut next_code = self.load_cell(&next_cell)?;
                if inline {
                    code.append(&mut next_code)
                } else {
                    let next =
                        Instruction::new("IMPLICIT-JMP").with_param(InstructionParameter::Code {
                            code: next_code,
                            cell: next_cell,
                        });
                    code.push(next)
                }
            }
            Ordering::Greater => fail!("two or more remaining references"),
        }
        Ok(code)
    }
    fn load_slice(&mut self, slice: &mut SliceData) -> Result<Code> {
        let mut code = Code::new();
        while slice.remaining_bits() > 0 {
            let mut bytecode = slice.clone();
            let handler = self.handlers.get_handler(&mut slice.clone())?;
            let mut insn = handler(self, slice)?;
            assert_eq!(bytecode.cell()?, slice.cell()?);
            let bits = bytecode.remaining_bits() - slice.remaining_bits();
            let refs = bytecode.remaining_references() - slice.remaining_references();
            bytecode.shrink_data(..bits);
            bytecode.shrink_references(..refs);
            insn.set_bytecode(bytecode);
            code.push(insn);
        }
        Ok(code)
    }
    fn load_cell(&mut self, cell: &Option<Cell>) -> Result<Code> {
        if let Some(cell) = cell {
            self.load_cell_impl(cell)
        } else {
            Ok(Code::single(Instruction::new(";; missing cell")))
        }
    }
    fn load_cell_impl(&mut self, cell: &Cell) -> Result<Code> {
        if let Some(code) = self.history.get(&cell.repr_hash()) {
            if self.collapse {
                Ok(Code::single(Instruction::new(";;").with_param(InstructionParameter::Cell {
                    cell: Some(cell.clone()),
                    collapsed: true,
                })))
            } else {
                Ok(code.clone())
            }
        } else {
            let code =
                self.load(&mut SliceData::load_cell_ref(cell)?, false).unwrap_or_else(|_| {
                    // failed to load the cell - emit it as-is
                    Code::single(Instruction::new(".cell").with_param(InstructionParameter::Cell {
                        cell: Some(cell.clone()),
                        collapsed: false,
                    }))
                });
            self.history.insert(cell.repr_hash(), code.clone());
            Ok(code)
        }
    }
    pub(super) fn unknown(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        fail!("unknown opcode {}", slice.as_hex_string())
    }
    pub(super) fn setcp(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xff);
        let cp = slice.get_next_byte()?;
        Ok(Instruction::new("SETCP").with_param(InstructionParameter::Integer(cp as isize)))
    }
    create_handler_2!(setcpx, 0xfff0, "SETCPX");
    create_handler_1!(nop, 0x00, "NOP");
    pub(super) fn xchg_simple(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(4)?;
        check!(opc == 0 || opc == 1);
        let i = slice.get_next_int(4)? as isize;
        match opc {
            0 => Ok(Instruction::new("XCHG").with_param(InstructionParameter::StackRegister(i))),
            1 => {
                Ok(Instruction::new("XCHG")
                    .with_param(InstructionParameter::StackRegisterPair(1, i)))
            }
            _ => fail!("unknown opcode"),
        }
    }
    pub(super) fn xchg_std(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x10);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XCHG").with_param(InstructionParameter::StackRegisterPair(i, j)))
    }
    pub(super) fn xchg_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x11);
        let ii = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("XCHG").with_param(InstructionParameter::StackRegisterPair(0, ii)))
    }
    pub(super) fn push_simple(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(4)?;
        check_eq!(opc, 0x2);
        let i = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PUSH").with_param(InstructionParameter::StackRegister(i)))
    }
    pub(super) fn pop_simple(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(4)?;
        check_eq!(opc, 0x3);
        let i = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("POP").with_param(InstructionParameter::StackRegister(i)))
    }
    pub(super) fn xchg3(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(4)?;
        check_eq!(opc, 0x4);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XCHG3").with_param(InstructionParameter::StackRegisterTriple(i, j, k)))
    }
    pub(super) fn xchg2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x50);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XCHG2").with_param(InstructionParameter::StackRegisterPair(i, j)))
    }
    pub(super) fn xcpu(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x51);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XCPU").with_param(InstructionParameter::StackRegisterPair(i, j)))
    }
    pub(super) fn puxc(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x52);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PUXC").with_param(InstructionParameter::StackRegisterPair(i, j - 1)))
    }
    pub(super) fn push2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x53);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PUSH2").with_param(InstructionParameter::StackRegisterPair(i, j)))
    }
    pub(super) fn xc2pu(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x541);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XC2PU").with_param(InstructionParameter::StackRegisterTriple(i, j, k)))
    }
    pub(super) fn xcpuxc(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x542);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XCPUXC").with_param(InstructionParameter::StackRegisterTriple(
            i,
            j,
            k - 1,
        )))
    }
    pub(super) fn xcpu2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x543);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("XCPU2").with_param(InstructionParameter::StackRegisterTriple(i, j, k)))
    }
    pub(super) fn puxc2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x544);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PUXC2").with_param(InstructionParameter::StackRegisterTriple(
            i,
            j - 1,
            k - 1,
        )))
    }
    pub(super) fn puxcpu(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x545);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PUXCPU").with_param(InstructionParameter::StackRegisterTriple(
            i,
            j - 1,
            k - 1,
        )))
    }
    pub(super) fn pu2xc(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x546);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PU2XC").with_param(InstructionParameter::StackRegisterTriple(
            i,
            j - 1,
            k - 2,
        )))
    }
    pub(super) fn push3(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x547);
        let i = slice.get_next_int(4)? as isize;
        let j = slice.get_next_int(4)? as isize;
        let k = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("PUSH3").with_param(InstructionParameter::StackRegisterTriple(i, j, k)))
    }
    pub(super) fn blkswap(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x55);
        let i = slice.get_next_int(4)? as usize;
        let j = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("BLKSWAP")
            .with_param(InstructionParameter::LengthAndIndex(i + 1, j + 1)))
    }
    pub(super) fn push(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x56);
        let ii = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("PUSH").with_param(InstructionParameter::StackRegister(ii)))
    }
    pub(super) fn pop(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x57);
        let ii = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("POP").with_param(InstructionParameter::StackRegister(ii)))
    }
    create_handler_1!(rot, 0x58, "ROT");
    create_handler_1!(rotrev, 0x59, "ROTREV");
    create_handler_1!(swap2, 0x5a, "SWAP2");
    create_handler_1!(drop2, 0x5b, "DROP2");
    create_handler_1!(dup2, 0x5c, "DUP2");
    create_handler_1!(over2, 0x5d, "OVER2");
    pub(super) fn reverse(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x5e);
        let i = slice.get_next_int(4)? as usize;
        let j = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("REVERSE").with_param(InstructionParameter::LengthAndIndex(i + 2, j)))
    }
    pub(super) fn blkdrop(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x5f0);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("BLKDROP").with_param(InstructionParameter::Length(i)))
    }
    pub(super) fn blkpush(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x5f);
        let i = slice.get_next_int(4)? as usize;
        let j = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("BLKPUSH").with_param(InstructionParameter::LengthAndIndex(i, j)))
    }
    create_handler_1!(pick, 0x60, "PICK");
    create_handler_1!(rollx, 0x61, "ROLLX");
    create_handler_1!(rollrevx, 0x62, "ROLLREVX");
    create_handler_1!(blkswx, 0x63, "BLKSWX");
    create_handler_1!(revx, 0x64, "REVX");
    create_handler_1!(dropx, 0x65, "DROPX");
    create_handler_1!(tuck, 0x66, "TUCK");
    create_handler_1!(xchgx, 0x67, "XCHGX");
    create_handler_1!(depth, 0x68, "DEPTH");
    create_handler_1!(chkdepth, 0x69, "CHKDEPTH");
    create_handler_1!(onlytopx, 0x6a, "ONLYTOPX");
    create_handler_1!(onlyx, 0x6b, "ONLYX");
    pub(super) fn blkdrop2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x6c);
        let i = slice.get_next_int(4)? as usize;
        check!(i > 0);
        let j = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("BLKDROP2").with_param(InstructionParameter::LengthAndIndex(i, j)))
    }
    create_handler_1!(null, 0x6d, "NULL");
    create_handler_1!(isnull, 0x6e, "ISNULL");
    pub(super) fn tuple_create(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f0);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("TUPLE").with_param(InstructionParameter::Length(k)))
    }
    pub(super) fn tuple_index(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f1);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("INDEX").with_param(InstructionParameter::Length(k)))
    }
    pub(super) fn tuple_un(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f2);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("UNTUPLE").with_param(InstructionParameter::Length(k)))
    }
    pub(super) fn tuple_unpackfirst(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f3);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("UNPACKFIRST").with_param(InstructionParameter::Length(k)))
    }
    pub(super) fn tuple_explode(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f4);
        let n = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("EXPLODE").with_param(InstructionParameter::Length(n)))
    }
    pub(super) fn tuple_setindex(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f5);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SETINDEX").with_param(InstructionParameter::Length(k)))
    }
    pub(super) fn tuple_index_quiet(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f6);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("INDEXQ").with_param(InstructionParameter::Length(k)))
    }
    pub(super) fn tuple_setindex_quiet(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6f7);
        let k = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SETINDEXQ").with_param(InstructionParameter::Length(k)))
    }
    create_handler_2!(tuple_createvar, 0x6f80, "TUPLEVAR");
    create_handler_2!(tuple_indexvar, 0x6f81, "INDEXVAR");
    create_handler_2!(tuple_untuplevar, 0x6f82, "UNTUPLEVAR");
    create_handler_2!(tuple_unpackfirstvar, 0x6f83, "UNPACKFIRSTVAR");
    create_handler_2!(tuple_explodevar, 0x6f84, "EXPLODEVAR");
    create_handler_2!(tuple_setindexvar, 0x6f85, "SETINDEXVAR");
    create_handler_2!(tuple_indexvar_quiet, 0x6f86, "INDEXVARQ");
    create_handler_2!(tuple_setindexvar_quiet, 0x6f87, "SETINDEXVARQ");
    create_handler_2!(tuple_len, 0x6f88, "TLEN");
    create_handler_2!(tuple_len_quiet, 0x6f89, "QTLEN");
    create_handler_2!(istuple, 0x6f8a, "ISTUPLE");
    create_handler_2!(tuple_last, 0x6f8b, "LAST");
    create_handler_2!(tuple_push, 0x6f8c, "TPUSH");
    create_handler_2!(tuple_pop, 0x6f8d, "TPOP");
    create_handler_2!(nullswapif, 0x6fa0, "NULLSWAPIF");
    create_handler_2!(nullswapifnot, 0x6fa1, "NULLSWAPIFNOT");
    create_handler_2!(nullrotrif, 0x6fa2, "NULLROTRIF");
    create_handler_2!(nullrotrifnot, 0x6fa3, "NULLROTRIFNOT");
    create_handler_2!(nullswapif2, 0x6fa4, "NULLSWAPIF2");
    create_handler_2!(nullswapifnot2, 0x6fa5, "NULLSWAPIFNOT2");
    create_handler_2!(nullrotrif2, 0x6fa6, "NULLROTRIF2");
    create_handler_2!(nullrotrifnot2, 0x6fa7, "NULLROTRIFNOT2");
    pub(super) fn tuple_index2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0x6fb);
        let i = slice.get_next_int(2)? as isize;
        let j = slice.get_next_int(2)? as isize;
        Ok(Instruction::new("INDEX2").with_param(InstructionParameter::StackRegisterPair(i, j)))
    }
    pub(super) fn tuple_index3(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0x6fe);
        let i = slice.get_next_int(2)? as isize;
        let j = slice.get_next_int(2)? as isize;
        let k = slice.get_next_int(2)? as isize;
        Ok(Instruction::new("INDEX3")
            .with_param(InstructionParameter::StackRegisterTriple(i, j, k)))
    }
    pub(super) fn pushint(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        assert!((0x70..0x82).contains(&opc));
        let mut x: i16 = 0;
        if opc <= 0x7a {
            x = opc as i16 - 0x70;
        } else if opc < 0x80 {
            x = opc as i16 - 0x80;
        } else if opc == 0x80 {
            x = (slice.get_next_int(8)? as i8) as i16;
        } else if opc == 0x81 {
            x = slice.get_next_int(16)? as i16;
        }
        Ok(Instruction::new("PUSHINT").with_param(InstructionParameter::Integer(x as isize)))
    }
    // adapted from vm/src/stack/integer/conversion.rs
    fn bigint(&mut self, slice: &mut SliceData) -> Result<num::BigInt> {
        fn twos_complement(digits: &mut Vec<u32>) {
            let mut carry = true;
            for d in digits {
                *d = d.not();
                if carry {
                    *d = d.wrapping_add(1);
                    carry = d.is_zero();
                }
            }
        }
        let first_byte = slice.get_next_byte()?;
        let byte_len = ((first_byte & 0b11111000u8) as usize >> 3) + 3;
        let greatest3bits = (first_byte & 0b111) as u32;
        let digit_count = (byte_len + 3) >> 2;
        let mut digits: Vec<u32> = vec![0; digit_count];
        let (sign, mut value) = if greatest3bits & 0b100 == 0 {
            (num::bigint::Sign::Plus, greatest3bits)
        } else {
            (num::bigint::Sign::Minus, 0xFFFF_FFF8u32 | greatest3bits)
        };

        let mut upper = byte_len & 0b11;
        if upper == 0 {
            upper = 4;
        }
        for _ in 1..upper {
            value <<= 8;
            value |= slice.get_next_byte()? as u32;
        }
        let last_index = digit_count - 1;
        digits[last_index] = value;

        for i in (0..last_index).rev() {
            let mut value = (slice.get_next_byte()? as u32) << 24;
            value |= (slice.get_next_byte()? as u32) << 16;
            value |= (slice.get_next_byte()? as u32) << 8;
            value |= slice.get_next_byte()? as u32;

            digits[i] = value;
        }

        if sign == num::bigint::Sign::Minus {
            twos_complement(&mut digits);
        }
        Ok(num::BigInt::new(sign, digits))
    }
    pub(super) fn pushint_big(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x82);
        let int = self.bigint(slice)?;
        Ok(Instruction::new("PUSHINT").with_param(InstructionParameter::BigInteger(int)))
    }
    pub(super) fn pushpow2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x83);
        let xx = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("PUSHPOW2").with_param(InstructionParameter::Integer(xx + 1)))
    }
    create_handler_2!(pushnan, 0x83ff, "PUSHNAN");
    pub(super) fn pushpow2dec(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x84);
        let xx = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("PUSHPOW2DEC").with_param(InstructionParameter::Integer(xx + 1)))
    }
    pub(super) fn pushnegpow2(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x85);
        let xx = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("PUSHNEGPOW2").with_param(InstructionParameter::Integer(xx + 1)))
    }
    pub(super) fn pushref(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x88);
        let cell = slice.checked_drain_reference().ok();
        Ok(Instruction::new("PUSHREF")
            .with_refs(1)
            .with_param(InstructionParameter::Cell { cell, collapsed: false }))
    }
    pub(super) fn pushrefslice(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x89);
        let cell = slice.checked_drain_reference().ok();
        Ok(Instruction::new("PUSHREFSLICE")
            .with_refs(1)
            .with_param(InstructionParameter::Cell { cell, collapsed: false }))
    }
    pub(super) fn pushrefcont(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x8a);
        let cell = slice.checked_drain_reference().ok();
        let code = self.load_cell(&cell)?;
        Ok(Instruction::new("PUSHREFCONT")
            .with_refs(1)
            .with_param(InstructionParameter::Code { code, cell }))
    }
    pub(super) fn pushslice_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x8b);
        let x = slice.get_next_int(4)? as usize;
        let mut bitstring = slice.get_next_slice(x * 8 + 4)?;
        bitstring.trim_right();
        Ok(Instruction::new("PUSHSLICE").with_param(InstructionParameter::Slice(bitstring)))
    }
    pub(super) fn pushslice_mid(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x8c);
        let r = slice.get_next_int(2)? as usize;
        let xx = slice.get_next_int(5)? as usize;
        let bits = xx * 8 + 1;

        let mut subslice = slice.clone();
        subslice.shrink_data(..bits);
        subslice.shrink_references(..r);
        subslice.trim_right();
        let missing_bits = bits - subslice.remaining_bits();
        let missing_refs = r - subslice.remaining_references();

        slice.shrink_data(bits..);
        slice.shrink_references(r..);

        let mut insn = Instruction::new("PUSHSLICE")
            .with_refs(r)
            .with_param(InstructionParameter::Slice(subslice));
        comment_missing_bits_and_refs(&mut insn, missing_bits, missing_refs);
        Ok(insn)
    }
    pub(super) fn pushslice_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0x8d);
        let r = slice.get_next_int(3)? as usize;
        let xx = slice.get_next_int(7)? as usize;
        let bits = xx * 8 + 6;

        let mut subslice = slice.clone();
        subslice.shrink_data(..bits);
        subslice.shrink_references(..r);
        subslice.trim_right();
        let missing_bits = bits - subslice.remaining_bits();
        let missing_refs = r - subslice.remaining_references();

        slice.shrink_data(bits..);
        slice.shrink_references(r..);

        let mut insn = Instruction::new("PUSHSLICE")
            .with_refs(r)
            .with_param(InstructionParameter::Slice(subslice));
        comment_missing_bits_and_refs(&mut insn, missing_bits, missing_refs);
        Ok(insn)
    }
    pub(super) fn pushcont_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(7)?;
        check_eq!(opc << 1, 0x8e);
        let r = slice.get_next_int(2)? as usize;
        let xx = slice.get_next_int(7)? as usize;
        let bits = xx * 8;

        let mut subslice = slice.clone();
        subslice.shrink_data(..bits);
        subslice.shrink_references(..r);
        let missing_bits = bits - subslice.remaining_bits();
        let missing_refs = r - subslice.remaining_references();
        let code = self.load(&mut subslice, false)?;

        slice.shrink_data(bits..);
        slice.shrink_references(r..);

        let mut insn = Instruction::new("PUSHCONT")
            .with_refs(r)
            .with_param(InstructionParameter::Code { code, cell: None });
        comment_missing_bits_and_refs(&mut insn, missing_bits, missing_refs);
        Ok(insn)
    }
    pub(super) fn pushcont_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(4)?;
        check_eq!(opc, 0x9);
        let x = slice.get_next_int(4)? as usize;
        let mut body = slice.get_next_slice(x * 8)?;
        let code = self.load(&mut body, true)?;
        Ok(Instruction::new("PUSHCONT").with_param(InstructionParameter::Code { code, cell: None }))
    }
    create_handler_1t!(add, 0xa0, "ADD");
    create_handler_1t!(sub, 0xa1, "SUB");
    create_handler_1t!(subr, 0xa2, "SUBR");
    create_handler_1t!(negate, 0xa3, "NEGATE");
    create_handler_1t!(inc, 0xa4, "INC");
    create_handler_1t!(dec, 0xa5, "DEC");
    pub(super) fn addconst<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xa6);
        let cc = slice.get_next_int(8)? as i8;
        Ok(T::insn(Instruction::new("ADDCONST"))
            .with_param(InstructionParameter::Integer(cc as isize)))
    }
    pub(super) fn mulconst<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xa7);
        let cc = slice.get_next_int(8)? as i8;
        Ok(T::insn(Instruction::new("MULCONST"))
            .with_param(InstructionParameter::Integer(cc as isize)))
    }
    create_handler_1t!(mul, 0xa8, "MUL");
    pub(super) fn divmod<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xa9);
        let opc2 = slice.get_next_int(8)?;
        match opc2 {
            0x04 => Ok(T::insn(Instruction::new("DIV"))),
            0x05 => Ok(T::insn(Instruction::new("DIVR"))),
            0x06 => Ok(T::insn(Instruction::new("DIVC"))),
            0x08 => Ok(T::insn(Instruction::new("MOD"))),
            0x0c => Ok(T::insn(Instruction::new("DIVMOD"))),
            0x0d => Ok(T::insn(Instruction::new("DIVMODR"))),
            0x0e => Ok(T::insn(Instruction::new("DIVMODC"))),
            0x24 => Ok(T::insn(Instruction::new("RSHIFT"))),
            0x34 => {
                let tt = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("RSHIFT"))
                    .with_param(InstructionParameter::Length(tt + 1)))
            }
            0x38 => {
                let tt = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("MODPOW2"))
                    .with_param(InstructionParameter::Length(tt + 1)))
            }
            0x84 => Ok(T::insn(Instruction::new("MULDIV"))),
            0x85 => Ok(T::insn(Instruction::new("MULDIVR"))),
            0x8c => Ok(T::insn(Instruction::new("MULDIVMOD"))),
            0xa4 => Ok(T::insn(Instruction::new("MULRSHIFT"))),
            0xa5 => Ok(T::insn(Instruction::new("MULRSHIFTR"))),
            0xb4 => {
                let tt = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("MULRSHIFT"))
                    .with_param(InstructionParameter::Length(tt + 1)))
            }
            0xb5 => {
                let tt = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("MULRSHIFTR"))
                    .with_param(InstructionParameter::Length(tt + 1)))
            }
            0xc4 => Ok(T::insn(Instruction::new("LSHIFTDIV"))),
            0xc5 => Ok(T::insn(Instruction::new("LSHIFTDIVR"))),
            0xd4 => {
                let tt = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("LSHIFTDIV"))
                    .with_param(InstructionParameter::Length(tt + 1)))
            }
            0xd5 => {
                let tt = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("LSHIFTDIVR"))
                    .with_param(InstructionParameter::Length(tt + 1)))
            }
            _ => {
                fail!("unknown divmod kind");
            }
        }
    }
    pub(super) fn lshift<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        match opc {
            0xaa => {
                let cc = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("LSHIFT"))
                    .with_param(InstructionParameter::Length(cc + 1)))
            }
            0xac => Ok(T::insn(Instruction::new("LSHIFT"))),
            _ => fail!("unknown lshift kind"),
        }
    }
    pub(super) fn rshift<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        match opc {
            0xab => {
                let cc = slice.get_next_int(8)? as usize;
                Ok(T::insn(Instruction::new("RSHIFT"))
                    .with_param(InstructionParameter::Length(cc + 1)))
            }
            0xad => Ok(T::insn(Instruction::new("RSHIFT"))),
            _ => fail!("unknown rshift kind"),
        }
    }
    create_handler_1t!(pow2, 0xae, "POW2");
    create_handler_1t!(and, 0xb0, "AND");
    create_handler_1t!(or, 0xb1, "OR");
    create_handler_1t!(xor, 0xb2, "XOR");
    create_handler_1t!(not, 0xb3, "NOT");
    pub(super) fn fits<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xb4);
        let cc = slice.get_next_int(8)? as usize;
        Ok(T::insn(Instruction::new("FITS")).with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn ufits<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xb5);
        let cc = slice.get_next_int(8)? as usize;
        Ok(T::insn(Instruction::new("UFITS")).with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_2t!(fitsx, 0xb600, "FITSX");
    create_handler_2t!(ufitsx, 0xb601, "UFITSX");
    create_handler_2t!(bitsize, 0xb602, "BITSIZE");
    create_handler_2t!(ubitsize, 0xb603, "UBITSIZE");
    create_handler_2t!(min, 0xb608, "MIN");
    create_handler_2t!(max, 0xb609, "MAX");
    create_handler_2t!(minmax, 0xb60a, "MINMAX");
    create_handler_2t!(abs, 0xb60b, "ABS");
    create_handler_1t!(sgn, 0xb8, "SGN");
    create_handler_1t!(less, 0xb9, "LESS");
    create_handler_1t!(equal, 0xba, "EQUAL");
    create_handler_1t!(leq, 0xbb, "LEQ");
    create_handler_1t!(greater, 0xbc, "GREATER");
    create_handler_1t!(neq, 0xbd, "NEQ");
    create_handler_1t!(geq, 0xbe, "GEQ");
    create_handler_1t!(cmp, 0xbf, "CMP");
    pub(super) fn eqint<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xc0);
        let yy = (slice.get_next_int(8)? as i8) as isize;
        Ok(T::insn(Instruction::new("EQINT")).with_param(InstructionParameter::Integer(yy)))
    }
    pub(super) fn lessint<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xc1);
        let yy = (slice.get_next_int(8)? as i8) as isize;
        Ok(T::insn(Instruction::new("LESSINT")).with_param(InstructionParameter::Integer(yy)))
    }
    pub(super) fn gtint<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xc2);
        let yy = (slice.get_next_int(8)? as i8) as isize;
        Ok(T::insn(Instruction::new("GTINT")).with_param(InstructionParameter::Integer(yy)))
    }
    pub(super) fn neqint<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xc3);
        let yy = (slice.get_next_int(8)? as i8) as isize;
        Ok(T::insn(Instruction::new("NEQINT")).with_param(InstructionParameter::Integer(yy)))
    }
    create_handler_1!(isnan, 0xc4, "ISNAN");
    create_handler_1!(chknan, 0xc5, "CHKNAN");
    create_handler_2!(sempty, 0xc700, "SEMPTY");
    create_handler_2!(sdempty, 0xc701, "SDEMPTY");
    create_handler_2!(srempty, 0xc702, "SREMPTY");
    create_handler_2!(sdfirst, 0xc703, "SDFIRST");
    create_handler_2!(sdlexcmp, 0xc704, "SDLEXCMP");
    create_handler_2!(sdeq, 0xc705, "SDEQ");
    create_handler_2!(sdpfx, 0xc708, "SDPFX");
    create_handler_2!(sdpfxrev, 0xc709, "SDPFXREV");
    create_handler_2!(sdppfx, 0xc70a, "SDPPFX");
    create_handler_2!(sdppfxrev, 0xc70b, "SDPPFXREV");
    create_handler_2!(sdsfx, 0xc70c, "SDSFX");
    create_handler_2!(sdsfxrev, 0xc70d, "SDSFXREV");
    create_handler_2!(sdpsfx, 0xc70e, "SDPSFX");
    create_handler_2!(sdpsfxrev, 0xc70f, "SDPSFXREV");
    create_handler_2!(sdcntlead0, 0xc710, "SDCNTLEAD0");
    create_handler_2!(sdcntlead1, 0xc711, "SDCNTLEAD1");
    create_handler_2!(sdcnttrail0, 0xc712, "SDCNTTRAIL0");
    create_handler_2!(sdcnttrail1, 0xc713, "SDCNTTRAIL1");
    create_handler_1!(newc, 0xc8, "NEWC");
    create_handler_1!(endc, 0xc9, "ENDC");
    pub(super) fn sti(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xca);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STI").with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn stu(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xcb);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STU").with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_1!(stref, 0xcc, "STREF");
    create_handler_1!(endcst, 0xcd, "STBREFR");
    create_handler_1!(stslice, 0xce, "STSLICE");
    create_handler_2!(stix, 0xcf00, "STIX");
    create_handler_2!(stux, 0xcf01, "STUX");
    create_handler_2!(stixr, 0xcf02, "STIXR");
    create_handler_2!(stuxr, 0xcf03, "STUXR");
    create_handler_2!(stixq, 0xcf04, "STIXQ");
    create_handler_2!(stuxq, 0xcf05, "STUXQ");
    create_handler_2!(stixrq, 0xcf06, "STIXRQ");
    create_handler_2!(stuxrq, 0xcf07, "STUXRQ");
    pub(super) fn stir(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf0a);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STIR").with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn stur(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf0b);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STUR").with_param(InstructionParameter::Length(cc + 1)))
    }

    pub(super) fn stiq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf0c);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STI").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn stuq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf0d);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STU").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn stirq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf0e);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STIR").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn sturq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf0f);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("STUR").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_2!(stbref, 0xcf11, "STBREF");
    create_handler_2!(stb, 0xcf13, "STB");
    create_handler_2!(strefr, 0xcf14, "STREFR");
    create_handler_2!(stslicer, 0xcf16, "STSLICER");
    create_handler_2!(stbr, 0xcf17, "STBR");
    create_handler_2!(strefq, 0xcf18, "STREFQ");
    create_handler_2!(stbrefq, 0xcf19, "STBREFQ");
    create_handler_2!(stsliceq, 0xcf1a, "STSLICEQ");
    create_handler_2!(stbq, 0xcf1b, "STBQ");
    create_handler_2!(strefrq, 0xcf1c, "STREFRQ");
    create_handler_2!(stbrefrq, 0xcf1d, "STBREFRQ");
    create_handler_2!(stslicerq, 0xcf1e, "STSLICERQ");
    create_handler_2!(stbrq, 0xcf1f, "STBRQ");
    create_handler_2!(strefconst, 0xcf20, "STREFCONST");
    create_handler_2!(stref2const, 0xcf21, "STREF2CONST");
    create_handler_2!(endxc, 0xcf23, "ENDXC");
    create_handler_2!(stile4, 0xcf28, "STILE4");
    create_handler_2!(stule4, 0xcf29, "STULE4");
    create_handler_2!(stile8, 0xcf2a, "STILE8");
    create_handler_2!(stule8, 0xcf2b, "STULE8");
    create_handler_2!(bdepth, 0xcf30, "BDEPTH");
    create_handler_2!(bbits, 0xcf31, "BBITS");
    create_handler_2!(brefs, 0xcf32, "BREFS");
    create_handler_2!(bbitrefs, 0xcf33, "BBITREFS");
    create_handler_2!(brembits, 0xcf35, "BREMBITS");
    create_handler_2!(bremrefs, 0xcf36, "BREMREFS");
    create_handler_2!(brembitrefs, 0xcf37, "BREMBITREFS");
    pub(super) fn bchkbits_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf38);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("BCHKBITS").with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_2!(bchkbits_long, 0xcf39, "BCHKBITS");
    create_handler_2!(bchkrefs, 0xcf3a, "BCHKREFS");
    create_handler_2!(bchkbitrefs, 0xcf3b, "BCHKBITREFS");
    pub(super) fn bchkbitsq_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xcf3c);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("BCHKBITS")
            .set_quiet()
            .with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_2!(bchkbitsq_long, 0xcf3d, "BCHKBITSQ");
    create_handler_2!(bchkrefsq, 0xcf3e, "BCHKREFSQ");
    create_handler_2!(bchkbitrefsq, 0xcf3f, "BCHKBITREFSQ");
    create_handler_2!(stzeroes, 0xcf40, "STZEROES");
    create_handler_2!(stones, 0xcf41, "STONES");
    create_handler_2!(stsame, 0xcf42, "STSAME");
    pub(super) fn stsliceconst(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(9)?;
        check_eq!(opc << 3, 0xcf8);
        let x = slice.get_next_int(2)?;
        check_eq!(x, 0);
        let y = slice.get_next_int(3)?;
        let mut sss = slice.get_next_slice(y as usize * 8 + 2)?;
        sss.trim_right();
        Ok(Instruction::new("STSLICECONST").with_param(InstructionParameter::Slice(sss)))
    }
    create_handler_2!(btos, 0xcf50, "BTOS");
    create_handler_1!(ctos, 0xd0, "CTOS");
    create_handler_1!(ends, 0xd1, "ENDS");
    pub(super) fn ldi(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xd2);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("LDI").with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn ldu(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xd3);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("LDU").with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_1!(ldref, 0xd4, "LDREF");
    create_handler_1!(ldrefrtos, 0xd5, "LDREFRTOS");
    pub(super) fn ldslice(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xd6);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("LDSLICE").with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_2!(ldix, 0xd700, "LDIX");
    create_handler_2!(ldux, 0xd701, "LDUX");
    create_handler_2!(pldix, 0xd702, "PLDIX");
    create_handler_2!(pldux, 0xd703, "PLDUX");
    create_handler_2!(ldixq, 0xd704, "LDIXQ");
    create_handler_2!(lduxq, 0xd705, "LDUXQ");
    create_handler_2!(pldixq, 0xd706, "PLDIXQ");
    create_handler_2!(plduxq, 0xd707, "PLDUXQ");
    pub(super) fn pldi(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd70a);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("PLDI").with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn pldu(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd70b);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("PLDU").with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn ldiq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd70c);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("LDI").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn lduq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd70d);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("LDU").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn pldiq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd70e);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("PLDI").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn plduq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd70f);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("PLDU").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn plduz(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xd710);
        let c = slice.get_next_int(3)? as usize;
        Ok(Instruction::new("PLDUZ").with_param(InstructionParameter::Length(32 * (c + 1))))
    }
    create_handler_2!(ldslicex, 0xd718, "LDSLICEX");
    create_handler_2!(pldslicex, 0xd719, "PLDSLICEX");
    create_handler_2!(ldslicexq, 0xd71a, "LDSLICEXQ");
    create_handler_2!(pldslicexq, 0xd71b, "PLDSLICEXQ");
    pub(super) fn pldslice(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd71d);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("PLDSLICE").with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn ldsliceq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd71e);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("LDSLICE").set_quiet().with_param(InstructionParameter::Length(cc + 1)))
    }
    pub(super) fn pldsliceq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xd71f);
        let cc = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("PLDSLICE")
            .set_quiet()
            .with_param(InstructionParameter::Length(cc + 1)))
    }
    create_handler_2!(sdskipfirst, 0xd721, "SDSKIPFIRST");
    create_handler_2!(sdcutlast, 0xd722, "SDCUTLAST");
    create_handler_2!(sdskiplast, 0xd723, "SDSKIPLAST");
    create_handler_2!(sdsubstr, 0xd724, "SDSUBSTR");
    create_handler_2!(sdbeginsx, 0xd726, "SDBEGINSX");
    create_handler_2!(sdbeginsxq, 0xd727, "SDBEGINSXQ");
    pub(super) fn sdbegins(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc << 2, 0xd728);
        let x = slice.get_next_int(7)? as usize;
        let mut bitstring = slice.get_next_slice(8 * x + 3)?;
        bitstring.trim_right();
        Ok(Instruction::new("SDBEGINS").with_param(InstructionParameter::Slice(bitstring)))
    }
    pub(super) fn sdbeginsq(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc << 2, 0xd72c);
        let x = slice.get_next_int(7)? as usize;
        let mut bitstring = slice.get_next_slice(8 * x + 3)?;
        bitstring.trim_right();
        Ok(Instruction::new("SDBEGINS")
            .set_quiet()
            .with_param(InstructionParameter::Slice(bitstring)))
    }
    create_handler_2!(scutfirst, 0xd730, "SCUTFIRST");
    create_handler_2!(sskipfirst, 0xd731, "SSKIPFIRST");
    create_handler_2!(scutlast, 0xd732, "SCUTLAST");
    create_handler_2!(sskiplast, 0xd733, "SSKIPLAST");
    create_handler_2!(subslice, 0xd734, "SUBSLICE");
    create_handler_2!(split, 0xd736, "SPLIT");
    create_handler_2!(splitq, 0xd737, "SPLITQ");
    create_handler_2!(xctos, 0xd739, "XCTOS");
    create_handler_2!(xload, 0xd73a, "XLOAD");
    create_handler_2!(xloadq, 0xd73b, "XLOADQ");
    create_handler_2!(schkbits, 0xd741, "SCHKBITS");
    create_handler_2!(schkrefs, 0xd742, "SCHKREFS");
    create_handler_2!(schkbitrefs, 0xd743, "XCHKBITREFS");
    create_handler_2!(schkbitsq, 0xd745, "SCHKBITSQ");
    create_handler_2!(schkrefsq, 0xd746, "SCHKREFSQ");
    create_handler_2!(schkbitrefsq, 0xd747, "SCHKBITREFSQ");
    create_handler_2!(pldrefvar, 0xd748, "PLDREFVAR");
    create_handler_2!(sbits, 0xd749, "SBITS");
    create_handler_2!(srefs, 0xd74a, "SREFS");
    create_handler_2!(sbitrefs, 0xd74b, "SBITREFS");
    create_handler_2!(pldref, 0xd74c, "PLDREF");
    pub(super) fn pldrefidx(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc << 2, 0xd74c);
        let n = slice.get_next_int(2)? as usize;
        Ok(Instruction::new("PLDREFIDX").with_param(InstructionParameter::Length(n)))
    }
    create_handler_2!(ldile4, 0xd750, "LDILE4");
    create_handler_2!(ldule4, 0xd751, "LDULE4");
    create_handler_2!(ldile8, 0xd752, "LDILE8");
    create_handler_2!(ldule8, 0xd753, "LDULE8");
    create_handler_2!(pldile4, 0xd754, "PLDILE4");
    create_handler_2!(pldule4, 0xd755, "PLDULE4");
    create_handler_2!(pldile8, 0xd756, "PLDILE8");
    create_handler_2!(pldule8, 0xd757, "PLDULE8");
    create_handler_2!(ldile4q, 0xd758, "LDILE4Q");
    create_handler_2!(ldule4q, 0xd759, "LDULE4Q");
    create_handler_2!(ldile8q, 0xd75a, "LDILE8Q");
    create_handler_2!(ldule8q, 0xd75b, "LDULE8Q");
    create_handler_2!(pldile4q, 0xd75c, "PLDILE4Q");
    create_handler_2!(pldule4q, 0xd75d, "PLDULE4Q");
    create_handler_2!(pldile8q, 0xd75e, "PLDILE8Q");
    create_handler_2!(pldule8q, 0xd75f, "PLDULE8Q");
    create_handler_2!(ldzeroes, 0xd760, "LDZEROES");
    create_handler_2!(ldones, 0xd761, "LDONES");
    create_handler_2!(ldsame, 0xd762, "LDSAME");
    create_handler_2!(sdepth, 0xd764, "SDEPTH");
    create_handler_2!(cdepth, 0xd765, "CDEPTH");
    create_handler_2!(clevel, 0xd766, "CLEVEL");
    create_handler_2!(clevel_mask, 0xd767, "CLEVELMASK");
    pub(super) fn cdepthi(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc, 0xd76c);
        let i = slice.get_next_int(2)? as usize;
        Ok(Instruction::new("CDEPTHI").with_param(InstructionParameter::Length(i)))
    }
    create_handler_2!(cdepthix, 0xd771, "CDEPTHIX");
    pub(super) fn chashi(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc, 0xd768);
        let i = slice.get_next_int(2)? as usize;
        Ok(Instruction::new("CHASHI").with_param(InstructionParameter::Length(i)))
    }
    create_handler_2!(chashix, 0xd770, "CHASHIX");
    create_handler_1!(callx, 0xd8, "CALLX");
    create_handler_1!(jmpx, 0xd9, "JMPX");
    pub(super) fn callxargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        match opc {
            0xda => {
                let p = slice.get_next_int(4)? as usize;
                let r = slice.get_next_int(4)? as usize;
                Ok(Instruction::new("CALLXARGS")
                    .with_param(InstructionParameter::Pargs(p))
                    .with_param(InstructionParameter::Rargs(r)))
            }
            0xdb => {
                let z = slice.get_next_int(4)?;
                check_eq!(z, 0);
                let p = slice.get_next_int(4)? as usize;
                Ok(Instruction::new("CALLXARGS")
                    .with_param(InstructionParameter::Pargs(p))
                    .with_param(InstructionParameter::Rargs(usize::MAX)))
            }
            _ => fail!("unknown callxargs kind"),
        }
    }
    pub(super) fn jmpxargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xdb1);
        let p = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("JMPXARGS").with_param(InstructionParameter::Pargs(p)))
    }
    pub(super) fn retargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xdb2);
        let r = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("RETARGS").with_param(InstructionParameter::Rargs(r)))
    }
    create_handler_2!(ret, 0xdb30, "RET");
    create_handler_2!(retalt, 0xdb31, "RETALT");
    create_handler_2!(retbool, 0xdb32, "RETBOOL");
    create_handler_2!(callcc, 0xdb34, "CALLCC");
    create_handler_2!(jmpxdata, 0xdb35, "JMPXDATA");
    pub(super) fn callccargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc, 0xdb36);
        let p = slice.get_next_int(4)? as usize;
        let r = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("CALLCCARGS")
            .with_param(InstructionParameter::Pargs(p))
            .with_param(InstructionParameter::Rargs(r)))
    }
    create_handler_2!(callxva, 0xdb38, "CALLXVARARGS");
    create_handler_2!(retva, 0xdb39, "RETVARARGS");
    create_handler_2!(jmpxva, 0xdb3a, "JMPXVARARGS");
    create_handler_2!(callccva, 0xdb3b, "CALLCCVARARGS");
    create_handler_2r!(callref, 0xdb3c, "CALLREF");
    create_handler_2r!(jmpref, 0xdb3d, "JMPREF");
    create_handler_2r!(jmprefdata, 0xdb3e, "JMPREFDATA");
    create_handler_2!(retdata, 0xdb3f, "RETDATA");
    create_handler_1!(ifret, 0xdc, "IFRET");
    create_handler_1!(ifnotret, 0xdd, "IFNOTRET");
    create_handler_1!(if_, 0xde, "IF");
    create_handler_1!(ifnot, 0xdf, "IFNOT");
    create_handler_1!(ifjmp, 0xe0, "IFJMP");
    create_handler_1!(ifnotjmp, 0xe1, "IFNOTJMP");
    create_handler_1!(ifelse, 0xe2, "IFELSE");
    create_handler_2r!(ifref, 0xe300, "IFREF");
    create_handler_2r!(ifnotref, 0xe301, "IFNOTREF");
    create_handler_2r!(ifjmpref, 0xe302, "IFJMPREF");
    create_handler_2r!(ifnotjmpref, 0xe303, "IFNOTJMPREF");
    create_handler_2!(condsel, 0xe304, "CONDSEL");
    create_handler_2!(condselchk, 0xe305, "CONDSELCHK");
    create_handler_2!(ifretalt, 0xe308, "IFRETALT");
    create_handler_2!(ifnotretalt, 0xe309, "IFNOTRETALT");
    create_handler_2r!(ifrefelse, 0xe30d, "IFREFELSE");
    create_handler_2r!(ifelseref, 0xe30e, "IFELSEREF");
    create_handler_3r!(ifrefelseref, 0xe30f, "IFREFELSEREF");
    create_handler_2!(repeat_break, 0xe314, "REPEATBRK");
    create_handler_2!(repeatend_break, 0xe315, "REPEATENDBRK");
    create_handler_2!(until_break, 0xe316, "UNTILBRK");
    create_handler_2!(untilend_break, 0xe317, "UNTILENDBRK");
    create_handler_2!(while_break, 0xe318, "WHILEBRK");
    create_handler_2!(whileend_break, 0xe319, "WHILEENDBRK");
    create_handler_2!(again_break, 0xe31a, "AGAINBRK");
    create_handler_2!(againend_break, 0xe31b, "AGAINENDBRK");
    pub(super) fn ifbitjmp(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(15)?;
        check_eq!(opc << 1, 0xe38);
        let n = slice.get_next_int(5)? as isize;
        Ok(Instruction::new("IFBITJMP").with_param(InstructionParameter::Integer(n)))
    }
    pub(super) fn ifnbitjmp(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(15)?;
        check_eq!(opc << 1, 0xe3a);
        let n = slice.get_next_int(5)? as isize;
        Ok(Instruction::new("IFNBITJMP").with_param(InstructionParameter::Integer(n)))
    }
    pub(super) fn ifbitjmpref(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(15)?;
        check_eq!(opc << 1, 0xe3c);
        let n = slice.get_next_int(5)? as isize;
        let cell = slice.checked_drain_reference().ok();
        let code = self.load_cell(&cell)?;
        Ok(Instruction::new("IFBITJMPREF")
            .with_refs(1)
            .with_param(InstructionParameter::Integer(n))
            .with_param(InstructionParameter::Code { code, cell }))
    }
    pub(super) fn ifnbitjmpref(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(15)?;
        check_eq!(opc << 1, 0xe3e);
        let n = slice.get_next_int(5)? as isize;
        let cell = slice.checked_drain_reference().ok();
        let code = self.load_cell(&cell)?;
        Ok(Instruction::new("IFNBITJMPREF")
            .with_refs(1)
            .with_param(InstructionParameter::Integer(n))
            .with_param(InstructionParameter::Code { code, cell }))
    }
    create_handler_1!(repeat, 0xe4, "REPEAT");
    create_handler_1!(repeatend, 0xe5, "REPEATEND");
    create_handler_1!(until, 0xe6, "UNTIL");
    create_handler_1!(untilend, 0xe7, "UNTILEND");
    create_handler_1!(while_, 0xe8, "WHILE");
    create_handler_1!(whileend, 0xe9, "WHILEEND");
    create_handler_1!(again, 0xea, "AGAIN");
    create_handler_1!(againend, 0xeb, "AGAINEND");
    pub(super) fn setcontargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xec);
        let r = slice.get_next_int(4)? as usize;
        let n = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("SETCONTARGS")
            .with_param(InstructionParameter::Rargs(r))
            .with_param(InstructionParameter::Nargs(n)))
    }
    pub(super) fn returnargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed0);
        let p = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("RETURNARGS").with_param(InstructionParameter::Pargs(p)))
    }
    create_handler_2!(returnva, 0xed10, "RETURNVARARGS");
    create_handler_2!(setcontva, 0xed11, "SETCONTVARARGS");
    create_handler_2!(setnumva, 0xed12, "SETNUMVARARGS");
    create_handler_2!(bless, 0xed1e, "BLESS");
    create_handler_2!(blessva, 0xed1f, "BLESSVARARGS");
    pub(super) fn pushctr(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed4);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("PUSHCTR").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn popctr(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed5);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("POPCTR").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn setcontctr(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed6);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SETCONTCTR").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn setretctr(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed7);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SETRETCTR").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn setaltctr(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed8);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SETALTCTR").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn popsave(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xed9);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("POPSAVE").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn save(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xeda);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SAVE").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn savealt(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xedb);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SAVEALT").with_param(InstructionParameter::ControlRegister(i)))
    }
    pub(super) fn saveboth(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xedc);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("SAVEBOTH").with_param(InstructionParameter::ControlRegister(i)))
    }
    create_handler_2!(pushctrx, 0xede0, "PUSHCTRX");
    create_handler_2!(popctrx, 0xede1, "POPCTRX");
    create_handler_2!(setcontctrx, 0xede2, "SETCONTCTRX");
    pub(super) fn setcontctrmany(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xede3);
        let x = slice.get_next_int(8)? as usize;
        Ok(Instruction::new("SETCONTCTRMANY").with_param(InstructionParameter::Length(x)))
    }
    create_handler_2!(setcontctrmanyx, 0xede4, "SETCONTCTRMANYX");
    create_handler_2!(compos, 0xedf0, "COMPOS");
    create_handler_2!(composalt, 0xedf1, "COMPOSALT");
    create_handler_2!(composboth, 0xedf2, "COMPOSBOTH");
    create_handler_2!(atexit, 0xedf3, "ATEXIT");
    create_handler_2!(atexitalt, 0xedf4, "ATEXITALT");
    create_handler_2!(setexitalt, 0xedf5, "SETEXITALT");
    create_handler_2!(thenret, 0xedf6, "THENRET");
    create_handler_2!(thenretalt, 0xedf7, "THENRETALT");
    create_handler_2!(invert, 0xedf8, "INVERT");
    create_handler_2!(booleval, 0xedf9, "BOOLEVAL");
    create_handler_2!(samealt, 0xedfa, "SAMEALT");
    create_handler_2!(samealt_save, 0xedfb, "SAMEALTSAVE");
    pub(super) fn blessargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xee);
        let r = slice.get_next_int(4)? as usize;
        let n = slice.get_next_int(4)? as isize;
        Ok(Instruction::new("BLESSARGS")
            .with_param(InstructionParameter::Rargs(r))
            .with_param(InstructionParameter::Nargs(n)))
    }
    pub(super) fn call_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xf0);
        let n = slice.get_next_int(8)? as isize;
        Ok(Instruction::new("CALL").with_param(InstructionParameter::Nargs(n)))
    }
    pub(super) fn call_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0xf10);
        let n = slice.get_next_int(14)? as isize;
        Ok(Instruction::new("CALL").with_param(InstructionParameter::Nargs(n)))
    }
    pub(super) fn jmp(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0xf14);
        let n = slice.get_next_int(14)? as isize;
        Ok(Instruction::new("JMPDICT").with_param(InstructionParameter::Nargs(n)))
    }
    pub(super) fn prepare(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0xf18);
        let n = slice.get_next_int(14)? as isize;
        Ok(Instruction::new("PREPARE").with_param(InstructionParameter::Nargs(n)))
    }
    pub(super) fn throw_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0xf20);
        let nn = slice.get_next_int(6)? as isize;
        Ok(Instruction::new("THROW").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwif_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0xf24);
        let nn = slice.get_next_int(6)? as isize;
        Ok(Instruction::new("THROWIF").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwifnot_short(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(10)?;
        check_eq!(opc << 2, 0xf28);
        let nn = slice.get_next_int(6)? as isize;
        Ok(Instruction::new("THROWIFNOT").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throw_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xf2c0);
        let nn = slice.get_next_int(11)? as isize;
        Ok(Instruction::new("THROW").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwarg(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xf2c8);
        let nn = slice.get_next_int(11)? as isize;
        Ok(Instruction::new("THROWARG").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwif_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xf2d0);
        let nn = slice.get_next_int(11)? as isize;
        Ok(Instruction::new("THROWIF").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwargif(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xf2d8);
        let nn = slice.get_next_int(11)? as isize;
        Ok(Instruction::new("THROWARGIF").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwifnot_long(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xf2e0);
        let nn = slice.get_next_int(11)? as isize;
        Ok(Instruction::new("THROWIFNOT").with_param(InstructionParameter::Integer(nn)))
    }
    pub(super) fn throwargifnot(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(13)?;
        check_eq!(opc << 3, 0xf2e8);
        let nn = slice.get_next_int(11)? as isize;
        Ok(Instruction::new("THROWARGIFNOT").with_param(InstructionParameter::Integer(nn)))
    }
    create_handler_2!(throwany, 0xf2f0, "THROWANY");
    create_handler_2!(throwargany, 0xf2f1, "THROWARGANY");
    create_handler_2!(throwanyif, 0xf2f2, "THROWANYIF");
    create_handler_2!(throwarganyif, 0xf2f3, "THROWARGANYIF");
    create_handler_2!(throwanyifnot, 0xf2f4, "THROWANYIFNOT");
    create_handler_2!(throwarganyifnot, 0xf2f5, "THROWARGANYIFNOT");
    create_handler_2!(try_, 0xf2ff, "TRY");
    pub(super) fn tryargs(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(8)?;
        check_eq!(opc, 0xf3);
        let p = slice.get_next_int(4)? as usize;
        let r = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("TRYARGS")
            .with_param(InstructionParameter::Pargs(p))
            .with_param(InstructionParameter::Rargs(r)))
    }
    create_handler_2!(ldgrams, 0xfa00, "LDGRAMS");
    create_handler_2!(ldvarint16, 0xfa01, "LDVARINT16");
    create_handler_2!(stgrams, 0xfa02, "STGRAMS");
    create_handler_2!(stvarint16, 0xfa03, "STVARINT16");
    create_handler_2!(ldvaruint32, 0xfa04, "LDVARUINT32");
    create_handler_2!(ldvarint32, 0xfa05, "LDVARINT32");
    create_handler_2!(stvaruint32, 0xfa06, "STVARUINT32");
    create_handler_2!(stvarint32, 0xfa07, "STVARINT32");
    pub(super) fn ldmsgaddr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa40);
        Ok(T::insn(Instruction::new("LDMSGADDR")))
    }
    pub(super) fn ldstdaddr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa48);
        Ok(T::insn(Instruction::new("LDSTDADDR")))
    }
    pub(super) fn ldoptstdaddr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa50);
        Ok(T::insn(Instruction::new("LDOPTSTDADDR")))
    }
    pub(super) fn ststdaddr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa52);
        Ok(T::insn(Instruction::new("STSTDADDR")))
    }
    pub(super) fn stoptstdaddr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa54);
        Ok(T::insn(Instruction::new("STOPTSTDADDR")))
    }
    pub(super) fn parsemsgaddr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa42);
        Ok(T::insn(Instruction::new("PARSEMSGADDR")))
    }
    pub(super) fn rewrite_std_addr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa44);
        Ok(T::insn(Instruction::new("REWRITESTDADDR")))
    }
    pub(super) fn rewrite_var_addr<T>(&mut self, slice: &mut SliceData) -> Result<Instruction>
    where
        T: OperationBehavior,
    {
        let opc = slice.get_next_int(16)?;
        check_eq!(opc & 0xfffe, 0xfa46);
        Ok(T::insn(Instruction::new("REWRITEVARADDR")))
    }
    create_handler_2!(sendrawmsg, 0xfb00, "SENDRAWMSG");
    create_handler_2!(rawreserve, 0xfb02, "RAWRESERVE");
    create_handler_2!(rawreservex, 0xfb03, "RAWRESERVEX");
    create_handler_2!(setcode, 0xfb04, "SETCODE");
    create_handler_2!(setlibcode, 0xfb06, "SETLIBCODE");
    create_handler_2!(changelib, 0xfb07, "CHANGELIB");
    create_handler_2!(stdict, 0xf400, "STDICT");
    create_handler_2!(skipdict, 0xf401, "SKIPDICT");
    create_handler_2!(lddicts, 0xf402, "LDDICTS");
    create_handler_2!(plddicts, 0xf403, "PLDDICTS");
    create_handler_2!(lddict, 0xf404, "LDDICT");
    create_handler_2!(plddict, 0xf405, "PLDDICT");
    create_handler_2!(lddictq, 0xf406, "LDDICT");
    create_handler_2!(plddictq, 0xf407, "PLDDICT");
    create_handler_2!(dictget, 0xf40a, "DICTGET");
    create_handler_2!(dictgetref, 0xf40b, "DICTGETREF");
    create_handler_2!(dictiget, 0xf40c, "DICTIGET");
    create_handler_2!(dictigetref, 0xf40d, "DICTIGETREF");
    create_handler_2!(dictuget, 0xf40e, "DICTUGET");
    create_handler_2!(dictugetref, 0xf40f, "DICTUGETREF");
    create_handler_2!(dictset, 0xf412, "DICTSET");
    create_handler_2!(dictsetref, 0xf413, "DICTSETREF");
    create_handler_2!(dictiset, 0xf414, "DICTISET");
    create_handler_2!(dictisetref, 0xf415, "DICTISETREF");
    create_handler_2!(dictuset, 0xf416, "DICTUSET");
    create_handler_2!(dictusetref, 0xf417, "DICTUSETREF");
    create_handler_2!(dictsetget, 0xf41a, "DICTSETGET");
    create_handler_2!(dictsetgetref, 0xf41b, "DICTSETGETREF");
    create_handler_2!(dictisetget, 0xf41c, "DICTISETGET");
    create_handler_2!(dictisetgetref, 0xf41d, "DICTISETGETREF");
    create_handler_2!(dictusetget, 0xf41e, "DICTUSETGET");
    create_handler_2!(dictusetgetref, 0xf41f, "DICTUSETGETREF");
    create_handler_2!(dictreplace, 0xf422, "DICTREPLACE");
    create_handler_2!(dictreplaceref, 0xf423, "DICTREPLACEREF");
    create_handler_2!(dictireplace, 0xf424, "DICTIREPLACE");
    create_handler_2!(dictireplaceref, 0xf425, "DICTIREPLACEREF");
    create_handler_2!(dictureplace, 0xf426, "DICTUREPLACE");
    create_handler_2!(dictureplaceref, 0xf427, "DICTUREPLACEREF");
    create_handler_2!(dictreplaceget, 0xf42a, "DICTREPLACEGET");
    create_handler_2!(dictreplacegetref, 0xf42b, "DICTREPLACEGETREF");
    create_handler_2!(dictireplaceget, 0xf42c, "DICTIREPLACEGET");
    create_handler_2!(dictireplacegetref, 0xf42d, "DICTIREPLACEGETREF");
    create_handler_2!(dictureplaceget, 0xf42e, "DICTUREPLACEGET");
    create_handler_2!(dictureplacegetref, 0xf42f, "DICTUREPLACEGETREF");
    create_handler_2!(dictadd, 0xf432, "DICTADD");
    create_handler_2!(dictaddref, 0xf433, "DICTADDREF");
    create_handler_2!(dictiadd, 0xf434, "DICTIADD");
    create_handler_2!(dictiaddref, 0xf435, "DICTIADDREF");
    create_handler_2!(dictuadd, 0xf436, "DICTUADD");
    create_handler_2!(dictuaddref, 0xf437, "DICTUADDREF");
    create_handler_2!(dictaddget, 0xf43a, "DICTADDGET");
    create_handler_2!(dictaddgetref, 0xf43b, "DICTADDGETREF");
    create_handler_2!(dictiaddget, 0xf43c, "DICTIADDGET");
    create_handler_2!(dictiaddgetref, 0xf43d, "DICTIADDGETREF");
    create_handler_2!(dictuaddget, 0xf43e, "DICTUADDGET");
    create_handler_2!(dictuaddgetref, 0xf43f, "DICTUADDGETREF");
    create_handler_2!(dictsetb, 0xf441, "DICTSETB");
    create_handler_2!(dictisetb, 0xf442, "DICTISETB");
    create_handler_2!(dictusetb, 0xf443, "DICTUSETB");
    create_handler_2!(dictsetgetb, 0xf445, "DICTSETGETB");
    create_handler_2!(dictisetgetb, 0xf446, "DICTISETGETB");
    create_handler_2!(dictusetgetb, 0xf447, "DICTUSETGETB");
    create_handler_2!(dictreplaceb, 0xf449, "DICTREPLACEB");
    create_handler_2!(dictireplaceb, 0xf44a, "DICTIREPLACEB");
    create_handler_2!(dictureplaceb, 0xf44b, "DICTUREPLACEB");
    create_handler_2!(dictreplacegetb, 0xf44d, "DICTREPLACEGETB");
    create_handler_2!(dictireplacegetb, 0xf44e, "DICTIREPLACEGETB");
    create_handler_2!(dictureplacegetb, 0xf44f, "DICTUREPLACEGETB");
    create_handler_2!(dictaddb, 0xf451, "DICTADDB");
    create_handler_2!(dictiaddb, 0xf452, "DICTIADDB");
    create_handler_2!(dictuaddb, 0xf453, "DICTUADDB");
    create_handler_2!(dictaddgetb, 0xf455, "DICTADDGETB");
    create_handler_2!(dictiaddgetb, 0xf456, "DICTIADDGETB");
    create_handler_2!(dictuaddgetb, 0xf457, "DICTUADDGETB");
    create_handler_2!(dictdel, 0xf459, "DICTDEL");
    create_handler_2!(dictidel, 0xf45a, "DICTIDEL");
    create_handler_2!(dictudel, 0xf45b, "DICTUDEL");
    create_handler_2!(dictdelget, 0xf462, "DICTDELGET");
    create_handler_2!(dictdelgetref, 0xf443, "DICTDELGETREF");
    create_handler_2!(dictidelget, 0xf444, "DICTIDELGET");
    create_handler_2!(dictidelgetref, 0xf445, "DICTIDELGETREF");
    create_handler_2!(dictudelget, 0xf466, "DICTUDELGET");
    create_handler_2!(dictudelgetref, 0xf467, "DICTUDELGETREF");
    create_handler_2!(dictgetoptref, 0xf469, "DICTGETOPTREF");
    create_handler_2!(dictigetoptref, 0xf46a, "DICTIGETOPTREF");
    create_handler_2!(dictugetoptref, 0xf46b, "DICTUGETOPTREF");
    create_handler_2!(dictsetgetoptref, 0xf46d, "DICTSETGETOPTREF");
    create_handler_2!(dictisetgetoptref, 0xf46e, "DICTISETGETOPTREF");
    create_handler_2!(dictusetgetoptref, 0xf46f, "DICTUSETGETOPTREF");
    create_handler_2!(pfxdictset, 0xf470, "PFXDICTSET");
    create_handler_2!(pfxdictreplace, 0xf471, "PFXDICTREPLACE");
    create_handler_2!(pfxdictadd, 0xf472, "PFXDICTADD");
    create_handler_2!(pfxdictdel, 0xf473, "PFXDICTDEL");
    create_handler_2!(dictgetnext, 0xf474, "DICTGETNEXT");
    create_handler_2!(dictgetnexteq, 0xf475, "DICTGETNEXTEQ");
    create_handler_2!(dictgetprev, 0xf476, "DICTGETPREV");
    create_handler_2!(dictgetpreveq, 0xf477, "DICTGETPREVEQ");
    create_handler_2!(dictigetnext, 0xf478, "DICTIGETNEXT");
    create_handler_2!(dictigetnexteq, 0xf479, "DICTIGETNEXTEQ");
    create_handler_2!(dictigetprev, 0xf47a, "DICTIGETPREV");
    create_handler_2!(dictigetpreveq, 0xf47b, "DICTIGETPREVEQ");
    create_handler_2!(dictugetnext, 0xf47c, "DICTUGETNEXT");
    create_handler_2!(dictugetnexteq, 0xf47d, "DICTUGETNEXTEQ");
    create_handler_2!(dictugetprev, 0xf47e, "DICTUGETPREV");
    create_handler_2!(dictugetpreveq, 0xf47f, "DICTUGETPREVEQ");
    create_handler_2!(dictmin, 0xf482, "DICTMIN");
    create_handler_2!(dictminref, 0xf483, "DICTMINREF");
    create_handler_2!(dictimin, 0xf484, "DICTIMIN");
    create_handler_2!(dictiminref, 0xf485, "DICTIMINREF");
    create_handler_2!(dictumin, 0xf486, "DICTUMIN");
    create_handler_2!(dictuminref, 0xf487, "DICTUMINREF");
    create_handler_2!(dictmax, 0xf48a, "DICTMAX");
    create_handler_2!(dictmaxref, 0xf48b, "DICTMAXREF");
    create_handler_2!(dictimax, 0xf48c, "DICTIMAX");
    create_handler_2!(dictimaxref, 0xf48d, "DICTIMAXREF");
    create_handler_2!(dictumax, 0xf48e, "DICTUMAX");
    create_handler_2!(dictumaxref, 0xf48f, "DICTUMAXREF");
    create_handler_2!(dictremmin, 0xf492, "DICTREMMIN");
    create_handler_2!(dictremminref, 0xf493, "DICTREMMINREF");
    create_handler_2!(dictiremmin, 0xf494, "DICTIREMMIN");
    create_handler_2!(dictiremminref, 0xf495, "DICTIREMMINREF");
    create_handler_2!(dicturemmin, 0xf496, "DICTUREMMIN");
    create_handler_2!(dicturemminref, 0xf497, "DICTUREMMINREF");
    create_handler_2!(dictremmax, 0xf49a, "DICTREMMAX");
    create_handler_2!(dictremmaxref, 0xf49b, "DICTREMMAXREF");
    create_handler_2!(dictiremmax, 0xf49c, "DICTIREMMAX");
    create_handler_2!(dictiremmaxref, 0xf49d, "DICTIREMMAXREF");
    create_handler_2!(dicturemmax, 0xf49e, "DICTUREMMAX");
    create_handler_2!(dicturemmaxref, 0xf49f, "DICTUREMMAXREF");
    create_handler_2!(dictigetjmp, 0xf4a0, "DICTIGETJMP");
    create_handler_2!(dictugetjmp, 0xf4a1, "DICTUGETJMP");
    create_handler_2!(dictigetexec, 0xf4a2, "DICTIGETEXEC");
    create_handler_2!(dictugetexec, 0xf4a3, "DICTUGETEXEC");
    pub(super) fn dictpushconst(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc << 2, 0xf4a4);
        let n = slice.get_next_int(10)? as usize;
        let cell = slice.checked_drain_reference().ok();
        Ok(Instruction::new("DICTPUSHCONST")
            .with_refs(1)
            .with_param(InstructionParameter::Length(n))
            .with_param(InstructionParameter::Cell { cell, collapsed: false }))
    }
    create_handler_2!(pfxdictgetq, 0xf4a8, "PFXDICTGETQ");
    create_handler_2!(pfxdictget, 0xf4a9, "PFXDICTGET");
    create_handler_2!(pfxdictgetjmp, 0xf4aa, "PFXDICTGETJMP");
    create_handler_2!(pfxdictgetexec, 0xf4ab, "PFXDICTGETEXEC");
    pub(super) fn pfxdictswitch(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(14)?;
        check_eq!(opc << 2, 0xf4ac);
        let n = slice.get_next_int(10)? as usize;
        let cell = slice.checked_drain_reference().ok();
        Ok(Instruction::new("PFXDICTSWITCH")
            .with_refs(1)
            .with_param(InstructionParameter::Length(n))
            .with_param(InstructionParameter::Cell { cell, collapsed: false }))
    }
    create_handler_2!(subdictget, 0xf4b1, "SUBDICTGET");
    create_handler_2!(subdictiget, 0xf4b2, "SUBDICTIGET");
    create_handler_2!(subdictuget, 0xf4b3, "SUBDICTUGET");
    create_handler_2!(subdictrpget, 0xf4b5, "SUBDICTRPGET");
    create_handler_2!(subdictirpget, 0xf4b6, "SUBDICTIRPGET");
    create_handler_2!(subdicturpget, 0xf4b7, "SUBDICTURPGET");
    create_handler_2!(dictigetjmpz, 0xf4bc, "DICTIGETJMPZ");
    create_handler_2!(dictugetjmpz, 0xf4bd, "DICTUGETJMPZ");
    create_handler_2!(dictigetexecz, 0xf4be, "DICTIGETEXECZ");
    create_handler_2!(dictugetexecz, 0xf4bf, "DICTUGETEXECZ");
    create_handler_2!(accept, 0xf800, "ACCEPT");
    create_handler_2!(setgaslimit, 0xf801, "SETGASLIMIT");
    create_handler_2!(buygas, 0xf802, "BUYGAS");
    create_handler_2!(gramtogas, 0xf804, "GRAMTOGAS");
    create_handler_2!(gasconsumed, 0xf807, "GASCONSUMED");
    create_handler_2!(gastogram, 0xf805, "GASTOGRAM");
    create_handler_2!(commit, 0xf80f, "COMMIT");
    create_handler_2!(randu256, 0xf810, "RANDU256");
    create_handler_2!(rand, 0xf811, "RAND");
    create_handler_2!(setrand, 0xf814, "SETRAND");
    create_handler_2!(addrand, 0xf815, "ADDRAND");
    pub(super) fn getparam(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xf82);
        let i = slice.get_next_int(4)? as usize;
        Ok(Instruction::new("GETPARAM").with_param(InstructionParameter::Length(i)))
    }
    create_handler_2!(now, 0xf823, "NOW");
    create_handler_2!(blocklt, 0xf824, "BLOCKLT");
    create_handler_2!(ltime, 0xf825, "LTIME");
    create_handler_2!(randseed, 0xf826, "RANDSEED");
    create_handler_2!(balance, 0xf827, "BALANCE");
    create_handler_2!(my_addr, 0xf828, "MYADDR");
    create_handler_2!(config_root, 0xf829, "CONFIGROOT");
    create_handler_2!(my_code, 0xf82a, "MYCODE");
    create_handler_2!(config_dict, 0xf830, "CONFIGDICT");
    create_handler_2!(config_ref_param, 0xf832, "CONFIGPARAM");
    create_handler_2!(config_opt_param, 0xf833, "CONFIGOPTPARAM");
    create_handler_2!(getglobvar, 0xf840, "GETGLOBVAR");
    pub(super) fn getglob(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(11)?;
        check_eq!(opc << 1, 0xf84);
        let k = slice.get_next_int(5)? as usize;
        assert_ne!(k, 0);
        Ok(Instruction::new("GETGLOB").with_param(InstructionParameter::Length(k)))
    }
    create_handler_2!(setglobvar, 0xf860, "SETGLOBVAR");
    pub(super) fn setglob(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(11)?;
        check_eq!(opc << 1, 0xf86);
        let k = slice.get_next_int(5)? as usize;
        assert_ne!(k, 0);
        Ok(Instruction::new("SETGLOB").with_param(InstructionParameter::Length(k)))
    }
    create_handler_2!(hashbu, 0xf916, "HASHBU");
    create_handler_2!(hashcu, 0xf900, "HASHCU");
    create_handler_2!(hashsu, 0xf901, "HASHSU");
    create_handler_2!(sha256u, 0xf902, "SHA256U");
    create_handler_2!(chksignu, 0xf910, "CHKSIGNU");
    create_handler_2!(chksigns, 0xf911, "CHKSIGNS");
    create_handler_2!(cdatasizeq, 0xf940, "CDATASIZEQ");
    create_handler_2!(cdatasize, 0xf941, "CDATASIZE");
    create_handler_2!(sdatasizeq, 0xf942, "SDATASIZEQ");
    create_handler_2!(sdatasize, 0xf943, "SDATASIZE");
    create_handler_2!(dump_stack, 0xfe00, "DUMPSTK");
    pub(super) fn dump_stack_top(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xfe0);
        let n = slice.get_next_int(4)? as isize;
        assert!(n > 0);
        Ok(Instruction::new("DUMPSTKTOP").with_param(InstructionParameter::Integer(n)))
    }
    create_handler_2!(dump_hex, 0xfe10, "HEXDUMP");
    create_handler_2!(print_hex, 0xfe11, "HEXPRINT");
    create_handler_2!(dump_bin, 0xfe12, "BINDUMP");
    create_handler_2!(print_bin, 0xfe13, "BINPRINT");
    create_handler_2!(dump_str, 0xfe14, "STRDUMP");
    create_handler_2!(print_str, 0xfe15, "STRPRINT");
    create_handler_2!(debug_off, 0xfe1e, "DEBUGOFF");
    create_handler_2!(debug_on, 0xfe1f, "DEBUGON");
    pub(super) fn dump_var(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xfe2);
        let n = slice.get_next_int(4)? as isize;
        assert!(n < 15);
        Ok(Instruction::new("DUMP").with_param(InstructionParameter::Integer(n)))
    }
    pub(super) fn print_var(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xfe3);
        let n = slice.get_next_int(4)? as isize;
        assert!(n < 15);
        Ok(Instruction::new("PRINT").with_param(InstructionParameter::Integer(n)))
    }
    pub(super) fn dump_string(&mut self, slice: &mut SliceData) -> Result<Instruction> {
        let opc = slice.get_next_int(12)?;
        check_eq!(opc, 0xfef);
        let n = slice.get_next_int(4)?;
        let mode = slice.get_next_int(8)?;
        match n {
            0 => {
                check_eq!(mode, 0x00);
                Ok(Instruction::new("LOGFLUSH"))
            }
            _ => {
                if mode == 0x00 {
                    let s = slice.get_next_slice(n as usize * 8)?;
                    Ok(Instruction::new("LOGSTR").with_param(InstructionParameter::Slice(s)))
                } else if mode == 0x01 {
                    let s = slice.get_next_slice(n as usize * 8)?;
                    Ok(Instruction::new("PRINTSTR").with_param(InstructionParameter::Slice(s)))
                } else {
                    fail!("unknown dump_string mode")
                }
            }
        }
    }
}
