# BOC Fuzzing

Fuzz targets for `BocReader` — the BOC (Bag of Cells) deserializer that processes untrusted network input.

## Prerequisites

```bash
rustup toolchain install nightly
cargo install cargo-fuzz
```

## Targets

| Target | What it covers |
|---|---|
| `fuzz_boc_read` | `read()`, `read_root()`, `read_to_storage()` — all in-memory deserialization paths |
| `fuzz_boc_stream_read` | `stream_read()` — streaming deserialization with CRC filter |

## Usage

Run from `block/fuzz/` directory:

```bash
# Run indefinitely (Ctrl+C to stop)
cargo +nightly fuzz run fuzz_boc_read
cargo +nightly fuzz run fuzz_boc_stream_read

# Limit input size (recommended, real BOCs are small)
cargo +nightly fuzz run fuzz_boc_read -- -max_len=4096

# Time-limited run
cargo +nightly fuzz run fuzz_boc_read -- -max_total_time=300

# Use multiple jobs
cargo +nightly fuzz run fuzz_boc_read -- -jobs=4 -workers=4
```

## Reproducing a crash

```bash
cargo +nightly fuzz run fuzz_boc_read artifacts/fuzz_boc_read/<crash_file>
```

## Corpus

Seed corpus is in `corpus/` directories, pre-populated with `.boc` files from `block/src/tests/test_data/`. The fuzzer will extend it automatically during runs.
