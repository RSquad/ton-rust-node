# Config vars
clippy                     = ton_block ton_block_json emulator ton_tl
simple                     = adnl block block_json emulator nodectl node secrets-vault tl
ci_run                     = block executor vm
perf_cmd                   = "record -F 997 --call-graph dwarf,16384 -g -o ./target/perf.data"
check                      =
config                     = --release
bm_cells                   = 1000000

#### Core

.SECONDEXPANSION:
.SUFFIXES:

build:
	@cargo build $(config)

check:
	@cargo check --all --tests --release
	@cargo check --all --tests --release --features "xp25"

clippy:
	@cargo clippy --package ton_block --release --no-deps
	@cargo clippy --package ton_block_json --release --no-deps
#	@cargo clippy --package emulator --release --no-deps
	@cargo clippy --package ton_api --release --no-deps
	@cargo clippy --package ton_assembler --release --no-deps
	@cargo clippy --package lockfree --release --no-deps

docker-image:
	@docker build --force-rm --no-cache .

fmt:
	@cargo +nightly fmt $(check) --manifest-path ./adnl/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./adnl/raptorq/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./assembler/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./block/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./block-json/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./executor/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./emulator/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./lockfree/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/catchain/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/consensus-common/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/simplex/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/storage/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/tests/compat_test/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node/validator-session/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/commands/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/common/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/contracts/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/control-client/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/elections/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/nodectl/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/service/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./node-control/ton-http-api-client/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./tl/ton_api/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./tl/ton_tl_codegen/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./secrets-vault/Cargo.toml
	@cargo +nightly fmt $(check) --manifest-path ./vm/Cargo.toml
	@rustfmt +nightly $(check) --edition 2021 --config-path ./ ./common/src/config.rs
	@rustfmt +nightly $(check) --edition 2021 --config-path ./ ./common/src/info.rs
	@rustfmt +nightly $(check) --edition 2021 --config-path ./ ./common/src/log.rs
	@rustfmt +nightly $(check) --edition 2021 --config-path ./ ./common/src/test.rs
	@rustfmt +nightly $(check) --edition 2021 --config-path ./ ./common/build/build.rs

#### Tests

test: $(addprefix test-, $(simple)) $(addprefix test-ci_run-, $(ci_run))

test-ci_run-%:
	@cargo test $(what) $(config) --package ton_$* --features "ci_run" -- --nocapture

test-%:
	@cargo test $(what) $(config) --package ton_$* -- --nocapture

test-adnl:
	@cargo test test_adnl_$(what) $(config) --package adnl --features "client,server,node,telemetry" -- --nocapture
	@cargo test test_overlay_$(what) $(config) --package adnl --features "client,server,node" -- --nocapture
	@cargo test test_rldp_$(what) $(config) --package adnl --features "client,server,node,debug" -- --nocapture
	@cargo test test_quic_$(what) $(config) --package adnl --features "client,server,node,telemetry" -- --nocapture

test-catchain:
	@cargo test $(what) $(config) --package catchain -- --nocapture

test-consensus-common:
	@cargo test $(what) $(config) --package consensus-common -- --nocapture

test-node: test-storage test-catchain test-consensus-common test-simplex test-validator_session
	@cargo test $(what) $(config) --bins --features "ci_run" -- --nocapture

test-nodectl:
	$(MAKE) -C node-control test what=$(what) config=$(config)

test-secrets-vault:
	@cargo test $(what) $(config) --package secrets-vault -- --nocapture

test-simplex:
	@cargo test $(what) $(config) --package simplex -- --nocapture

test-storage:
	@cargo test $(what) $(config) --package storage -- --nocapture

test-tl:
	@cargo test $(what) $(config) --package ton_tl_codegen -- --nocapture
	@cargo test $(what) $(config) --package ton_api -- --nocapture

test-emulator:
	@cargo test $(what) $(config) --package emulator -- --nocapture

test-validator_session:
	@cargo test $(what) $(config) --package validator_session -- --nocapture

xp25-test: xp25-test-adnl xp25-test-block xp25-test-node

