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

## Contributing

Contribution to the project is expected to be done via pull requests submission.

## License

Licensed under the [GNU General Public License v3.0](../LICENSE).
