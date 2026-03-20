/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use anyhow::Context;
use num::Num;
use ton_api::ton::tvm::StackEntry;
use ton_block::{Cell, read_single_root_boc};

#[derive(Debug)]
pub struct TvmStackParser {
    pub stack: Vec<StackEntry>,
}

impl TvmStackParser {
    pub fn new(stack: Vec<StackEntry>) -> Self {
        Self { stack }
    }

    pub fn decimal_string(&self, index: usize) -> anyhow::Result<&str> {
        let entry = self.entry(index)?;
        let decimal = entry
            .number()
            .ok_or(anyhow::anyhow!(
                "stack entry is not a number: index={}, entry={:?}",
                index,
                entry
            ))?
            .number();
        Ok(decimal)
    }

    pub fn number_bytes(&self, index: usize, size: usize) -> anyhow::Result<Vec<u8>> {
        let decimal = self.decimal_string(index)?;
        let number = if decimal.starts_with("0x") {
            num::BigUint::from_str_radix(decimal.get(2..).unwrap(), 16)
        } else {
            num::BigUint::from_str_radix(decimal, 10)
        }
        .map_err(|e| anyhow::anyhow!("Failed to parse decimal string: {}: {}", decimal, e))?;
        let mut bytes = number.to_bytes_be();
        let mut result = vec![0u8; size];
        result.fill(0);
        let start = if bytes.len() <= size {
            size - bytes.len()
        } else {
            bytes.truncate(size);
            0
        };
        result[start..].copy_from_slice(&bytes);
        Ok(result)
    }

    /// Get a slice of the stack entry at the given index
    pub fn cell(&self, index: usize) -> anyhow::Result<Cell> {
        let entry = self.entry(index)?;
        let boc = entry
            .cell()
            .ok_or_else(|| anyhow::anyhow!("stack entry is not a cell: index={}", index))?;
        let cell = read_single_root_boc(&boc.bytes)
            .map_err(|e| anyhow::anyhow!("invalid boc: index={}: {}", index, e))?;
        Ok(cell)
    }

    pub fn cell_opt(&self, index: usize) -> anyhow::Result<Option<Cell>> {
        let entry = self.entry(index)?;
        if let StackEntry::Tvm_StackEntryUnsupported = entry {
            return Ok(None);
        }
        let boc = entry
            .cell()
            .ok_or_else(|| anyhow::anyhow!("stack entry is not a cell: index={}", index))?;
        let cell = read_single_root_boc(&boc.bytes)
            .map_err(|e| anyhow::anyhow!("invalid boc: index={}: {}", index, e))?;
        Ok(Some(cell))
    }

    pub fn i64(&self, index: usize) -> anyhow::Result<i64> {
        let mut decimal = self.decimal_string(index)?;
        let minus = if decimal.starts_with("-") {
            decimal = decimal
                .get(1..)
                .ok_or_else(|| anyhow::anyhow!("invalid number format: {}", decimal))?;
            if decimal.is_empty() {
                return Err(anyhow::anyhow!("invalid number format: {}", decimal));
            }
            true
        } else {
            false
        };
        let num = if decimal.starts_with("0x") {
            let hex_str = decimal
                .get(2..)
                .ok_or_else(|| anyhow::anyhow!("invalid hex format: {}", decimal))?;
            if hex_str.is_empty() {
                return Err(anyhow::anyhow!("invalid hex format: {}", decimal));
            }
            i64::from_str_radix(hex_str, 16)
                .context(format!("parse i64 from hex: item={}", index))?
        } else {
            if decimal.is_empty() {
                return Err(anyhow::anyhow!("invalid number format: empty number string"));
            }
            decimal.parse::<i64>().context(format!("parse i64 from decimal: item={}", index))?
        };
        Ok(if minus { -num } else { num })
    }

    pub fn bool(&self, index: usize) -> anyhow::Result<bool> {
        Ok(self.i64(index)? != 0)
    }

