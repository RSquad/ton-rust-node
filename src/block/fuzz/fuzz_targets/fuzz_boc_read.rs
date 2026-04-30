#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;
use std::sync::Arc;
use ton_block::{BocReader, Cell, CellLoader, CellsTempStorage, Result, UInt256};

struct FuzzCellsStorage {
    cells: HashMap<u32, Cell>,
    loader: CellLoader,
}

impl FuzzCellsStorage {
    fn new() -> Self {
        Self {
            cells: HashMap::new(),
            loader: Arc::new(|_| Err(anyhow::format_err!("no loader"))),
        }
    }
}

impl CellsTempStorage for FuzzCellsStorage {
    fn load_hash_and_depth(&self, index: u32) -> Result<(UInt256, u16)> {
        let cell = self
            .cells
            .get(&index)
            .ok_or_else(|| anyhow::format_err!("cell {} not found", index))?;
        Ok((cell.repr_hash().clone(), cell.repr_depth()))
    }
    fn load_cell(&self, index: u32) -> Result<Cell> {
        self.cells
            .get(&index)
            .cloned()
            .ok_or_else(|| anyhow::format_err!("cell {} not found", index))
    }
    fn store_cell(&mut self, index: u32, cell: &Cell) -> Result<()> {
        self.cells.insert(index, cell.clone());
        Ok(())
    }
    fn cleanup(&mut self) -> Result<()> {
        Ok(())
    }
    fn loader(&self) -> &CellLoader {
        &self.loader
    }
}

fuzz_target!(|data: &[u8]| {
    // In-memory read
    let _ = BocReader::new().read(data);

    // read_root (header + root cell only)
    let _ = BocReader::new().read_root(data);

    // read_to_storage
    let _ = BocReader::new().read_to_storage(data, &mut FuzzCellsStorage::new());
});
