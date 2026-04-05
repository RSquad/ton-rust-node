# Cross-Implementation Compatibility Tests Makefile

# Check if CPP_SRC_PATH is set
ifndef CPP_SRC_PATH
$(error CPP_SRC_PATH is not set. Usage: CPP_SRC_PATH=/path/to/ton-cpp-testnet make test)
endif

# Directories
ROOT_DIR := $(shell dirname $(realpath $(lastword $(MAKEFILE_LIST))))
WORKSPACE_ROOT := $(ROOT_DIR)/../../..
BUILD_DIR := $(ROOT_DIR)/build
CPP_BUILD_DIR ?= $(BUILD_DIR)/cpp
RUST_TARGET_DIR := $(BUILD_DIR)/rust

# C++ source files location
CPP_TEST_SRC := $(ROOT_DIR)/cpp_src

# Output binary
CPP_TEST_BIN := $(CPP_BUILD_DIR)/compat_test_node

# CMake settings
CMAKE_BUILD_TYPE ?= Release
CMAKE_JOBS ?= $(shell nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)

# Rust test settings
RUST_TEST_THREADS ?= 1
TEST ?=

.PHONY: all build build-cpp build-rust test test-rust clean help check-cpp-path

all: build

help:
	@echo "Cross-Implementation Compatibility Tests"
	@echo ""
	@echo "Usage: CPP_SRC_PATH=/path/to/ton-cpp-testnet make <target>"
	@echo ""
	@echo "Targets:"
	@echo "  build       - Build both C++ and Rust components"
	@echo "  build-cpp   - Build C++ test harness only"
	@echo "  build-rust  - Build Rust test code only"
	@echo "  test        - Run all compatibility tests"
	@echo "  test-rust   - Run Rust tests (requires C++ binary)"
	@echo "  clean       - Remove build artifacts"
	@echo "  help        - Show this help message"
	@echo ""
	@echo "Environment Variables:"
	@echo "  CPP_SRC_PATH     - Path to C++ TON source (required)"
	@echo "  CPP_BUILD_DIR    - C++ build directory (default: ./build/cpp)"
	@echo "  CMAKE_BUILD_TYPE - CMake build type (default: Release)"
	@echo "  TEST             - Specific test to run (optional)"
	@echo "  RUST_LOG         - Rust log level (optional)"

check-cpp-path:
	@if [ ! -d "$(CPP_SRC_PATH)/adnl" ]; then \
		echo "Error: $(CPP_SRC_PATH) does not appear to be a valid TON C++ source directory"; \
		echo "Expected to find $(CPP_SRC_PATH)/adnl"; \
		exit 1; \
	fi
	@echo "Using C++ source: $(CPP_SRC_PATH)"

build: build-cpp build-rust

build-cpp: check-cpp-path
	@echo "Building C++ test harness..."
	@echo "Note: TON C++ libraries must be pre-built in $(CPP_SRC_PATH)/build"
	@echo "If not built, run: cd $(CPP_SRC_PATH) && mkdir -p build && cd build && cmake .. && make overlay adnl dht"
	@mkdir -p $(CPP_BUILD_DIR)
	@cd $(CPP_BUILD_DIR) && cmake \
		-DCMAKE_BUILD_TYPE=$(CMAKE_BUILD_TYPE) \
		-DTON_SRC_PATH=$(CPP_SRC_PATH) \
		$(CPP_TEST_SRC)
	@cd $(CPP_BUILD_DIR) && cmake --build . -j$(CMAKE_JOBS)
	@mkdir -p $(CPP_TEST_SRC)/build
	@cp $(CPP_TEST_BIN) $(CPP_TEST_SRC)/build/compat_test_node
	@if [ "$$(uname)" = "Darwin" ]; then codesign --force --sign - $(CPP_TEST_SRC)/build/compat_test_node; fi
	@echo "C++ test harness built: $(CPP_TEST_BIN)"

build-rust:
	@echo "Building Rust test code..."
	@cd $(WORKSPACE_ROOT) && cargo build --package compat_test
	@echo "Rust test code built"

test: build test-rust

test-rust:
	@if [ ! -f "$(CPP_TEST_BIN)" ]; then \
		echo "Error: C++ test binary not found at $(CPP_TEST_BIN)"; \
		echo "Run 'make build-cpp' first"; \
		exit 1; \
	fi
	@echo "Running compatibility tests..."
	@cd $(WORKSPACE_ROOT) && \
		CPP_COMPAT_TEST_BIN=$(CPP_TEST_BIN) \
		RUST_TEST_THREADS=$(RUST_TEST_THREADS) \
		RUST_BACKTRACE=1 \
		cargo test --package compat_test \
		$(if $(TEST),--test $(TEST),)

clean:
	@echo "Cleaning build artifacts..."
	@rm -rf $(BUILD_DIR)
	@echo "Clean complete"

# Development helpers
.PHONY: fmt clippy

fmt:
	@cd $(WORKSPACE_ROOT) && cargo fmt --package compat_test

clippy:
	@cd $(WORKSPACE_ROOT) && cargo clippy --package compat_test
