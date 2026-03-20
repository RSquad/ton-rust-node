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
use ton_block::{
    error, fail, BuilderData, Cell, ExceptionCode, GasConsumer, Result, SliceData, MAX_DATA_BITS,
};

/// Pack data as a list of single-reference cells
pub fn pack_data_to_cell(bytes: &[u8], engine: &mut dyn GasConsumer) -> Result<Cell> {
    let mut cell = BuilderData::default();
    let cell_length_in_bytes = MAX_DATA_BITS / 8;
    for cur_slice in bytes.chunks(cell_length_in_bytes).rev() {
        if cell.bits_used() != 0 {
            let mut new_cell = BuilderData::new();
            new_cell.checked_append_reference(engine.finalize_cell(cell)?)?;
            cell = new_cell;
        }
        cell.append_raw(cur_slice, cur_slice.len() * 8)?;
    }
    engine.finalize_cell(cell)
}

/// Pack string as a list of single-reference cells
pub fn pack_string_to_cell(string: &str, engine: &mut dyn GasConsumer) -> Result<Cell> {
    pack_data_to_cell(string.as_bytes(), engine)
}

/// Unpack data as a list of single-reference cells
pub fn unpack_data_from_cell(mut cell: SliceData, engine: &mut dyn GasConsumer) -> Result<Vec<u8>> {
    let mut data = vec![];
    loop {
        if !cell.remaining_bits().is_multiple_of(8) {
            fail!(
                "Cannot parse string from cell because of length of cell bits len: {}",
                cell.remaining_bits()
            )
        }
        data.extend_from_slice(&cell.get_bytestring(0));
        match cell.remaining_references() {
            0 => return Ok(data),
            1 => cell = engine.load_cell(cell.reference(0)?)?,
            _ => {
                fail!(ExceptionCode::TypeCheckError, "Incorrect representation of string in cells")
            }
        }
    }
}

pub(crate) fn bytes_to_string(data: Vec<u8>) -> Result<String> {
    String::from_utf8(data)
        .map_err(|err| error!(ExceptionCode::TypeCheckError, "Cannot create utf8 string: {}", err))
}

/// Unpack string as a list of single-reference cells
pub fn unpack_string_from_cell(cell: SliceData, engine: &mut dyn GasConsumer) -> Result<String> {
    bytes_to_string(unpack_data_from_cell(cell, engine)?)
}
