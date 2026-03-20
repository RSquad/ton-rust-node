/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use rand::Rng;
use std::collections::HashSet;
use ton_block::{BuilderData, Cell, Result};

pub fn update_boc(
    old_root: &Cell,
    need_cells: u32,
    new_cells: &mut u32,
    rng: &mut impl rand::RngCore,
) -> Result<Cell> {
    let mut new_root = old_root.clone();
    while *new_cells < need_cells {
        new_root = update_boc_(&new_root, (need_cells as f32 * 2.5) as u32, new_cells, rng)?;
        //println!("update_boc iteration, new_cells: {new_cells}");
    }
    Ok(new_root)
}

pub fn update_boc_(
    old_root: &Cell,
    need_cells: u32,
    new_cells: &mut u32,
    rng: &mut impl rand::RngCore,
) -> Result<Cell> {
    let mut builder = BuilderData::new();
    let bits = rng.gen_range(0..=1023);
    let mut data = vec![0_u8; (bits + 7) / 8];
    rng.fill(&mut data[..]);
    builder.append_raw(&data, bits)?;

    for child in old_root.clone_references() {
        if rng.gen_range(0..need_cells) < *new_cells {
            builder.checked_append_reference(child)?;
        } else {
            *new_cells += 1;
            let child = update_boc_(&child, need_cells, new_cells, rng)?;
            builder.checked_append_reference(child)?;
        }
    }

    builder.into_cell()
}

/// Creates a BOC with random topology and specifed number of cells
pub fn generate_boc(need_cells: u32, rng: &mut impl rand::RngCore) -> Cell {
    println!("generate BOC with about {} cells", need_cells);

    let bottom_level_cells = (need_cells as f32 * 0.08) as usize;

    // starting from the bottom level
    let mut cells: Vec<Cell> = vec![];
    let mut bottom_level = true;
    // let mut level = 0;
    let mut total_cells = 0;
    let mut hashes = HashSet::new();
    loop {
        let mut next_level_cells = vec![];
        // level += 1;
        // println!("Next level #{level} started; prev level cells: {cells}", cells = cells.len());

        while !cells.is_empty() || bottom_level && next_level_cells.len() < bottom_level_cells {
            if rng.gen_range(1..5) == 1 && !cells.is_empty() {
                next_level_cells.push(cells.pop().unwrap());
                continue;
            }
            let mut builder = BuilderData::new();
            let bits = rng.gen_range(0..=1023);
            let mut data = vec![0_u8; (bits + 7) / 8];
            rng.fill(&mut data[..]);
            builder.append_raw(&data, bits).unwrap();

            let rc = match rng.gen_range(0..100) {
                0..=40 => 2,
                41..=80 => 3,
                81..=95 => 1,
                _ => 0,
            };

            for _ in 0..rc {
                if cells.is_empty() {
                    break;
                }
                let child = if rng.gen_range(1..3) == 1 {
                    cells[rng.gen_range(0..cells.len())].clone()
                } else {
                    cells.pop().unwrap()
                };
                builder.checked_append_reference(child).unwrap();
            }

            total_cells += 1;
            let cell = builder.into_cell().unwrap();
            hashes.insert(cell.repr_hash());
            next_level_cells.push(cell);
        }
        bottom_level = false;
        cells = next_level_cells;

        if cells.len() == 1 {
            println!(
                "Tree done; needed cells {need_cells}  uniq cells {}  total cells: {total_cells}",
                hashes.len()
            );
            return cells.pop().unwrap();
        }
    }
}
