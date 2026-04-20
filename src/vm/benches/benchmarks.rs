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
use criterion::{criterion_group, criterion_main, Criterion, SamplingMode};
use pprof::criterion::{Output, PProfProfiler};
use std::time::Duration;
use ton_assembler::compile_code_to_cell;
use ton_block::{Deserializable, SliceData, StateInit};
use ton_vm::{
    executor::{gas::gas_state::Gas, Engine},
    stack::{
        continuation::ContinuationData, integer::IntegerData, savelist::SaveList, Stack, StackItem,
    },
};

static DEFAULT_CAPABILITIES: u64 = 0x2e;

fn read_boc(filename: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut file = std::fs::File::open(filename).unwrap();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    bytes
}

fn load_boc(filename: &str) -> ton_block::Cell {
    let bytes = read_boc(filename);
    ton_block::read_single_root_boc(bytes).unwrap()
}

fn load_stateinit(filename: &str) -> StateInit {
    StateInit::construct_from_file(filename).unwrap()
}

fn bench_elector_algo_1000_vtors(c: &mut Criterion) {
    let elector_code = load_boc("benches/elector-code.boc");
    let elector_data = load_boc("benches/elector-data.boc");
    let config_data = load_boc("benches/config-data.boc");

    let elector_data_output = load_boc("benches/elector-data-output.boc");
    let elector_actions = load_boc("benches/elector-actions.boc");

    let mut ctrls = SaveList::default();
    ctrls.put(4, StackItem::Cell(elector_data)).unwrap();
    let params = vec![
        StackItem::int(0x76ef1ea),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(1633458077),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::tuple(vec![StackItem::int(1000000000), StackItem::None]),
        StackItem::slice(
            SliceData::from_string(
                "9fe0000000000000000000000000000000000000000000000000000000000000001_",
            )
            .unwrap(),
        ),
        StackItem::cell(config_data.reference(0).unwrap()),
        StackItem::None,
        StackItem::int(0),
    ];
    ctrls.put(7, StackItem::tuple(vec![StackItem::tuple(params)])).unwrap();

    let mut stack = Stack::new();
    stack.push(StackItem::int(1000000000));
    stack.push(StackItem::int(0));
    stack.push(StackItem::int(0));
    stack.push(StackItem::int(-2));

    let mut group = c.benchmark_group("flat-sampling");
    group.measurement_time(Duration::from_secs(10));
    group.noise_threshold(0.03);
    group.sample_size(10);
    group.sampling_mode(SamplingMode::Flat);
    group.bench_function("elector-algo-1000-vtors", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(elector_code.clone(), Some(ctrls.clone()), Some(stack.clone()), None, vec![])
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.gas_used(), 82386791);
            let output = engine.ctrl(4).unwrap().as_cell().unwrap();
            assert_eq!(output, &elector_data_output);
            let actions = engine.ctrl(5).unwrap().as_cell().unwrap();
            assert_eq!(actions, &elector_actions);
        })
    });
    group.finish();
}

fn bench_tiny_loop_200000_iters(c: &mut Criterion) {
    let tiny_code = load_boc("benches/tiny-code.boc");
    let tiny_data = load_boc("benches/tiny-data.boc");

    let mut ctrls = SaveList::default();
    ctrls.put(4, StackItem::Cell(tiny_data)).unwrap();
    let params = vec![
        StackItem::int(0x76ef1ea),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::tuple(vec![StackItem::int(1000000000), StackItem::None]),
        StackItem::default(),
        StackItem::None,
        StackItem::None,
        StackItem::int(0),
    ];
    ctrls.put(7, StackItem::tuple(vec![StackItem::tuple(params)])).unwrap();

    let mut stack = Stack::new();
    stack.push(StackItem::int(1000000000));
    stack.push(StackItem::int(0));
    stack.push(StackItem::int(0));
    stack.push(StackItem::int(-2));

    let mut group = c.benchmark_group("flat-sampling");
    group.measurement_time(Duration::from_secs(10));
    group.noise_threshold(0.03);
    group.sample_size(10);
    group.sampling_mode(SamplingMode::Flat);
    group.bench_function("tiny-loop-200000-iters", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(tiny_code.clone(), Some(ctrls.clone()), Some(stack.clone()), None, vec![])
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.gas_used(), 34000891);
            // result of computation gets verified within the test itself
        })
    });
}

