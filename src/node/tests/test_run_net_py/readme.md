# Prerequisites

- Python 3 (developed and tested with version 3.10.10)
- `pyyaml` package

```bash
pip install pyyaml
```

## For Development

If you plan to modify the `test_run_net.py` script, it is recommended to install the following tools for code quality checks:

- black code formatter

```bash
pip install black
```

- ruff linter

```bash
pip install ruff
```

- pyright linter

```bash
pip install pyright
```

# Usage

The `test_run_net.py` script is used to run a local TON network with multiple nodes for testing and development purposes.
The nodes are built from source code (both C++ and Rust versions are supported).
To run the script, use the following command:

```bash
python3 test_run_net.py
```

On the first run, the script generates a default configuration file `test_run_net.json` in the same directory.
You can edit this file to change network parameters, such as the number of nodes, ports, paths, etc.
After editing the configuration file, run the script again to start the network.

Without any arguments, the script will stop any running nodes, build the node binaries, generate configurations, and start new network from zerostate.

## Command-Line Arguments

The script supports the following command-line arguments:

- `--restart`: Stops any running nodes, builds the binaries, and starts the nodes. The network continues from the previous state.
- `--stop`: Only stops any running nodes and exits.
- `--start [node_numbers]`: Starts specified nodes (by their numbers) without stopping or rebuilding the entire network. If no node numbers are provided, all nodes are started. useful for starting specific nodes after a crash.
- `--elections`: Use `zerostate_blank_elections.json` instead of `zerostate_blank.json` (enables elections in the initial zerostate).

## Zerostate

The script generates a zerostate file for the network based on the blank file `zerostate_blank.json`. You can edit this file before running the script to customize the initial state of the blockchain.

## Rust Log Configuration

You can customize the logging configuration for Rust nodes by editing the `log_cfg_blank.yml` file. This file is copied to each node's working directory and used to configure logging behavior. Please do not edit the paths in the file, as they are set by the script.

## C++ Log Configuration

The C++ node does not have as flexible logging configuration as the Rust node. You can set the logging level by changing the `cpp_log_level` parameter in the `test_run_net.json` configuration file.

Supported log levels:

- FATAL = 0
- ERROR = 1
- WARNING = 2
- INFO = 3
- DEBUG = 4

Additional debug information can be enabled by setting the log level to 5.

## C++ Build Command

You should set the proper build command for the C++ node by changing the `cpp_build_command` parameter in the `test_run_net.json` configuration file.
By default, it is set to build scripts from the C++ node's readme file, depending on the operating system.
Please prepare the C++ node's source code before running the script according to the node's readme instructions.

# Code Checks

After making changes to the `test_run_net.py` file, it is recommended to run linters and the code formatter to ensure code quality and consistency.

- Run linters:

```bash
ruff check test_run_net.py
pyright test_run_net.py
```

- Run code formatter:

```bash
black test_run_net.py
```

# nodectl single-host E2E (`run_singlehost_nodectl.py`)

Bootstrap script (same directory) builds **nodectl**, starts the **single-host** network from `test_run_net.py`, runs the **nodectl service**, configures wallets/nodes/elections via CLI, then validates REST (election stakes vs elector contract).

Requires **`MASTER_WALLET_KEY`** (see `node/tests/test_load_net/.env`). CI wrapper: **`test_nodectl_ci.sh`**.

After stake validation (phase 13), the script smoke-tests **voting over REST**: `GET /v1/voting/config`, `GET /v1/voting/proposals`, and **`nodectl vote ls`**. Set **`VOTING_REST_VALIDATE=0`** to skip those checks.

Optional **`CREATE_VOTING_PROPOSAL=1`** (before smoke checks in phase 13): copies `test_load_net/scripts/singlehost-config-proposal-p15.json` to `config-params.json`, runs **`bun blueprint run create-proposal --custom <HTTP_API_URL>/jsonRPC --mnemonic`** against **`HTTP_API_URL`**, waits until **`nodectl vote ls`** shows an on-chain proposal, then **`nodectl vote add`** and checks **`GET /v1/voting/config`** lists the hash. Requires default singlehost **elections** zerostate (`elections_start_before` **420** in genesis — template uses **421**). Tune expiry with **`VOTING_PROPOSAL_EXPIRES_SECS`** (default **86400**). Blueprint’s mnemonic provider requires **`WALLET_MNEMONIC`** / **`WALLET_VERSION`**; the script sets a **dummy** test mnemonic because **`create-proposal.ts` signs with `MASTER_WALLET_KEY` only**. Override with **`BLUEPRINT_WALLET_MNEMONIC`** / **`BLUEPRINT_WALLET_VERSION`** if needed.

For **ad‑hoc** experiments without singlehost, use **`test_load_net`** (`npm`/`bun` scripts such as **`proposals:create`**) with the correct **`--custom …/jsonRPC`** URL.
