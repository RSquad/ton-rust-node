# TON Node Console

Control server client for interacting with the TON node via ADNL protocol.

## Usage

```bash
console -C console.json -c "command with parameters" -c "another command" -t timeout
```

Where

- `console.json` — path to configuration file
- `command with parameters` / `another command` — any of the supported console commands with necessary parameters
- `timeout` — command timeout in seconds

## Configuration

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

- `server_address` — address and port of the node.
- `server_key` — structure containing the server's Ed25519 public key. Use `crypto gen key` to generate a keypair, then fill `pub_key` with the `pubkey` field from its output. Keep `type_id` as `1209251014`.
- `client_key` — structure containing the client's Ed25519 private key. Use `crypto gen key` to generate a keypair, then fill `pvt_key` with the `secret` field from its output. Keep `type_id` as `1209251014`.
- `type_id` — key type identifier for Ed25519 (`1209251014`). Should not be changed.
- `wallet_id` — validator wallet address.
- `max_factor` — maximum ratio allowed between your stake and the minimal validator stake in the elected validator group, should be >= 1.

## Commands

### Key management

#### newkey

Generates new key pair on server.

```bash
console -c "newkey"
```

Returns ed25519 hash of public key in hex and base64 format.

#### exportpub

Exports public key by key hash.

- `key_hash` — ed25519 hash of public key in hex or base64 format.

```bash
console -c "exportpub 4374376452376543"
```

#### importprivatekey

Imports private key on server.

- `keytype` — key type.
- `key` — private key.

```bash
console -c "importprivatekey <keytype> <key>"
```

#### sign

Signs bytestring with private key.

- `key_hash` — ed25519 hash of public key in hex or base64 format.
- `data` — data in hex or base64 format.

```bash
console -c "sign 4374376452376543 af17db43f40b6aa24e7203a9f8c8652310c88c125062d1129f"
```

### Validator keys

#### addpermkey

Adds validator permanent key.

- `key_hash` — ed25519 hash of public key in hex or base64 format.
- `election-date` — election start in unixtime.
- `expire-at` — time the key expires and is deleted from node, in unixtime.

```bash
console -c "addpermkey 4374376452376543 1608205174 1608288600"
```

#### addtempkey

Adds validator temporary key.

- `perm_key_hash` — ed25519 hash of permanent public key in hex or base64 format.
- `key_hash` — ed25519 hash of public key in hex or base64 format.
- `expire-at` — time the key expires and is deleted from node, in unixtime.

```bash
console -c "addtempkey 4374376452376543 6783978551824553 1608288600"
```

#### addadnl

Sets key as ADNL address.

- `key_hash` — ed25519 hash of public key in hex or base64 format.
- `category` — address category.

```bash
console -c "addadnl 4374376452376543 0"
```

#### addvalidatoraddr

Adds validator ADNL address.

- `perm_key_hash` — ed25519 hash of permanent public key in hex or base64 format.
- `key_hash` — ed25519 hash of public key in hex or base64 format.
- `expire-at` — time the ADNL address expires and is deleted from node, in unixtime.

```bash
console -c "addvalidatoraddr 4374376452376543 6783978551824553 1608288600"
```

### Elections

#### election-bid

Obtains required information from the blockchain, generates all the necessary keys for validator, prepares the message in predefined format, asks to sign it and sends to the blockchain.

- `election-start` — unixtime of election start.
- `election-end` — unixtime of election end.
- `filename` — filename with path to save body of message (`validator-query.boc` by default).

```bash
console -c "election-bid 1608205174 1608288600"
```

Aliases: `ebid`, `election_bid`.

#### recover_stake

Recovers all or part of the validator stake from elector.

- `filename` — filename with path to save body of message (`recover-query.boc` by default).

```bash
console -c "recover_stake"
```

### Node status

#### getstats

Get node status, validation status (if node is validator) and other information.

```bash
console -c "getstats"
```

Returns JSON with fields: `sync_status`, `masterchainblocktime`, `masterchainblocknumber`, `timediff`, `in_current_vset_p34`, `in_current_vset_p36`, `last applied masterchain block id`, `processed workchain`, `validation_stats`, `tps_10`, `tps_300`.

#### getstatsnew

Get node status in new format.

```bash
console -c "getstatsnew"
```

#### getconsensusstats

Get consensus statistics for the node.

```bash
console -c "getconsensusstats"
```

#### get_validator_config

Get config from node or validator.

```bash
console -c "get_validator_config"
```

### Blockchain queries

#### getaccount

Load and save (optional) account information in JSON format.

- `account_address` — account address.
- `file_name` — (optional) file name to save account info.
- `block_id` — (optional) block id for historical query.

```bash
console -c "getaccount 0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F"
```

#### getaccountstate

Save account to file (in bag of cells format).

- `account_address` — account address.
- `file_name` — file name to save account's boc.

```bash
console -c "getaccountstate 0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F account.boc"
```

#### getaccountstate_byblock

Save masterchain account state from a specific block to file.

- `block_root_hash_or_seqno` — block root hash or masterchain block sequence number.
- `account_address` — account address.
- `file_name` — (optional) file name to save account's boc.

```bash
console -c "getaccountstate_byblock 12345 0:000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F account.boc"
```

#### getblock

Find block by masterchain seq_no, block id or root hash, then save to file.

- `block_id` — block id, root hash of block, or masterchain sequence number.
- `file_name` — file name to save the block.

```bash
console -c "getblock 12345 block.boc"
```

#### getblockchainconfig

Get current config from masterchain state.

```bash
console -c "getblockchainconfig"
```

#### get_config_param

Get current config param from masterchain state.

- `param_number` — config parameter number.

```bash
console -c "get_config_param 15"
```

#### sendmessage

Load a serialized message from file and send it to nodes as an external message.

- `file_name` — serialized message file (in bag of cells format).

```bash
console -c "sendmessage message.boc"
```

### Debugging

#### bundle

Prepare bundle for a given block.

- `block_id` — block identifier.

```bash
console -c "bundle <block_id>"
```

#### future_bundle

Prepare future bundle for a given block.

- `block_id` — block identifier.

```bash
console -c "future_bundle <block_id>"
```

### Custom overlays

#### add_custom_overlay

Add custom overlay from config file.

- `config_file` — path to overlay config file.

```bash
console -c "add_custom_overlay overlay.json"
```

#### del_custom_overlay

Delete custom overlay.

- `overlay_name` — name of the custom overlay.

```bash
console -c "del_custom_overlay my_overlay"
```

#### show_custom_overlays

Show all custom overlays.

```bash
console -c "show_custom_overlays"
```