    pub fn list(&self, index: usize) -> anyhow::Result<TvmStackParser> {
        let entry = self.stack.get(index).ok_or_else(|| {
            anyhow::anyhow!("stack index out of bounds: index={}, len={}", index, self.stack.len())
        })?;
        Ok(Self::new(
            entry.list().ok_or(anyhow::anyhow!("stack entry is not a list"))?.elements().clone(),
        ))
    }

    pub fn list_or_empty(&self, index: usize) -> anyhow::Result<TvmStackParser> {
        let entry = self.entry(index)?;
        match entry {
            StackEntry::Tvm_StackEntryList(list) => Ok(Self::new(list.list.elements().clone())),
            StackEntry::Tvm_StackEntryUnsupported => Ok(Self::new(Vec::new())),
            _ => Err(anyhow::anyhow!("stack entry is not a list: index={}", index)),
        }
    }

    pub fn tuple(&self, index: usize) -> anyhow::Result<TvmStackParser> {
        let entry = self.stack.get(index).ok_or_else(|| {
            anyhow::anyhow!("stack index out of bounds: index={}, len={}", index, self.stack.len())
        })?;
        Ok(Self::new(
            entry
                .tuple()
                .ok_or(anyhow::anyhow!("stack entry is not a tuple: index={}", index))?
                .elements()
                .clone(),
        ))
    }

    fn entry(&self, index: usize) -> anyhow::Result<&StackEntry> {
        self.stack.get(index).ok_or_else(|| {
            anyhow::anyhow!("stack index out of bounds: index={}, len={}", index, self.stack.len())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ton_api::ton::tvm::{
        List, Number, Tuple, cell, list,
        numberdecimal::NumberDecimal,
        stackentry::{StackEntryCell, StackEntryList, StackEntryNumber, StackEntryTuple},
        tuple,
    };
    use ton_block::{BuilderData, Cell, IBitstring, write_boc};

    fn create_number_entry(value: &str) -> StackEntry {
        StackEntry::Tvm_StackEntryNumber(StackEntryNumber {
            number: Number::Tvm_NumberDecimal(NumberDecimal { number: value.to_string() }),
        })
    }

    fn create_list_entry(elements: Vec<StackEntry>) -> StackEntry {
        StackEntry::Tvm_StackEntryList(StackEntryList {
            list: List::Tvm_List(list::List { elements }),
        })
    }

    fn create_tuple_entry(elements: Vec<StackEntry>) -> StackEntry {
        StackEntry::Tvm_StackEntryTuple(StackEntryTuple {
            tuple: Tuple::Tvm_Tuple(tuple::Tuple { elements }),
        })
    }

    fn create_unsupported_entry() -> StackEntry {
        StackEntry::Tvm_StackEntryUnsupported
    }

    fn create_cell_entry(cell: &Cell) -> StackEntry {
        let boc = write_boc(cell).unwrap();
        StackEntry::Tvm_StackEntryCell(StackEntryCell { cell: cell::Cell { bytes: boc } })
    }

    #[test]
    fn test_new() {
        let stack = vec![create_number_entry("123"), create_number_entry("456")];
        let parser = TvmStackParser::new(stack.clone());
        assert_eq!(parser.stack.len(), 2);
    }

    #[test]
    fn test_decimal_string_success() {
        let stack = vec![
            create_number_entry("123"),
            create_number_entry("0x456"),
            create_number_entry("-789"),
            create_number_entry("-0x1"),
        ];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.decimal_string(0).unwrap(), "123");
        assert_eq!(parser.decimal_string(1).unwrap(), "0x456");
        assert_eq!(parser.decimal_string(2).unwrap(), "-789");
        assert_eq!(parser.decimal_string(3).unwrap(), "-0x1");
    }

    #[test]
    fn test_decimal_string_error_not_number() {
        let stack = vec![create_list_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.decimal_string(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack entry is not a number"));
    }

    #[test]
    fn test_decimal_string_error_index_out_of_bounds() {
        let stack = vec![create_number_entry("123")];
        let parser = TvmStackParser::new(stack);

        let result = parser.decimal_string(1);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack index out of bounds"));
    }

    #[test]
    fn test_bytes_success_exact_size() {
        let stack = vec![create_number_entry("255")];
        let parser = TvmStackParser::new(stack);

        let result = parser.number_bytes(0, 1).unwrap();
        assert_eq!(result, vec![255u8]);
    }

    #[test]
    fn test_bytes_success_with_padding() {
        let stack = vec![create_number_entry("255")];
        let parser = TvmStackParser::new(stack);

        let result = parser.number_bytes(0, 4).unwrap();
        assert_eq!(result, vec![0u8, 0u8, 0u8, 255u8]);
    }

    #[test]
    fn test_bytes_success_large_number() {
        let stack = vec![create_number_entry("65535")];
        let parser = TvmStackParser::new(stack);

        let result = parser.number_bytes(0, 2).unwrap();
        assert_eq!(result, vec![255u8, 255u8]);
    }

    #[test]
    fn test_bytes_success_truncation() {
        let stack = vec![create_number_entry("16777215")]; // 0xFFFFFF
        let parser = TvmStackParser::new(stack);

        let result = parser.number_bytes(0, 2).unwrap();
        assert_eq!(result, vec![255u8, 255u8]);
    }

    #[test]
    fn test_bytes_success_zero() {
        let stack = vec![create_number_entry("0")];
        let parser = TvmStackParser::new(stack);

        let result = parser.number_bytes(0, 4).unwrap();
        assert_eq!(result, vec![0u8, 0u8, 0u8, 0u8]);
    }

    #[test]
    fn test_bytes_error_invalid_decimal() {
        let stack = vec![create_number_entry("invalid")];
        let parser = TvmStackParser::new(stack);

        let result = parser.number_bytes(0, 4);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to parse decimal string"));
    }

    #[test]
    fn test_i64_success_decimal_positive() {
        let stack = vec![create_number_entry("12345")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), 12345);
    }

    #[test]
    fn test_i64_success_decimal_negative() {
        let stack = vec![create_number_entry("-12345")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), -12345);
    }

    #[test]
    fn test_i64_success_hex_positive() {
        let stack = vec![create_number_entry("0xFF")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), 255);
    }

    #[test]
    fn test_i64_success_hex_negative() {
        let stack = vec![create_number_entry("-0x1")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), -1);
    }

