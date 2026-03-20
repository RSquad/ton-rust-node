/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use ton_block::{
    error, fail, read_single_root_boc, write_boc, BuilderData, Cell, GasConsumer, HashmapE, Result,
    SliceData, Status,
};

fn read_boc(filename: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut file = std::fs::File::open(filename).unwrap();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    bytes
}

fn bench_boc_read(c: &mut criterion::Criterion) {
    let bytes = read_boc("src/tests/data/medium.boc");
    c.bench_function("boc-read", |b| {
        b.iter(|| {
            criterion::black_box(read_single_root_boc(bytes.clone()).unwrap());
        })
    });
}

fn bench_boc_write(c: &mut criterion::Criterion) {
    let bytes = read_boc("src/tests/data/medium.boc");
    let cell = read_single_root_boc(bytes).unwrap();
    let mut g = c.benchmark_group("bench");
    g.measurement_time(std::time::Duration::new(15, 0));
    g.bench_function("boc-write", |b| {
        b.iter(|| {
            criterion::black_box(write_boc(&cell).unwrap());
        })
    });
}

enum OperationType {
    New { bits: usize, cell: Option<Cell> },
    Get { key: SliceData },
    Set { key: SliceData, value: SliceData },
    SetBuilder { key: SliceData, value: BuilderData },
    Remove { key: SliceData },
    GetMinMax { min: bool, signed: bool },
    FindLeaf { key: SliceData, next: bool, eq: bool, signed: bool },
}

struct Operation {
    id: usize,
    typ: OperationType,
}

impl Operation {
    fn new(id: usize, typ: OperationType) -> Self {
        Self { id, typ }
    }
}

struct Plan {
    ops: Vec<Operation>,
    max_id: usize,
}

impl Plan {
    fn new() -> Self {
        Self { ops: Vec::new(), max_id: 0 }
    }
    fn push(&mut self, op: Operation) {
        self.max_id = std::cmp::max(self.max_id, op.id);
        self.ops.push(op);
    }
}

fn r(hex: &str) -> Cell {
    read_single_root_boc(hex::decode(hex).unwrap()).unwrap()
}

fn load_plan(filename: &str) -> Result<Plan> {
    let mut plan = Plan::new();
    for (lineno, line) in std::fs::read_to_string(filename)?.lines().enumerate() {
        let fields = line.split(" ").collect::<Vec<_>>();
        let id = fields[0].parse::<usize>()?;
        use OperationType::*;
        let typ = match fields[1] {
            "with_hashmap" => New {
                bits: fields[2].parse::<usize>()?,
                cell: (fields.len() == 4).then(|| r(fields[3])),
            },
            "set_with_gas" => Set {
                key: SliceData::load_cell(r(fields[2]))?,
                value: SliceData::load_cell(r(fields[3]))?,
            },
            "set_builder_with_gas" => SetBuilder {
                key: SliceData::load_cell(r(fields[2]))?,
                value: BuilderData::from_cell(&r(fields[3]))?,
            },
            "get_with_gas" => Get { key: SliceData::load_cell(r(fields[2]))? },
            "remove_with_gas" => Remove { key: SliceData::load_cell(r(fields[2]))? },
            "get_min_max" => {
                GetMinMax { min: fields[2].parse::<bool>()?, signed: fields[3].parse::<bool>()? }
            }
            "find_leaf" => FindLeaf {
                key: SliceData::load_cell(r(fields[2]))?,
                next: fields[3].parse::<bool>()?,
                eq: fields[4].parse::<bool>()?,
                signed: fields[5].parse::<bool>()?,
            },
            op => fail!("unknown operation {} at line {}", op, lineno + 1),
        };
        plan.push(Operation::new(id, typ));
    }
    Ok(plan)
}

fn execute_plan(plan: &Plan) -> Status {
    let mut gas_consumer = TrivialGasConsumer {};
    let mut hashmaps = vec![HashmapE::with_bit_len(0); plan.max_id + 1];
    for op in &plan.ops {
        let hashmap = hashmaps.get_mut(op.id).ok_or(error!("invalid hashmap id"))?;
        use OperationType::*;
        match &op.typ {
            New { bits, cell } => {
                *hashmap = HashmapE::with_hashmap(*bits, cell.clone());
            }
            Get { key } => {
                hashmap.get_with_gas(key.clone(), &mut gas_consumer)?;
            }
            Set { key, value } => {
                hashmap.set_with_gas(key.clone(), value, &mut gas_consumer)?;
            }
            SetBuilder { key, value } => {
                hashmap.set_builder_with_gas(key.clone(), value, &mut gas_consumer)?;
            }
            Remove { key } => {
                hashmap.remove_with_gas(key.clone(), &mut gas_consumer)?;
            }
            GetMinMax { min, signed } => {
                hashmap.get_min_max(*min, *signed, &mut gas_consumer)?;
            }
            FindLeaf { key, next, eq, signed } => {
                hashmap.find_leaf(key.clone(), *next, *eq, *signed, &mut gas_consumer)?;
            }
        }
    }
    Ok(())
}

struct TrivialGasConsumer {}

impl GasConsumer for TrivialGasConsumer {
    fn finalize_cell(&mut self, builder: BuilderData) -> Result<Cell> {
        builder.finalize(1024)
    }
    fn load_cell(&mut self, cell: Cell) -> Result<SliceData> {
        SliceData::load_cell(cell)
    }
    fn finalize_cell_and_load(&mut self, builder: BuilderData) -> Result<SliceData> {
        let cell = self.finalize_cell(builder)?;
        self.load_cell(cell)
    }
}

fn bench_hashmap(c: &mut criterion::Criterion) {
    let plan = load_plan("benches/hashmap-plan.txt").unwrap();
    c.bench_function("hashmap", |b| b.iter(|| execute_plan(&plan).unwrap()));
}

criterion::criterion_group!(
    name = benches;
    config = criterion::Criterion::default()
        .with_profiler(pprof::criterion::PProfProfiler::new(
            100,
            pprof::criterion::Output::Flamegraph(None)
        ));
    targets =
        bench_boc_read,
        bench_boc_write,
        bench_hashmap,
);
criterion::criterion_main!(benches);