fn bench_num_bigint(c: &mut Criterion) {
    c.bench_function("num-bigint", |b| {
        b.iter(|| {
            let n = num::BigInt::from(1000000);
            let mut accum = num::BigInt::from(0);
            let mut iter = num::BigInt::from(0);
            loop {
                if !(iter < n) {
                    break;
                }
                accum += num::BigInt::from(iter.bits());
                iter += 1;
            }
            assert_eq!(num::BigInt::from(18951425), accum);
        })
    });
}

// Note: the gmp-mpfr-based rug crate shows almost the same perf as num-bigint

// fn bench_rug_bigint(c: &mut Criterion) {
//     c.bench_function("rug-bigint", |b| b.iter( || {
//         let n = rug::Integer::from(1000000);
//         let mut accum = rug::Integer::from(0);
//         let mut iter = rug::Integer::from(0);
//         loop {
//             if !(iter < n) {
//                 break;
//             }
//             accum += rug::Integer::from(iter.significant_bits());
//             iter += 1;
//         }
//         assert_eq!(rug::Integer::from(18951425), accum);
//     }));
// }

fn bench_load_boc(c: &mut Criterion) {
    let bytes = read_boc("benches/elector-data.boc");
    c.bench_function("load-boc", |b| {
        b.iter(|| ton_block::read_single_root_boc(bytes.clone()).unwrap())
    });
}

const MAX_TUPLE_SIZE: usize = 255;

// array = [row1, row2, ...], array.len() <= MAX_TUPLE_SIZE
// row_i = [v1, v2, ...], row_i.len() <= row_size

fn make_array(input: &[i64], row_size: usize) -> StackItem {
    assert!(0 < row_size && row_size <= MAX_TUPLE_SIZE);
    assert!(input.len() <= row_size * MAX_TUPLE_SIZE);
    let mut row = Vec::new();
    let mut rows = Vec::new();
    for i in 0..input.len() {
        row.push(StackItem::int(input[i]));
        if (i + 1) % row_size == 0 {
            assert_eq!(row.len(), row_size);
            rows.push(StackItem::tuple(row));
            row = Vec::new();
        }
    }
    if row.len() > 0 {
        rows.push(StackItem::tuple(row));
    }
    StackItem::tuple(rows)
}

fn bench_mergesort_tuple(c: &mut Criterion) {
    let code = load_boc("benches/mergesort/mergesort.boc");
    let code_slice = SliceData::load_cell_ref(&code).unwrap();

    const ROW_SIZE: usize = 32; // size() function in the code
    const COUNT: usize = 1000; // total elements count

    let mut input = Vec::with_capacity(COUNT);
    for i in 0..COUNT {
        input.push((COUNT - i - 1) as i64);
    }
    let array = make_array(&input, ROW_SIZE);
    input.sort();
    let expected = make_array(&input, ROW_SIZE);

    // runvmx mode: +1 = same_c3
    let mut ctrls = SaveList::default();
    ctrls.put(3, StackItem::continuation(ContinuationData::with_code(code_slice.clone()))).unwrap();

    let mut stack = Stack::new();
    stack.push(StackItem::int(-1));
    stack.push(array);
    // runvmx mode: +2 = push_0
    stack.push(StackItem::int(0));

    c.bench_function("mergesort-tuple", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(code.clone(), Some(ctrls.clone()), Some(stack.clone()), None, vec![])
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.gas_used(), 51_216_096);
            assert_eq!(engine.stack().depth(), 1);
            assert_eq!(engine.stack().get(0).unwrap(), &expected);
        })
    });
}

