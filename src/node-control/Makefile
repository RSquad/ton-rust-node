CONFIG ?= nodectl-local.json
LEVEL ?= info

packages = common nodectl elections ton-http-api-client service control-client commands contracts

test: $(addprefix test-, $(packages))

test-%:
	cargo test $(what) $(config) --package $* -- --nocapture

.PHONY: test $(CONFIG_IDS)

build: $(addprefix build-, $(packages))

build-%:
	cargo build $(config) --package $*

daemon:
	cargo run -p nodectl -- --verbose=$(LEVEL) service --config="$(CONFIG)"

CONFIG_IDS := 15 16 17 32 34 36

$(CONFIG_IDS):
	cargo run -p nodectl -- config-param --config="$(CONFIG)" $@

# ── Singlehost integration test ───────────────────────────────────────────────
# Boots a local 6-node TON network and runs nodectl against it end-to-end.
#
# Variables (all optional):
#   NOBUILD=1                skip cargo build (use pre-built binary)
#   NODE_CNT=6               number of validator nodes
#   PARTICIPANTS_WAIT=600    seconds to wait for election participants
#
# Examples:
#   make singlehost
#   make singlehost NOBUILD=1
#   make singlehost NODE_CNT=4 PARTICIPANTS_WAIT=900

SINGLEHOST_DIR  ?= ../node/tests/test_run_net_py
NOBUILD         ?= 0
NODE_CNT        ?= 6
PARTICIPANTS_WAIT ?= 600

# File target: rebuilt only when requirements.txt changes (or venv is missing).
# Calling `make singlehost-venv` installs deps; subsequent calls are instant.
$(SINGLEHOST_DIR)/.venv/pyvenv.cfg: $(SINGLEHOST_DIR)/requirements.txt
	python3 -m venv $(SINGLEHOST_DIR)/.venv
	$(SINGLEHOST_DIR)/.venv/bin/pip install -q -r $(SINGLEHOST_DIR)/requirements.txt

singlehost-venv: $(SINGLEHOST_DIR)/.venv/pyvenv.cfg

test-e2e: $(SINGLEHOST_DIR)/.venv/pyvenv.cfg
	cd $(SINGLEHOST_DIR) && \
	  NOBUILD=$(NOBUILD) \
	  NODE_CNT=$(NODE_CNT) \
	  PARTICIPANTS_WAIT_SECONDS=$(PARTICIPANTS_WAIT) \
	  .venv/bin/python3 run_singlehost_nodectl.py

stop-e2e:
	cd $(SINGLEHOST_DIR) && \
	  .venv/bin/python3 test_run_net.py --stop
	  pkill -9 nodectl

.PHONY: test-e2e stop-e2e singlehost-venv
