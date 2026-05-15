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
use clap::{Parser, Subcommand};
use std::{collections::HashSet, io::Write, process::ExitCode};
use ton_assembler::disasm::{disasm_ex, fmt::print_tree_of_cells, loader::Loader};
use ton_block::{error, fail, read_boc, write_boc, Cell, SliceData, Status};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Dump a boc as a tree of bitstrings
    Dump {
        /// input boc
        boc: String,
    },
    /// Extract one cell from a boc
    Extract {
        /// cell index (from 0 to 3)
        index: usize,
        /// input boc
        boc: String,
        /// output boc
        output_boc: String,
        /// root index (0 by default)
        #[arg(short, long)]
        root: Option<usize>,
    },
    /// Disassemble a code fragment
    Fragment {
        /// bitstring
        bitstring: String,
    },
    /// Disassemble a code boc
    Text {
        /// input boc
        boc: String,
        /// interpret the boc as StateInit and take the code cell
        #[arg(short, long)]
        stateinit: bool,
        /// print full assembler listing w/o collapsing of identical cells
        #[arg(short, long)]
        full: bool,
    },
}

fn main() -> ExitCode {
    if let Err(e) = main_impl() {
        eprintln!("{}", e);
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

fn main_impl() -> Status {
    let cli = Cli::parse();
    match cli.command {
        Commands::Dump { boc } => subcommand_dump(boc),
        Commands::Extract { boc, output_boc, index, root } => {
            subcommand_extract(boc, output_boc, index, root)
        }
        Commands::Fragment { bitstring } => subcommand_fragment(bitstring),
        Commands::Text { boc, stateinit, full } => subcommand_text(boc, stateinit, full),
    }
}

fn subcommand_dump(filename: String) -> Status {
    let tvc = std::fs::read(filename).map_err(|e| error!("failed to read boc file: {}", e))?;
    let roots = read_boc(tvc).map_err(|e| error!("{}", e))?.roots;
    if roots.is_empty() {
        println!("empty");
    } else {
        println!("{} {} in total", roots.len(), if roots.len() > 1 { "roots" } else { "root" });
        for i in 0..roots.len() {
            let root = roots.get(i).unwrap();
            let mut count = 0;
            let mut queue = vec![root.clone()];
            while let Some(cell) = queue.pop() {
                if count == usize::MAX {
                    fail!("cells count exceeds max {}", usize::MAX)
                }
                count += 1;
                let count = cell.references_count();
                for i in 0..count {
                    queue.push(cell.reference(i)?);
                }
            }
            println!(
                "root {} ({} {}, {} unique):",
                i,
                count,
                if count > 1 { "cells" } else { "cell" },
                count_unique_cells(root)
            );
            print_tree_of_cells(root);
        }
    }
    Ok(())
}

fn count_unique_cells(cell: &Cell) -> usize {
    let mut queue = vec![cell.clone()];
    let mut set = HashSet::new();
    while let Some(cell) = queue.pop() {
        if set.insert(cell.repr_hash().clone()) {
            let count = cell.references_count();
            for i in 0..count {
                queue.push(cell.reference(i).unwrap());
            }
        }
    }
    set.len()
}

fn subcommand_extract(
    filename: String,
    output: String,
    index: usize,
    root: Option<usize>,
) -> Status {
    let boc = std::fs::read(filename).map_err(|e| error!("failed to read input file: {}", e))?;
    let roots = read_boc(boc).map_err(|e| error!("{}", e))?.roots;

    let root_index = root.unwrap_or_default();
    let root = roots.get(root_index).ok_or_else(|| error!("failed to get root {}", root_index))?;

    let cell = root.reference(index)?;

    let output_bytes = write_boc(&cell)?;
    let mut output_file = std::fs::File::create(output)?;
    output_file.write_all(&output_bytes)?;

    Ok(())
}

fn subcommand_fragment(fragment: String) -> Status {
    let cell = SliceData::from_string(&fragment)?.into_cell()?;
    let mut slice = SliceData::load_cell(cell)?;
    let mut loader = Loader::new(false);
    let code = loader.load(&mut slice, false)?;
    let text = code.print("", true, 12);
    print!("{}", text);
    Ok(())
}

fn subcommand_text(filename: String, stateinit: bool, full: bool) -> Status {
    let boc = std::fs::read(filename).map_err(|e| error!("failed to read input file: {}", e))?;
    let roots = read_boc(boc).map_err(|e| error!("{}", e))?.roots;

    let roots_count = roots.len();
    if roots_count == 0 {
        println!("boc is empty");
        return Ok(());
    } else if roots_count > 1 {
        println!("warning: boc contains {} roots, getting the first one", roots_count)
    }

    let root0 = roots.first().ok_or_else(|| error!("failed to get root 0"))?;
    let cell = if stateinit { root0.reference(0)? } else { root0.clone() };

    print!("{}", disasm_ex(&mut SliceData::load_cell(cell)?, !full)?);
    Ok(())
}
