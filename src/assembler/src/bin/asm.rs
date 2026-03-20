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
use clap::Parser;
use std::{error::Error, io::Write, process::ExitCode};
use ton_assembler::{DbgInfo, Engine, Units};
use ton_block::Cell;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input assembly sources
    #[arg(required = true)]
    inputs: Vec<String>,
    /// Output boc filename ("output.boc" by default)
    #[arg(short, long)]
    boc: Option<String>,
    /// Output debug map filename ("output.debug.json" by default)
    #[arg(short, long)]
    dbg: Option<String>,
}

fn main() -> ExitCode {
    if let Err(e) = main_impl() {
        eprintln!("{}", e);
        ExitCode::from(1)
    } else {
        ExitCode::from(0)
    }
}

fn main_impl() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let output = args.boc.unwrap_or("output.boc".to_string());
    let dbgmap = args.dbg.unwrap_or("output.debug.json".to_string());

    let mut engine = Engine::new("");

    let mut units = Units::new();
    for input in args.inputs {
        let code = std::fs::read_to_string(input.clone())?;
        engine.reset(input);
        units = engine.compile_toplevel(&code).map_err(|e| e.to_string())?;
    }
    let (b, d) = units.finalize();

    let c = b.into_cell()?;
    write_boc(&c, &output)?;

    let dbg = DbgInfo::from(c, d);
    write_dbg(dbg, &dbgmap)?;

    Ok(())
}

fn write_boc(cell: &Cell, output: &str) -> Result<(), Box<dyn Error>> {
    let bytes = ton_block::write_boc(cell)?;
    let mut file = std::fs::File::create(output)?;
    file.write_all(&bytes)?;
    Ok(())
}

fn write_dbg(dbg: DbgInfo, output: &str) -> Result<(), Box<dyn Error>> {
    let json = serde_json::to_string_pretty(&dbg)?;
    let mut file = std::fs::File::create(output)?;
    file.write_all(json.as_bytes())?;
    Ok(())
}
