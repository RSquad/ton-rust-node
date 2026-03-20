# ton-node

TON node and validator implementation in Rust.

## Crates

| Crate | Description |
|---|---|
| `node` | Node binary, collator, validator, storage, networking |
| `adnl` | ADNL/RLDP/Overlay/DHT protocol stack |
| `block` | TON blockchain data types (cells, messages, blocks) |
| `block-json` | JSON serialization for blockchain types |
| `vm` | TVM (TON Virtual Machine) |
| `executor` | Transaction executor |
| `assembler` | TVM assembler/disassembler |
| `tl` | TL schema types and codegen |
| `emulator` | TVM emulator C-compatible library |
| `node-control` | nodectl — node management CLI |
| `secrets-vault` | Cryptographic key/secrets management |

## Building

```bash
cargo build --release
```

### Installing Git Hooks

This project uses automated Git hooks that run quality checks before pushes. To install the hooks:

```bash
make install-hooks
```

This command will:
- Install git hooks if not already present
- Set up pre-push hooks (format check, clippy test, release compile check)
- Configure all necessary Git hook files

The hooks will automatically run on `git push` to ensure code quality.

### Running tests

```
cargo test --release --package catchain -- --nocapture --test-threads=1 
cargo test --release --package storage -- --nocapture --test-threads=1 
cargo test --release --package validator_session -- --nocapture --test-threads=1 
cargo test --release -- --nocapture --test-threads=1
```

## TON Node Usage

To get help about command line arguments, run
```
ton-node --help
```

## TON Node Console Usage

This tool serves the purpose of generating election requests for the Rust Node.

### How to use

```bash
console -C console.json -c "commamd with parameters" -c "another command" -t timeout
```

Where

`console.json` - path to configuration file

`commamd with parameters`/ `another command` – any of the supported console commands with necessary parameters

`timeout` – command timeout in seconds

Configuration file should be created manually and have the following format:

```json
{
		"config": {
				"server_address": "127.0.0.1:4924",
				"server_key": {
						"type_id": 1209251014,
						"pub_key": "cujCRU4rQbSw48yHVHxQtRPhUlbo+BuZggFTQSu04Y8="
				},
				"client_key": {
						"type_id": 1209251014,
						"pvt_key": "oEivbTDjSOSCgooUM0DAS2z2hIdnLw/PT82A/OFLDmA="
				}
		},
		"wallet_id": "-1:af17db43f40b6aa24e7203a9f8c8652310c88c125062d1129fe883eaa1bd6763",
		"max_factor": 2.7
}
```

Where

`server_address` – address and port of the node.

`server_key` - structure containing the server's Ed25519 public key. Use `crypto gen key` to generate a
keypair, then fill `pub_key` with the `pubkey` field from its output and keep `type_id` as `1209251014`.

`client_key` - structure containing the client's Ed25519 private key. Use `crypto gen key` to generate a
keypair, then fill `pvt_key` with the `secret` field from its output and keep `type_id` as `1209251014`.

`type_id` - key type identifier for Ed25519 (`1209251014`). Should not be changed.

`wallet_id` - validator wallet address.

`max_factor` - [max_factor](https://docs.ton.dev/86757ecb2/p/456977-validator-elections) stake parameter (maximum ratio allowed between your stake and the minimal
 validator stake in the elected validator group), should be ≥ 1
 
### Commands

#### addadnl

**`addadnl`** – sets key as ADNL address.

params:

• `perm_key_hash` - ed25519 hash of permanent public key in hex or base64 format.

• `key_hash` - ed25519 hash of public key in hex or base64 format.

• `expire-at` - time the ADNL address expires and is deleted from node, in unixtime.

Example:

```bash
console -c "addadnl 4374376452376543 6783978551824553 1608288600"
```

#### addpermkey

**`addpermkey`** - adds validator permanent key

params:

• `key_hash` - ed25519 hash of public key in hex or base64 format.

• `election-date` - election start in unixtime.

• `expire-at`- time the key expires and is deleted from node, in unixtime.

Example:

```bash
console -c "addpermkey 4374376452376543 1608205174 1608288600"
```

#### addtempkey

**`addtempkey`** - adds validator temporary key.

params:

• `perm_key_hash` - ed25519 hash of permanent public key in hex or base64 format.

• `key_hash` - ed25519 hash of public key in hex or base64 format.

• `expire-at` - time the key expires and is deleted from node, in unixtime.

Example:

```bash
console -c "addtempkey 4374376452376543 6783978551824553 1608288600"
```

#### addvalidatoraddr

**`addvalidatoraddr`** - adds validator ADNL address.

params:

• `perm_key_hash` - ed25519 hash of permanent public key in hex or base64 format.

• `key_hash` - ed25519 hash of public key in hex or base64 format.

• `expire-at`- time the ADNL address expires and is deleted from node, in unixtime.

Example:

```bash
console -c "addvalidatoraddr 4374376452376543 6783978551824553 1608288600"
```

#### election-bid

**`election-bid`** - obtains required information from the blockchain, generates all the necessary keys for validator, prepares the message in predefined format, asks to sign it and sends to the blockchain.

params:

• `election-start` - unixtime of election start.

• `election-end` - unixtime of election end.

• `filename` - filename with path to save body of message ("validator-query.boc" by default)

