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
    loader::Loader,
    types::{Code, Instruction, InstructionParameter},
};
use std::collections::HashMap;
use ton_block::{fail, Cell, HashmapE, HashmapType, Result, SliceData};

fn match_dictpushconst_dictugetjmp(
    pair: &mut [Instruction],
) -> Option<&mut Vec<InstructionParameter>> {
    let insn2 = pair.get(1)?.name();
    if insn2 != "DICTUGETJMP" && insn2 != "DICTUGETJMPZ" {
        return None;
    }
    let insn1 = pair.get_mut(0)?;
    if insn1.name() != "DICTPUSHCONST" && insn1.name() != "PFXDICTSWITCH" {
        return None;
    }
    Some(insn1.params_mut())
}

impl Code {
    fn process_dictpushconst_dictugetjmp(code: &mut Code) {
        for pair in code.chunks_mut(2) {
            if let Some(params) = match_dictpushconst_dictugetjmp(pair) {
                // TODO transform cell to code right here (for nested dicts)
                params.push(InstructionParameter::CodeDictMarker)
            }
        }
    }

    fn traverse_code_tree(&mut self, process: fn(&mut Code)) {
        let mut stack = vec![self];
        while let Some(code) = stack.pop() {
            process(code);
            for insn in code.iter_mut() {
                for param in insn.params_mut() {
                    if let InstructionParameter::Code { code: ref mut inner, cell: _ } = param {
                        stack.push(inner)
                    }
                }
            }
        }
    }

    pub fn elaborate_dictpushconst_dictugetjmp(&mut self) {
        self.traverse_code_tree(Self::process_dictpushconst_dictugetjmp)
    }
}

pub(super) struct DelimitedHashmapE {
    dict: HashmapE,
    map: HashMap<Vec<u8>, (u64, usize, Code)>,
}

impl DelimitedHashmapE {
    pub fn new(cell: Cell, key_size: usize) -> Self {
        Self { dict: HashmapE::with_hashmap(key_size, Some(cell)), map: HashMap::new() }
    }
    fn slice_eq_data(lhs: &SliceData, rhs: &SliceData) -> bool {
        let bit_len = lhs.remaining_bits();
        if bit_len != rhs.remaining_bits() {
            return false;
        }
        let mut offset = 0;
        while (offset + 8) <= bit_len {
            if lhs.get_byte(offset).unwrap() != rhs.get_byte(offset).unwrap() {
                return false;
            }
            offset += 8
        }
        if (bit_len > offset)
            && (lhs.get_bits(offset, bit_len - offset).unwrap()
                != rhs.get_bits(offset, bit_len - offset).unwrap())
        {
            return false;
        }
        true
    }
    fn slice_eq_children(lhs: &SliceData, rhs: &SliceData) -> bool {
        let refs_count = lhs.remaining_references();
        if refs_count != rhs.remaining_references() {
            return false;
        }
        for i in 0..refs_count {
            let ref1 = lhs.reference(i).unwrap();
            let ref2 = rhs.reference(i).unwrap();
            if ref1.repr_hash() != ref2.repr_hash() {
                return false;
            }
        }
        true
    }
    fn locate(mut slice: SliceData, target: &SliceData, path: Vec<u8>) -> Result<(Vec<u8>, usize)> {
        if Self::slice_eq_children(&slice, target) {
            loop {
                if Self::slice_eq_data(&slice, target) {
                    return Ok((path, slice.pos()));
                }
                if slice.get_next_bit().is_err() {
                    break;
                }
            }
        }
        for i in 0..slice.remaining_references() {
            let child = SliceData::load_cell(slice.reference(i)?)?;
            let mut next = path.clone();
            next.push(i as u8);
            if let Ok(v) = Self::locate(child, target, next) {
                return Ok(v);
            }
        }
        fail!("not found")
    }
    pub fn mark(&mut self) -> Result<()> {
        let dict_slice = SliceData::load_cell_ref(self.dict.data().unwrap())?;
        for entry in self.dict.iter() {
            let (key, mut slice) = entry?;
            let id = SliceData::load_builder(key)?.get_next_int(self.dict.bit_len())?;
            let loc = Self::locate(dict_slice.clone(), &slice, vec![])?;
            let mut loader = Loader::new(false);
            let code = loader.load(&mut slice, true)?;
            if self.map.insert(loc.0, (id, loc.1, code)).is_some() {
                fail!("non-unique path found")
            }
        }
        Ok(())
    }
    fn print_impl(&self, cell: &Cell, indent: &str, path: Vec<u8>) -> String {
        let mut text = String::new();
        text += &format!("{}.cell ", indent);
        text += &format!("{{ ;; #{}\n", cell.repr_hash().as_hex_string());
        let inner_indent = String::from("  ") + indent;
        let mut slice = SliceData::load_cell_ref(cell).unwrap();
        if let Some((id, offset, code)) = self.map.get(&path) {
            let aux = slice.get_next_slice(*offset).unwrap();
            text += &format!("{}.blob x{}\n", inner_indent, aux.as_hex_string());
            text += &format!("{};; method {}\n", inner_indent, id);
            text += &code.print(&inner_indent, true, 0);
        } else {
            if slice.remaining_bits() > 0 {
                text += &format!("{}.blob x{}\n", inner_indent, slice.as_hex_string());
            }
            for i in 0..cell.references_count() {
                let mut path = path.clone();
                path.push(i as u8);
                text += &self.print_impl(&cell.reference(i).unwrap(), inner_indent.as_str(), path);
            }
        }
        text += &format!("{}}}\n", indent);
        text
    }
    pub fn print(&self, indent: &str) -> String {
        self.print_impl(self.dict.data().unwrap(), indent, vec![])
    }
}