xp25-test-adnl:
	@cargo test test_adnl $(config) --package adnl --features "client,server,node,telemetry,xp25" -- --nocapture
	@cargo test test_overlay $(config) --package adnl --features "client,server,node, xp25" -- --nocapture
	@cargo test test_rldp $(config) --package adnl --features "client,server,node, xp25" -- --nocapture

xp25-test-block:
	@cargo test $(what) $(config) --package ton_block --features "ci_run,xp25" -- --nocapture

xp25-test-node:
	@cargo test $(what) $(config) --bins --features "ci_run,xp25" -- --nocapture

test-overlay:
	@cargo test --test test_overlay --release --package adnl --features "client,server,node" -- --test-threads=1 --nocapture

#### Tools

cells_benchmark:
	@cargo build --release --bin benchmark
	target/release/benchmark generate-boc target/boc1.boc $(bm_cells)
	target/release/benchmark generate-boc target/boc2.boc $(bm_cells)
	target/release/benchmark generate-boc target/boc3.boc $(bm_cells)
	target/release/benchmark apply-boc target/boc1.boc ./target/benchmark_db --cleanup
	target/release/benchmark apply-boc target/boc2.boc ./target/benchmark_db
	target/release/benchmark apply-boc target/boc3.boc ./target/benchmark_db
	rm -rf ./target/benchmark_db
	rm target/boc*.boc

console:
	@cargo run $(config) --bin console -- --config ./devnet/console.json

key:
	@cargo run $(config) --bin crypto -- gen key

keyid:
	@cargo run $(config) --bin crypto -- get adnl-id --secret $(what)

resolve:
	@cargo run $(config) --bin adnl_resolve $(what)

run:
	@PID=$$(ps ax | grep "node --configs" | grep -v grep | awk '{print $$1}'); \
	echo $$PID; \
	[ -n "$$PID" ] && kill $$PID 2>/dev/null || true; \
	while [ -n "$$PID" ] && kill -0 $$PID 2>/dev/null; do sleep 0.1; done
	@cargo run --bin node $(config) -- --configs ./target > ./target/log 2>&1 &

tag:
	$(eval TAG = $(if $(what),$(what),latest))
	@-git tag -d $(TAG)
	@-git push --delete origin $(TAG)
	@git tag $(TAG)
	@git push origin $(TAG)

bench:
	@cargo bench -p adnl --features debug

profile:
	bench=$$(ls -t ./target/release/deps/bench_rldp-* | head -n 1) && flamegraph -v -o ./target/rldp.svg -c $(perf_cmd) -- $$bench

bin:
	@cargo build --bin $(what) $(config) --features export_key

bins:
	@cargo build --bins $(config) --features export_key

check_features:
	@echo "default features"
	@cargo check
	@echo "no-default-features"
	@cargo check --no-default-features
	@echo "default features + xp25"
	@cargo check --features "xp25"

package:
	@cargo run $(config) --bin print -- --package $(what)

#### Supplementary

help:
	@echo "Core:"
	@echo "  build              - Build the project"
	@echo "  check              - Compilation check"
	@echo "  check_features     - Check with different feature sets"
	@echo "  clippy             - Run Clippy lints"
	@echo "  docker-image       - Build Docker image"
	@echo "  fmt                - Run cargo fmt"
	@echo "  run                - Run node locally"
	@echo "  test               - Run all tests"
	@echo ""
	@echo "Tools:"
	@echo "  bench              - Run ADNL benchmarks"
	@echo "  bin what=<name>    - Build specific binary"
	@echo "  bins               - Build all binaries"
	@echo "  cells_benchmark    - Run cells benchmark"
	@echo "  console            - Run console tool"
	@echo "  key                - Generate key"
	@echo "  keyid what=<secret>- Get ADNL ID from secret key"
	@echo "  package what=<..>  - Print package"
	@echo "  profile            - Run flamegraph profiling"
	@echo "  resolve what=<..>  - Resolve ADNL address"
	@echo "  tag what=<tag>     - Tag and push"
