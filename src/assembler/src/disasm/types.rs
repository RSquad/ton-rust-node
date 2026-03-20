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
use std::{collections::HashMap, slice::ChunksMut};
use ton_block::{fail, Cell, Result, /*Bitmask,*/ SliceData};

#[derive(Debug, Default, Clone)]
pub struct Code {
    storage: Vec<Instruction>,
}

impl Code {
    pub fn new() -> Self {
        Self { storage: Vec::new() }
    }
    pub fn single(insn: Instruction) -> Self {
        Self { storage: vec![insn] }
    }
    pub fn append(&mut self, other: &mut Self) {
        self.storage.append(&mut other.storage)
    }
    pub fn push(&mut self, insn: Instruction) {
        self.storage.push(insn)
    }
    pub fn chunks_mut(&mut self, chunk_size: usize) -> ChunksMut<'_, Instruction> {
        self.storage.chunks_mut(chunk_size)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Instruction> {
        self.storage.iter()
    }
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Instruction> {
        self.storage.iter_mut()
    }
}

#[derive(Debug, Clone)]
pub struct Instruction {
    name: &'static str,
    params: Vec<InstructionParameter>,
    quiet: bool,
    comment: Option<String>,
    bytecode: Option<SliceData>,
    refs: usize,
}

impl Instruction {
    pub fn new(name: &'static str) -> Self {
        Self { name, params: vec![], quiet: false, comment: None, bytecode: None, refs: 0 }
    }
    pub fn with_refs(self, refs: usize) -> Self {
        let mut clone = self;
        clone.refs = refs;
        clone
    }
    pub fn with_param(self, param: InstructionParameter) -> Self {
        let mut clone = self;
        clone.params.push(param);
        clone
    }
    pub fn set_quiet(self) -> Self {
        let mut clone = self;
        clone.quiet = true;
        clone
    }
    pub fn name(&self) -> &'static str {
        self.name
    }
    pub fn params(&self) -> &Vec<InstructionParameter> {
        &self.params
    }
    pub fn params_mut(&mut self) -> &mut Vec<InstructionParameter> {
        &mut self.params
    }
    pub fn is_quiet(&self) -> bool {
        self.quiet
    }
    pub fn comment(&self) -> Option<&String> {
        self.comment.as_ref()
    }
    pub fn set_comment(&mut self, comment: String) {
        self.comment = Some(comment)
    }
    pub fn bytecode(&self) -> Option<&SliceData> {
        self.bytecode.as_ref()
    }
    pub fn set_bytecode(&mut self, bytecode: SliceData) {
        self.bytecode = Some(bytecode);
    }
    pub fn refs(&self) -> usize {
        self.refs
    }
}

#[derive(Debug, Clone)]
pub enum InstructionParameter {
    BigInteger(num::BigInt),
    ControlRegister(usize),
    //DivisionMode(DivMode),
    Integer(isize),
    Length(usize),
    LengthAndIndex(usize, usize),
    Nargs(isize),
    Pargs(usize),
    Rargs(usize),
    Slice(SliceData),
    StackRegister(isize),
    StackRegisterPair(isize, isize),
    StackRegisterTriple(isize, isize, isize),
    Code { code: Code, cell: Option<Cell> },
    Cell { cell: Option<Cell>, collapsed: bool },
    CodeDictMarker,
}

// #[derive(Clone, Debug)]
// pub struct DivMode {
//     flags: Bitmask,
// }

pub(super) trait OperationBehavior {
    fn insn(insn: Instruction) -> Instruction;
}
pub(super) struct Signaling {}
pub(super) struct Quiet {}
impl OperationBehavior for Signaling {
    fn insn(insn: Instruction) -> Instruction {
        insn
    }
}
impl OperationBehavior for Quiet {
    fn insn(insn: Instruction) -> Instruction {
        insn.set_quiet()
    }
}

enum ShapeKind {
    Any,
    Literal(Vec<u8>),
    Var(&'static str),
}

pub struct Shape {
    kind: ShapeKind,
    refs: Vec<Shape>,
}

impl Shape {
    pub fn any() -> Shape {
        Shape { kind: ShapeKind::Any, refs: vec![] }
    }
    pub fn literal(cst: &'static str) -> Shape {
        Shape { kind: ShapeKind::Literal(hex::decode(cst).expect("bad literal")), refs: vec![] }
    }
    pub fn var(name: &'static str) -> Shape {
        Shape { kind: ShapeKind::Var(name), refs: vec![] }
    }
    pub fn branch(self, node: Shape) -> Shape {
        let mut copy = self;
        copy.refs.push(node);
        copy
    }
    pub fn captures(&self, cell: &Cell) -> Result<HashMap<&'static str, Cell>> {
        let mut map = HashMap::new();
        let children = cell.references_count();
        match &self.kind {
            ShapeKind::Any => return Ok(map),
            ShapeKind::Literal(data) => {
                if cell.bit_length() != data.len() * 8 {
                    fail!("data size doesn't match")
                }
                if &cell.data()[..data.len()] != data {
                    fail!("data doesn't match")
                }
                if self.refs.len() != children {
                    fail!("number of children doesn't match")
                }
            }
            ShapeKind::Var(name) => {
                map.insert(*name, cell.clone());
                return Ok(map);
            }
        }
        for i in 0..children {
            let child = &cell.reference(i).unwrap();
            map.extend(self.refs[i].captures(child)?.into_iter());
        }
        Ok(map)
    }
}