Example:

```bash
console -c "election-bid 1608205174 1608288600"
```

Command calls all other necessary subcommands automatically. Election request is written to file.

#### exportpub

**`exportpub`** - exports public key by key hash.

params:

• `key_hash` - ed25519 hash of public key in hex or base64 format.

Returns public_key - ed25519 public key in hex and base64 format.

Example:

```bash
console -c "exportpub 4374376452376543"
```

#### getaccount

**`getaccount`** - load and save (optional) account information in json-format.

params:

• `account_address` - is the account address.

• `file_name` - is the file's name to save account information in json-format. This param is optional.

Returns json with account information. 

Base json-fields:

• `acc_type` - account type description;

• `balance` - account balance;

• `last_trans_lt` - logical time of the last account's transaction;

• `data(boc)` - account`s boc.

Example:

```bash
console -c "getaccount 0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F"
```

#### getaccountstate

**`getaccountstate`** - save account to the file (in bag of cells format).

params:

• `account_address` - is the account address.

• `file_name` - is the file's name to save account's boc.

Returns account's boc.

Example:

```bash
console -c "getaccountstate 0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F account.boc"
```

#### getblockchainconfig

**`getblockchainconfig`** - get current config from masterchain state.

Returns boc with current config.

Example:

```bash
console -c "getblockchainconfig"
```

#### getconfig

**`getconfig`** - get current config param from masterchain state.

params:

• `param_number` - config parameter number.

Returns boc with current config param.

Example:

```bash
console -c "getconfig 15"
```

#### getstats

**`getstats`** - get node status, validation status (if node is validator) and other information. 

Command has no parameters.

Returns node's information in JSON-format.

base json-fields:

• `sync_status` - synchronization status description;

• `masterchainblocktime` - field with time of last masterchain block, downloaded by node;

• `masterchainblocknumber` - field with number of last masterchain block, downloaded by node;

• `timediff` - field with time difference between now and last loaded masterchain block's time;

• `in_current_vset_p34` - true, if config param p34 contains this node's key;

• `in_current_vset_p36` - true, if config param p36 contains this node's key;

• `last applied masterchain block id` - field with information about last applied masterchain block's id;

• `processed workchain` - field with information about the processed workchain;

• `validation_stats` - field with information about validated workchains;

• `tps_10` - transactions per second average over 10 seconds;

• `tps_300` - transactions per second average over 300 seconds;

Example: 

```bash
console -c "getstats"
```

#### newkey

**`newkey`** - generates new key pair on server.

Command has no parameters.

Returns ed25519 hash of public key in hex and base64 format.

Example:

```bash
console -c "newkey"
```

#### recover_stake

**`recover_stake`** – recovers all or part of the validator stake from elector.

params:

• `filename` - filename with path to save body of message ("recover-query.boc" by default)

Example:

```bash
console -c "recover_stake"
```

#### sendmessage

**`sendmessage`** - loads a serialized message from file and sends it to nodes as an external message.

params:

• `file_name` - serialized message file (in bag of cells format).

Example:

```bash
console -c "sendmessage message.boc"
```

#### sign

**`sign`** - signs bytestring with private key.

params:

• `key_hash` - ed25519 hash of public key in hex or base64 format.

• `data` - data in hex or base64 format.

Example:

```bash
console -c "sign 4374376452376543 af17db43f40b6aa24e7203a9f8c8652310c88c125062d1129f"
```

## Zerostate tool

This tool generates config and zerostate for network launch from json zerostate file.

### How to use

```bash
zerostate -i zerostate.json
```

Where

`zerostate.json` – is the zerostate file.

## Crypto tool

Unified CLI for cryptographic operations: key generation, ADNL ID computation, and DHT node entry signing.

### How to use

```bash
# Generate a new Ed25519 keypair
crypto gen key

# Compute ADNL ID from a public or secret key
crypto get adnl-id --public <base64-key>
crypto get adnl-id --secret <base64-key>

# Generate a signed DHT node entry
crypto gen dht --addr <ip:port> --key <base64-secret-key>
```

Example:

```bash
crypto gen dht --addr 51.210.114.123:30303 --key ABwHd2EavvLJk789BjSF3OJBfX6c26Uzx1tMbnLnRTM=
```

## Dhtscan tool

This tool scans DHT for node records.

### How to use

```bash
dhtscan [--jsonl] [--overlay] [--workchain0] path-to-global-config
```

Where

`--jsonl` – optional flag that sets the output as single line json. Default output is multiline json.

`--overlay` – optional flag to search for overlay nodes.

`--workchain0` – optional flag to search both in masterchain and basechain. By default only masterchain is searched.

`path-to-global-config` – path to network global config file.

## Print tool

This tool prints a state or block from the database.

### How to use

```bash
print [--path path/to/node_db] [--state block_id] [--block block_id] [--shards block_id] [--boc path/to/boc] [--brief] [--accounts]
```

Where

`--path` – path to node database.

`--boc` - path to boc file with message, state or account. If account is config - config params will be printed

`block_id` – id of the block, state or shards to be printed.

`--brief` - print block without messages and transactions, state without accounts

`--accounts` - short info of all accounts will be printed as json

## Contributing

Contribution to the project is expected to be done via pull requests submission.

## License

See the [LICENSE](LICENSE) file for details.
