#!/bin/sh

RUSTFLAGS="-Z instrument-coverage" LLVM_PROFILE_FILE="tvm-%m.profraw" cargo test --tests

cargo profdata -- merge -sparse tvm-*.profraw -o tvm.profdata

objects=`find target/debug/deps -executable | grep -E "/test_|/ton_vm" | xargs -n 1 echo \--object`
cargo cov -- report \
    --use-color --ignore-filename-regex='/.cargo/|rustc/' \
    --instr-profile=tvm.profdata \
    $objects
