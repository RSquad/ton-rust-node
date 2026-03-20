#!/bin/bash

# Script to export public keys from HashiCorp Vault transit/keys/<prefix>*
# and insert them into node config.json files at .control_server.clients.list
#
# Usage: ./update_control_clients.sh [secret-prefix] [nodes-dir]
#   secret-prefix: The prefix for secret names (default: control-clients-node)
#                  Examples: control-clients-node, ext-wallets-node
#   nodes-dir:     Path to directory containing node_X subdirectories (default: ../node/tests/test_run_net_py/tmp)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Default secret prefix, can be overridden via command line argument
SECRET_PREFIX="${1:-control-clients-node}"

# Default nodes directory (relative to script location in node-control)
NODES_DIR="${2:-${SCRIPT_DIR}/../node/tests/test_run_net_py/tmp}"

# Resolve to absolute path
NODES_DIR="$(cd "$NODES_DIR" 2>/dev/null && pwd)" || {
    echo "Error: Nodes directory not found: $2"
    exit 1
}

# Check if vault is available
if ! command -v vault &> /dev/null; then
    echo "Error: vault command not found"
    exit 1
fi

# Check if jq is available
if ! command -v jq &> /dev/null; then
    echo "Error: jq command not found"
    exit 1
fi

echo "Using secret prefix: $SECRET_PREFIX"
echo "Using nodes directory: $NODES_DIR"
echo ""

# Get list of keys matching prefix from vault
echo "Fetching keys from vault..."
KEYS=$(vault list -format=json /transit/keys 2>/dev/null | jq -r --arg prefix "$SECRET_PREFIX" '.[] | select(startswith($prefix))')

if [ -z "$KEYS" ]; then
    echo "No keys found matching prefix: ${SECRET_PREFIX}*"
    exit 1
fi

echo "Found keys:"
echo "$KEYS"
echo ""

# Process each key
for KEY in $KEYS; do
    # Extract node number from key name (e.g., control-clients-node1 -> 1)
    NODE_NUM=$(echo "$KEY" | sed "s/${SECRET_PREFIX}//")
    
    CONFIG_FILE="${NODES_DIR}/node_${NODE_NUM}/config.json"
    
    if [ ! -f "$CONFIG_FILE" ]; then
        echo "Warning: Config file not found: $CONFIG_FILE, skipping..."
        continue
    fi
    
    echo "Processing $KEY -> node_${NODE_NUM}..."
    
    # Export public key from vault
    PUBLIC_KEY=$(vault read --format=json "/transit/keys/${KEY}" | jq -r '.data.keys["1"].public_key')
    
    if [ -z "$PUBLIC_KEY" ] || [ "$PUBLIC_KEY" == "null" ]; then
        echo "  Error: Failed to get public key for $KEY"
        continue
    fi
    
    echo "  Public key: $PUBLIC_KEY"
    
    # Update config.json with the new public key
    # Create new client entry and set it in the clients list
    TEMP_FILE=$(mktemp)
    jq --arg pubkey "$PUBLIC_KEY" '.control_server.clients.list = [{"type_id": 1209251014, "pub_key": $pubkey}]' "$CONFIG_FILE" > "$TEMP_FILE"
    mv "$TEMP_FILE" "$CONFIG_FILE"
    
    echo "  Updated $CONFIG_FILE"
done

echo ""
echo "Done!"
