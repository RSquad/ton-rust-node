# Local TON Network Setup with HashiCorp Vault

This guide describes how to set up a local TON network, configure HashiCorp Vault for secrets management, build nodectl configuration, and restart nodes without resetting the network state.

## Table of Contents

- [Prerequisites](#prerequisites)
- [Step 1: Start Local TON Network](#step-1-start-local-ton-network)
- [Step 2: Install and Configure HashiCorp Vault](#step-2-install-and-configure-hashicorp-vault)
- [Step 3: Generate Control Client Keys in Vault](#step-3-generate-control-client-keys-in-vault)
- [Step 4: Update Node Configurations with Client Keys](#step-4-update-node-configurations-with-client-keys)
- [Step 5: Build nodectl Configuration](#step-5-build-nodectl-configuration)
- [Step 6: Stop and Restart Nodes](#step-6-stop-and-restart-nodes)

---

## Prerequisites

| Requirement | Description |
|-------------|-------------|
| **Python 3** | Python 3.x with `yaml` and `json` modules |
| **Rust toolchain** | Rust 1.70+ with Cargo |
| **HashiCorp Vault** | Vault CLI installed |
| **jq** | JSON processor for shell scripts |

---

## Step 1: Start Local TON Network

### 1.1 Navigate to Test Directory

Open terminal with the repository root.

```bash
cd node/tests/test_run_net_py
```

### 1.2 Run the Network Setup Script

Then run:

```bash
python3 test_run_net.py --elections
```

After the first run it will generate `test_run_net.json` file. Modify it like this:

```json
  "rust_nodes_count": 5,
  "cpp_nodes_count": 0,
  "cpp_src_path":<set to ton-blockchain/ton>,
```

Run the script again:

```bash
python3 test_run_net.py --elections
```

This will build binaries, generate configs, create validator keys, and start 5 validator nodes.

The script will:

1. Build Rust binaries (node, console, crypto, zerostate)
2. Generate configuration files for each node (node_1 through node_5)
3. Create validator keys for each node
4. Build the zerostate and global config
5. Generate `nodectl-local.json` configuration
6. Start all 5 validator nodes

### 1.3 Generated Files

After running the script, you will have:

| Path | Description |
|------|-------------|
| `tmp/node_1/` through `tmp/node_5/` | Node working directories |
| `tmp/node_X/config.json` | Node configuration |
| `tmp/node_X/console.json` | Console client configuration |
| `tmp/global_config.json` | Global network configuration |
| `tmp/zerostate.json` | Zerostate configuration |
| `tmp/nodectl-local.json` | Basic nodectl configuration - with plain secret keys |

---

## Step 2: Install and Configure HashiCorp Vault

Note: you can skip this step if you already initialized vault and move to Step 3. But before `unseal and login` to the vault.

### 2.1 Install HashiCorp Vault

**macOS (Homebrew):**

```bash
brew tap hashicorp/tap
brew install hashicorp/tap/vault
```

**Ubuntu/Debian:**

```bash
wget -O- https://apt.releases.hashicorp.com/gpg | sudo gpg --dearmor -o /usr/share/keyrings/hashicorp-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/hashicorp-archive-keyring.gpg] https://apt.releases.hashicorp.com $(lsb_release -cs) main" | sudo tee /etc/apt/sources.list.d/hashicorp.list
sudo apt update && sudo apt install vault
```

### 2.2 Create Vault Data Directory

```bash
sudo mkdir -p /tmp/vault/data
```

### 2.3 Create Vault Configuration

Create `/tmp/vault/vault.hcl`:

```hcl
api_addr                = "http://127.0.0.1:8200"
cluster_addr            = "http://127.0.0.1:8201"
cluster_name            = "vault-cluster"
disable_mlock           = true
ui                      = false

listener "tcp" {
    address     = "127.0.0.1:8200"
    tls_disable = true
}

storage "raft" {
    path    = "/tmp/vault/data"
    node_id = "vault-server"
}
```

### 2.4 Start Vault Server

In a separate terminal:

```bash
vault server -config=/tmp/vault/vault.hcl
```

### 2.5 Initialize and Unseal Vault

Open a new terminal:

```bash
export VAULT_ADDR='http://127.0.0.1:8200'

# Initialize vault (save the output!)
vault operator init -key-shares=1 -key-threshold=1
```

**IMPORTANT:** Save the Unseal Key and Root Token from the output.

```bash
# Unseal the vault
vault operator unseal
# Enter the Unseal Key when prompted

# Login with root token
vault login
# Enter the Root Token when prompted
```

### 2.6 Enable Transit Secrets Engine

```bash
vault secrets enable transit
```

### 2.7 Set Environment Variables

Add to your shell profile or export in current session:

```bash
export VAULT_ADDR='http://127.0.0.1:8200'
export VAULT_URL="hashicorp://http://127.0.0.1:8200?api_key=<YOUR_ROOT_TOKEN>"
```

URL example: `"hashicorp://http://127.0.0.1:8200?api_key=<YOUR_ROOT_TOKEN>"`

---

## Step 3: Generate Control Client Keys in Vault

### 3.1 Build SecretsVault cli

```bash
cd secrets-vault
cargo build --release --bin secrets-vault-cli --features=secrets-vault-cli,hashicorp-storage
# the binary can be found in ./target/release/secrets-vault-cli
```

### 3.2 Generate Ed25519 keys for each node's control client authentication

```bash
# Generate control client keys for all 5 nodes
for i in 1 2 3 4 5; do
    secrets-vault-cli --url="$VAULT_URL" \
    generate \
    --secret-id="control-clients-node$i" \
    --algorithm=ed25519 \
    --extractable
done
```

### 3.3 Verify Keys Were Created

```bash
vault list transit/keys
```

Expected output:

```
Keys
----
control-clients-node1
control-clients-node2
control-clients-node3
control-clients-node4
control-clients-node5
```

### 3.4 Generate Wallet Keys

To store non-extractable wallet keys in Vault:

```bash
for i in 1 2 3 4 5; do
    secrets-vault-cli --url="$VAULT_URL" \
    generate \
    --secret-id="wallets-node$i" \
    --algorithm=ed25519 
done
```

---

## Step 4: Update Node Configurations with Client Keys

### 4.1 Stop Running Nodes

Go to node/tests/test_run_net_py

```bash
python3 test_run_net.py --stop
```

### 4.2 Update Node Configs with Vault Client Public Keys

Navigate to node-control directory and run the update script:

```bash
cd ../../../node-control

# Run the update script
./update_control_clients.sh
```

This script will:

1. Fetch public keys from Vault for each `control-clients-nodeX` key
2. Update each node's `config.json` with the client public key in `control_server.clients.list`

### 4.3 Verify Configuration Updates

Check that the client keys were added to node configs:

```bash
cat ../node/tests/test_run_net_py/tmp/node_1/config.json | jq '.control_server.clients'
```

Expected output:

```json
{
  "list": [
    {
      "type_id": 1209251014,
      "pub_key": "<BASE64_PUBLIC_KEY>"
    }
  ]
}
```

---

## Step 5: Build nodectl Configuration

### 5.1 Generate Nodectl Configuration

```bash
./build-nodectl-config.sh
```
Will generate `nodectl-local-generated.json` config file.

## Step 6: Stop and Restart Nodes

### 6.1 Stop All Nodes

```bash
cd ../node/tests/test_run_net_py
python3 test_run_net.py --stop
```

### 6.2 Restart Nodes Without Reset

Start each node individually without rebuilding or regenerating configs:

```bash
# Start node 1
python3 test_run_net.py --start 1 --nobuild

# Start node 2
python3 test_run_net.py --start 2 --nobuild

# Start node 3
python3 test_run_net.py --start 3 --nobuild

# Start node 4
python3 test_run_net.py --start 4 --nobuild

# Start node 5
python3 test_run_net.py --start 5 --nobuild
```

## Step 8: Deploy wallets and pools

```bash 
cd node/tests/test_load_net
cp .env.example .env
```

Update the following environment variables in `.env` file:

```bash
MASTER_WALLET_KEY=<private-key-for-master-singlehost-wallet>
API_ENDPOINTS="http://127.0.0.1:3301/"
NODECTL_CONFIG_PATH=../../../node-control/nodectl-local-generated.json
NODECTL_INITIAL_BALANCE=100100
VAULT_TOKEN=<vault-token>
VAULT_ADDR=<your-vault-url:port>
VAULT_NAMESPACE=<vault-namespace>
```

Deploy contracts:

```bash 
bun i # only once
bun run deploy:wallets && bun run deploy:pools && bun run topup:pools
```

## Step 9: Run nodectl service

```bash
cd node-control
cargo run --package nodectl -- service --config="nodectl-local-generated.json" 
```

---

## Quick Reference

### Start Network from Scratch

```bash
cd node/tests/test_run_net_py
python3 test_run_net.py
```

### Stop All Nodes

```bash
python3 test_run_net.py --stop
```

### Restart Without Reset

```bash
python3 test_run_net.py --start 1 --nobuild
```

### Vault Commands

```bash
# List all transit keys
vault list transit/keys

# Read key details
vault read --format=json transit/keys/control-clients-node1

# Get public key
vault read --format=json transit/keys/control-clients-node1 | jq -r '.data.keys["1"].public_key'
```

---

### Console Connection Test

Test connection to node control server:

```bash
cd tmp/bins
./console -C ../node_1/console.json -c getstats
```