fn bench_massive_cell_upload(c: &mut Criterion) {
    let stateinit = load_stateinit("benches/massive/cell-upload.tvc");
    let mut ctrls = SaveList::default();
    ctrls.put(4, StackItem::cell(stateinit.data().unwrap().clone())).unwrap();
    let params = vec![
        StackItem::int(0x76ef1ea),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(1678299227),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::tuple(vec![StackItem::int(1000000000), StackItem::None]),
        StackItem::default(),
        StackItem::None,
        StackItem::None,
        StackItem::int(0),
    ];
    ctrls.put(7, StackItem::tuple(vec![StackItem::tuple(params)])).unwrap();

    let msg = load_boc("benches/massive/cell-upload-msg.boc");
    let mut body = SliceData::load_cell_ref(&msg).unwrap();
    body.move_by(366).unwrap();

    let mut stack = Stack::new();
    stack.push(StackItem::int(1000000000));
    stack.push(StackItem::int(0));
    stack.push(StackItem::cell(msg));
    stack.push(StackItem::slice(body));
    stack.push(StackItem::int(-1));

    c.bench_function("massive-cell-upload", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(
                    stateinit.code().unwrap().clone(),
                    Some(ctrls.clone()),
                    Some(stack.clone()),
                    None,
                    vec![],
                )
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.gas_used(), 5479);
        })
    });
}

fn bench_massive_cell_finalize(c: &mut Criterion) {
    let stateinit = load_stateinit("benches/massive/cell-finalize.tvc");
    let mut ctrls = SaveList::default();
    ctrls.put(4, StackItem::cell(stateinit.data().unwrap().clone())).unwrap();
    let params = vec![
        StackItem::int(0x76ef1ea),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(1678296619),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::int(0),
        StackItem::tuple(vec![StackItem::int(1000000000), StackItem::None]),
        StackItem::default(),
        StackItem::None,
        StackItem::None,
        StackItem::int(0),
    ];
    ctrls.put(7, StackItem::tuple(vec![StackItem::tuple(params)])).unwrap();

    let msg = load_boc("benches/massive/cell-finalize-msg.boc");
    let mut body = SliceData::load_cell_ref(&msg).unwrap();
    body.move_by(366).unwrap();

    let mut stack = Stack::new();
    stack.push(StackItem::int(1000000000));
    stack.push(StackItem::int(0));
    stack.push(StackItem::cell(msg));
    stack.push(StackItem::slice(body));
    stack.push(StackItem::int(-1));

    c.bench_function("massive-cell-finalize", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(
                    stateinit.code().unwrap().clone(),
                    Some(ctrls.clone()),
                    Some(stack.clone()),
                    None,
                    vec![],
                )
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.gas_used(), 203585);
        })
    });
}

// evidence of independence of signature verification time from input data size
fn bench_ed25519_verify(c: &mut Criterion) {
    let secret =
        hex::decode("814f1aa234e221795562cffbe83ec8c1172675c978e81618962debff4c7b94fa").unwrap();
    let public =
        hex::decode("21765eab26c8783b73b6930d49a882034f2ea25ba45707905e60b65103d91b7e").unwrap();
    let data = hex::decode("4c1ba06d0cdf02a18b45d2ca4ce3076cf1e500598c875fdc4610aaffb465fdad35f1f4a5d31e57a96495e28d50210c76ca019a03208208ff0773193eeda315f57eda538d47d0185cb064f4b5b7d1180da43361e04bd6bded06eeb3e872718475158dae6fbc629b2ba72ea490ba7157f52cc8164d3978be333955c698292b4dae").unwrap();

    let mut signed = Vec::new();
    for size in [1, 2, 4, 8, 16, 32, 48, 64, 96, 128] {
        let mut data = data.clone();
        data.truncate(size);
        let signature = ton_block::ed25519_sign_with_secret(&secret, &data).unwrap();
        signed.push((data, signature));
    }

    let mut g = c.benchmark_group("ed25519_verify");
    for input in &signed {
        g.bench_with_input(format!("{}", input.0.len()), input, |b, input| {
            b.iter(|| ton_block::ed25519_verify(&public, &input.0, &input.1).unwrap())
        });
    }
    g.finish();
}