    #[test]
    fn test_i64_success_zero() {
        let stack = vec![create_number_entry("0")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), 0);
    }

    #[test]
    fn test_i64_success_max_i64() {
        let stack = vec![create_number_entry("9223372036854775807")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), i64::MAX);
    }

    #[test]
    fn test_i64_success_min_i64() {
        // Use MIN + 1 because parsing MIN requires parsing a number larger than i64::MAX
        // which fails. This tests the boundary case without the overflow issue.
        let stack = vec![create_number_entry("-9223372036854775807")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.i64(0).unwrap(), i64::MIN + 1);
    }

    #[test]
    fn test_i64_error_min_i64_overflow() {
        // Test that i64::MIN causes an overflow error when parsed
        let stack = vec![create_number_entry("-9223372036854775808")];
        let parser = TvmStackParser::new(stack);

        let result = parser.i64(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse i64 from decimal"));
    }

    #[test]
    fn test_i64_error_invalid_decimal() {
        let stack = vec![create_number_entry("invalid")];
        let parser = TvmStackParser::new(stack);

        let result = parser.i64(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse i64 from decimal"));
    }

    #[test]
    fn test_i64_error_invalid_hex() {
        let stack = vec![create_number_entry("0xGG")];
        let parser = TvmStackParser::new(stack);

        let result = parser.i64(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parse i64 from hex"));
    }

    #[test]
    fn test_i64_error_malformed_minus_only() {
        let stack = vec![create_number_entry("-")];
        let parser = TvmStackParser::new(stack);

        let result = parser.i64(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid number format"));
    }

    #[test]
    fn test_i64_error_malformed_hex_prefix_only() {
        let stack = vec![create_number_entry("0x")];
        let parser = TvmStackParser::new(stack);

        let result = parser.i64(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid hex format"));
    }

    #[test]
    fn test_i64_error_malformed_minus_hex_prefix_only() {
        let stack = vec![create_number_entry("-0x")];
        let parser = TvmStackParser::new(stack);

        let result = parser.i64(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid hex format"));
    }

    #[test]
    fn test_bool_success_true() {
        let stack = vec![create_number_entry("1")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.bool(0).unwrap(), true);
    }

    #[test]
    fn test_bool_success_false() {
        let stack = vec![create_number_entry("0")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.bool(0).unwrap(), false);
    }

    #[test]
    fn test_bool_success_non_zero() {
        let stack = vec![create_number_entry("42")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.bool(0).unwrap(), true);
    }

    #[test]
    fn test_bool_success_negative() {
        let stack = vec![create_number_entry("-1")];
        let parser = TvmStackParser::new(stack);

        assert_eq!(parser.bool(0).unwrap(), true);
    }

    #[test]
    fn test_list_success() {
        let inner_elements =
            vec![create_number_entry("1"), create_number_entry("2"), create_number_entry("3")];
        let stack = vec![create_list_entry(inner_elements.clone())];
        let parser = TvmStackParser::new(stack);

        let result = parser.list(0).unwrap();
        assert_eq!(result.stack.len(), 3);
    }

    #[test]
    fn test_list_success_empty() {
        let stack = vec![create_list_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.list(0).unwrap();
        assert_eq!(result.stack.len(), 0);
    }

    #[test]
    fn test_list_or_empty_unsupported() {
        let stack = vec![create_unsupported_entry()];
        let parser = TvmStackParser::new(stack);

        let result = parser.list_or_empty(0).unwrap();
        assert_eq!(result.stack.len(), 0);
    }

    #[test]
    fn test_list_error_not_list() {
        let stack = vec![create_number_entry("123")];
        let parser = TvmStackParser::new(stack);

        let result = parser.list(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack entry is not a list"));
    }

    #[test]
    fn test_list_error_index_out_of_bounds() {
        let stack = vec![create_list_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.list(1);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack index out of bounds"));
    }

    #[test]
    fn test_list_nested() {
        let inner_list = create_list_entry(vec![create_number_entry("42")]);
        let outer_elements = vec![inner_list];
        let stack = vec![create_list_entry(outer_elements)];
        let parser = TvmStackParser::new(stack);

        let result = parser.list(0).unwrap();
        assert_eq!(result.stack.len(), 1);
    }

    #[test]
    fn test_tuple_success() {
        let inner_elements = vec![create_number_entry("10"), create_number_entry("20")];
        let stack = vec![create_tuple_entry(inner_elements.clone())];
        let parser = TvmStackParser::new(stack);

        let result = parser.tuple(0).unwrap();
        assert_eq!(result.stack.len(), 2);
    }

    #[test]
    fn test_tuple_success_empty() {
        let stack = vec![create_tuple_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.tuple(0).unwrap();
        assert_eq!(result.stack.len(), 0);
    }

    #[test]
    fn test_tuple_error_not_tuple() {
        let stack = vec![create_number_entry("123")];
        let parser = TvmStackParser::new(stack);

        let result = parser.tuple(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack entry is not a tuple"));
    }

    #[test]
    fn test_tuple_error_index_out_of_bounds() {
        let stack = vec![create_tuple_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.tuple(1);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack index out of bounds"));
    }

    #[test]
    fn test_tuple_nested() {
        let inner_tuple = create_tuple_entry(vec![create_number_entry("99")]);
        let outer_elements = vec![inner_tuple];
        let stack = vec![create_tuple_entry(outer_elements)];
        let parser = TvmStackParser::new(stack);

        let result = parser.tuple(0).unwrap();
        assert_eq!(result.stack.len(), 1);
    }

    #[test]
    fn test_complex_workflow() {
        // Test a complex scenario: list containing tuples with numbers
        let tuple1 = create_tuple_entry(vec![create_number_entry("1"), create_number_entry("2")]);
        let tuple2 = create_tuple_entry(vec![create_number_entry("3"), create_number_entry("4")]);
        let list = create_list_entry(vec![tuple1, tuple2]);
        let stack = vec![list];
        let parser = TvmStackParser::new(stack);

        // Get the list
        let list_parser = parser.list(0).unwrap();
        assert_eq!(list_parser.stack.len(), 2);

        // Get first tuple from list
        let tuple1_parser = list_parser.tuple(0).unwrap();
        assert_eq!(tuple1_parser.stack.len(), 2);
        assert_eq!(tuple1_parser.i64(0).unwrap(), 1);
        assert_eq!(tuple1_parser.i64(1).unwrap(), 2);

        // Get second tuple from list
        let tuple2_parser = list_parser.tuple(1).unwrap();
        assert_eq!(tuple2_parser.stack.len(), 2);
        assert_eq!(tuple2_parser.i64(0).unwrap(), 3);
        assert_eq!(tuple2_parser.i64(1).unwrap(), 4);
    }

    #[test]
    fn test_cell_success_empty_cell() {
        let cell = Cell::default();
        let stack = vec![create_cell_entry(&cell)];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0).unwrap();
        assert_eq!(result.repr_hash(), cell.repr_hash());
    }

    #[test]
    fn test_cell_success_with_data() {
        let mut builder = BuilderData::new();
        builder.append_u32(0xDEADBEEF).unwrap();
        builder.checked_append_reference(Cell::default()).unwrap();
        let cell = builder.into_cell().unwrap();
        let stack = vec![create_cell_entry(&cell)];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0).unwrap();
        assert_eq!(result.repr_hash(), cell.repr_hash());
    }

    #[test]
    fn test_cell_success_multiple_cells() {
        let cell1 = Cell::default();
        let mut builder = BuilderData::new();
        builder.append_u32(0x12345678).unwrap();
        let cell2 = builder.into_cell().unwrap();

        let stack = vec![create_cell_entry(&cell1), create_cell_entry(&cell2)];
        let parser = TvmStackParser::new(stack);

        let result1 = parser.cell(0).unwrap();
        let result2 = parser.cell(1).unwrap();
        assert_eq!(result1.repr_hash(), cell1.repr_hash());
        assert_eq!(result2.repr_hash(), cell2.repr_hash());
    }

    #[test]
    fn test_cell_error_index_out_of_bounds() {
        let cell = Cell::default();
        let stack = vec![create_cell_entry(&cell)];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(1);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack index out of bounds"));
    }

    #[test]
    fn test_cell_error_not_cell_number() {
        let stack = vec![create_number_entry("123")];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack entry is not a cell"));
    }

    #[test]
    fn test_cell_error_not_cell_list() {
        let stack = vec![create_list_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack entry is not a cell"));
    }

    #[test]
    fn test_cell_error_not_cell_tuple() {
        let stack = vec![create_tuple_entry(vec![])];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("stack entry is not a cell"));
    }

    #[test]
    fn test_cell_error_invalid_boc() {
        let invalid_cell_entry = StackEntry::Tvm_StackEntryCell(StackEntryCell {
            cell: cell::Cell { bytes: vec![0xFF, 0xFF, 0xFF] },
        });
        let stack = vec![invalid_cell_entry];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid boc"));
    }

    #[test]
    fn test_cell_error_empty_boc() {
        let empty_cell_entry =
            StackEntry::Tvm_StackEntryCell(StackEntryCell { cell: cell::Cell { bytes: vec![] } });
        let stack = vec![empty_cell_entry];
        let parser = TvmStackParser::new(stack);

        let result = parser.cell(0);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid boc"));
    }
}
