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
use crate::{debug::DbgNode, CompileResult, DbgInfo, OperationError};
use ton_block::{BuilderData, Cell, SliceData};

#[derive(Clone, Default)]
pub struct Unit {
    builder: BuilderData,
    dbg: DbgNode,
}

impl Unit {
    pub fn new(builder: BuilderData, dbg: DbgNode) -> Self {
        Self { builder, dbg }
    }
    pub fn finalize(self) -> (Cell, DbgInfo) {
        let cell = self.builder.into_cell().unwrap();
        let dbg_info = DbgInfo::from(cell.clone(), self.dbg);
        (cell, dbg_info)
    }
}

pub struct Units {
    units: Vec<Unit>,
}

impl Default for Units {
    fn default() -> Self {
        Self::new()
    }
}

impl Units {
    /// Constructor
    pub fn new() -> Self {
        Self { units: vec![Unit::default()] }
    }
    /// Writes assembled unit
    pub fn write_unit(&mut self, unit: Unit) -> CompileResult {
        self.units.push(unit);
        Ok(())
    }
    /// Writes simple command
    pub fn write_command(&mut self, command: &[u8], dbg: DbgNode) -> CompileResult {
        self.write_command_bitstring(command, command.len() * 8, dbg)
    }
    pub fn write_command_bitstring(
        &mut self,
        command: &[u8],
        bits: usize,
        dbg: DbgNode,
    ) -> CompileResult {
        if let Some(last) = self.units.last_mut() {
            let orig_offset = last.builder.bits_used();
            if last.builder.append_raw(command, bits).is_ok() {
                last.dbg.inline_node(orig_offset, dbg);
                return Ok(());
            }
        }
        if let Ok(new_last) = BuilderData::with_raw(command, bits) {
            self.units.push(Unit::new(new_last, dbg));
            return Ok(());
        }
        Err(OperationError::NotFitInSlice)
    }
    /// Writes command with additional references
    pub fn write_composite_command(
        &mut self,
        command: &[u8],
        references: Vec<BuilderData>,
        dbg: DbgNode,
    ) -> CompileResult {
        assert_eq!(references.len(), dbg.children.len());
        if let Some(mut last) = self.units.last().cloned() {
            let orig_offset = last.builder.bits_used();
            if last.builder.references_free() > references.len() // one cell remains reserved for finalization
                && last.builder.append_raw(command, command.len() * 8).is_ok()
                && checked_append_references(&mut last.builder, &references)?
            {
                last.dbg.inline_node(orig_offset, dbg);
                *self.units.last_mut().unwrap() = last;
                return Ok(());
            }
        }
        let mut new_last = BuilderData::new();
        if new_last.append_raw(command, command.len() * 8).is_ok()
            && checked_append_references(&mut new_last, &references)?
        {
            self.units.push(Unit::new(new_last, dbg));
            return Ok(());
        }
        Err(OperationError::NotFitInSlice)
    }
    /// Puts recorded cells in a linear sequence
    pub fn finalize(mut self) -> (BuilderData, DbgNode) {
        let mut cursor = self.units.pop().expect("cells can't be empty");
        while let Some(mut destination) = self.units.pop() {
            let orig_offset = destination.builder.bits_used();
            let slice =
                SliceData::load_builder(cursor.builder).expect("failed to convert builder to cell");
            // try to inline cursor into destination
            if destination.builder.checked_append_references_and_data(&slice).is_ok() {
                destination.dbg.inline_node(orig_offset, cursor.dbg);
            } else {
                // otherwise just attach cursor to destination as a reference
                destination.builder.checked_append_reference(slice.into_cell().unwrap()).unwrap();
                destination.dbg.append_node(cursor.dbg);
            }
            cursor = destination;
        }
        (cursor.builder, cursor.dbg)
    }
}

fn checked_append_references(
    builder: &mut BuilderData,
    refs: &[BuilderData],
) -> Result<bool, OperationError> {
    for reference in refs {
        if builder
            .checked_append_reference(
                reference.clone().into_cell().map_err(|_| OperationError::NotFitInSlice)?,
            )
            .is_err()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