fn bench_chksignu(c: &mut Criterion) {
    let hash =
        hex::decode("8de120e0abffc55bf3fc723dee9e6d6bc01716064312a4e4be58be4e193fda8d").unwrap();
    let signature = SliceData::from_string("edf0554ee6f844bb7b08c91771d44c30dd69cc5b192ca2d8beff2e38b34f3d8f3c6e76b8c37c2a2fa3ea0bf082a128e2ae4c5befd941160ffcf4aed9e0d8f905").unwrap();
    let public =
        hex::decode("f5ec1345ad9adf191db35cdece12482e19a3a218e12f2d6c3e26e0ec6463d0a5").unwrap();

    let chksignu = compile_code_to_cell(
        "
        PUSHCONT {           ; 18
            BLKPUSH 3, 2     ; 26
            CHKSIGNU         ; 26
            THROWIFNOT 111   ; 34
            ; implicit RET   ; 5
        }
        REPEAT               ; 18
        ; implicit ret       ; 5
    ",
    )
    .unwrap();
    let dummy = compile_code_to_cell(
        "
        PUSHCONT {           ; 18
            BLKPUSH 3, 2     ; 26
            BLKDROP 2        ; 26
            THROWIFNOT 111   ; 34
            ; implicit RET   ; 5
        }
        REPEAT               ; 18
        ; implicit RET       ; 5
    ",
    )
    .unwrap();

    const ITERS: i64 = 10_000;
    const STEPS: u32 = 2             // enter
        + 4 * ITERS as u32           // loop
        + 1; // exit
    const GAS_USED: i64 = 18 + 18    // enter
        + (26 + 26 + 34 + 5) * ITERS // loop
        + 5; // exit

    let mut stack = Stack::new();
    stack.push(StackItem::int(IntegerData::from_unsigned_bytes_be(hash)));
    stack.push(StackItem::slice(signature));
    stack.push(StackItem::int(IntegerData::from_unsigned_bytes_be(public)));
    stack.push(StackItem::int(ITERS));

    c.bench_function("chksignu", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(chksignu.clone(), None, Some(stack.clone()), None, vec![])
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.steps(), STEPS);
            assert_eq!(engine.gas_used(), GAS_USED);
        })
    });

    c.bench_function("chksignu/dummy", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(dummy.clone(), None, Some(stack.clone()), None, vec![])
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.steps(), STEPS);
            assert_eq!(engine.gas_used(), GAS_USED);
        })
    });

    c.bench_function("chksignu/empty", |b| {
        b.iter(|| {
            let mut engine = Engine::with_capabilities(DEFAULT_CAPABILITIES)
                .setup(Default::default(), None, Some(stack.clone()), None, vec![])
                .unwrap();
            engine.execute().unwrap();
            assert_eq!(engine.steps(), 1);
            assert_eq!(engine.gas_used(), 5);
        })
    });

    // results for i9-9900k:
    // chksignu:       419.53 ms
    // chksignu/dummy: 6.2591 ms
    // chksignu/empty: 2.7092 µs -> engine overhead is insignificant

    // chksignu insn should cost such amount of gas that the following benchmark
    // shows roughly the same time as chksignu/dummy
    c.bench_function("chksignu/revised", |b| {
        b.iter(|| {
            let caps = DEFAULT_CAPABILITIES;
            let mut engine = Engine::with_capabilities(caps)
                .setup(
                    chksignu.clone(),
                    None,
                    Some(stack.clone()),
                    Some(Gas::test_with_limit(GAS_USED)),
                    vec![],
                )
                .unwrap();
            let res = engine.execute();
            assert_eq!(
                ton_vm::error::tvm_exception_code(&res.unwrap_err()).unwrap(),
                ton_block::ExceptionCode::OutOfGas
            );
        })
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets =
        bench_num_bigint,
        // bench_rug_bigint,
        bench_load_boc,
        bench_elector_algo_1000_vtors,
        bench_tiny_loop_200000_iters,
        bench_mergesort_tuple,
        bench_massive_cell_upload,
        bench_massive_cell_finalize,
        bench_ed25519_verify,
        bench_chksignu,
);
criterion_main!(benches);
