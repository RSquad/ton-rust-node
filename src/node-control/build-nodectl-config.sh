#! /bin/bash

set -e

CONFIG="nodectl-local-generated.json"
NODE_INDICES="1 2 3 4 5"

echo "=== Generating config ==="
cargo run -p nodectl -- config generate --output="$CONFIG" --force

if [ ! -n "$VAULT_URL" ]; then
    echo "ERROR: VAULT_URL is not set"
    echo "Please set VAULT_URL environment variable"
    echo "export VAULT_URL=\"hashicorp://\$VAULT_ADDR?api_key=\$NODECTL_TOKEN&namespace=\$VAULT_NAMESPACE\""
    exit 1
fi

echo "=== Adding vault configuration ==="
TEMP_FILE=$(mktemp)
jq --arg url "$VAULT_URL" '. + {"vault": {"url": $url}}' "$CONFIG" > "$TEMP_FILE"
mv "$TEMP_FILE" "$CONFIG"
echo 

echo "=== Set TON HTTP API Endpoint to Rust Node RPC Server ==="
TEMP_FILE=$(mktemp)
jq '. + {"ton_http_api": {"url": "http://127.0.0.1:3301/"}}' "$CONFIG" > "$TEMP_FILE"
mv "$TEMP_FILE" "$CONFIG"

echo "=== Configuring nodes ==="
for i in $NODE_INDICES; do
    NAME="node$i"
    echo "=== Configuring $NAME ==="
    SERVER_KEY=$(cat ../node/tests/test_run_net_py/tmp/node_$i/console.json | jq '.config.server_key.pub_key')
    WALLET_PUBLIC_KEY=$(vault read --format=json "/transit/keys/wallets-$NAME" | jq -r '.data.keys["1"].public_key')

    cargo run -p nodectl -- config node --config="$CONFIG" \
    --name="$NAME" \
    --server-address="127.0.0.1:310$i" \
    --client-key-id="control-clients-$NAME" \
    --server-key="$(echo $SERVER_KEY | tr -d '"')" \
    --wallet-key-id="wallets-$NAME" \
    --wallet-public-key="$WALLET_PUBLIC_KEY" \
    --nominator="Ef8egvmDjR3lzRrGKKO3ExiJG5ZMOhtSAzHvtAoSvkTi4ZPA"
done

