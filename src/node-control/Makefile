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
