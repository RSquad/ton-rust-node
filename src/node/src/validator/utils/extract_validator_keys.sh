#!/bin/bash

# Display usage information
function show_usage {
    echo "Usage: $0 <cpp_node_path> <source_rust_config> <result_rust_config>"
    echo "Example: $0 /path/to/cpp/node /path/to/configs/config.json /path/to/configs/config.with_cpp_validator_keys.json"
    exit 1
}

# Check if we have the required arguments
if [ $# -ne 3 ]; then
    show_usage
fi

# Set paths from CLI arguments
CPP_NODE_PATH="$1"
CONFIG_RUST_JSON="$2"
OUTPUT_JSON="$3"

# Derive CPP config and keyring paths from the CPP node path
CONFIG_CPP_JSON="${CPP_NODE_PATH}/db/config.json"
KEYRING_DIR="${CPP_NODE_PATH}/db/keyring"

# Check if config files exist
if [ ! -f "$CONFIG_CPP_JSON" ]; then
    echo "Error: $CONFIG_CPP_JSON does not exist"
    exit 1
fi

if [ ! -f "$CONFIG_RUST_JSON" ]; then
    echo "Error: $CONFIG_RUST_JSON does not exist"
    exit 1
fi

# Check if keyring directory exists
if [ ! -d "$KEYRING_DIR" ]; then
    echo "Error: $KEYRING_DIR directory does not exist"
    exit 1
fi

# Function to convert base64 to hex uppercase
base64_to_hex() {
    echo "$1" | base64 -d | xxd -p -u | tr -d '\n'
}

# Create a temporary file for the validator keys JSON array
TEMP_JSON=$(mktemp)
echo "[" > "$TEMP_JSON"

# Use jq to parse the JSON and extract validator information
validators=$(jq -r '.validators[] | @base64' "$CONFIG_CPP_JSON")
first_validator=true

echo "validators:"

for validator_base64 in $validators; do
    # Decode the base64 encoded validator object
    validator=$(echo "$validator_base64" | base64 -d)
    
    # Extract validator ID
    validator_id=$(echo "$validator" | jq -r '.id')
    validator_id_hex=$(base64_to_hex "$validator_id")
    
    # Extract election date
    election_id=$(echo "$validator" | jq -r '.election_date')
    
    # Extract ADNL address ID
    adnl_id=$(echo "$validator" | jq -r '.adnl_addrs[0].id')
    adnl_id_hex=$(base64_to_hex "$adnl_id")
    
    # Extract private keys from keyring
    adnl_private_key=""
    validator_private_key=""
    
    if [ -f "$KEYRING_DIR/$adnl_id_hex" ]; then
        adnl_private_key=$(dd if="$KEYRING_DIR/$adnl_id_hex" bs=1 skip=4 count=32 status=none | base64)
    else
        echo "Warning: ADNL key file $KEYRING_DIR/$adnl_id_hex not found"
        adnl_private_key="KEY_FILE_NOT_FOUND"
    fi
    
    if [ -f "$KEYRING_DIR/$validator_id_hex" ]; then
        validator_private_key=$(dd if="$KEYRING_DIR/$validator_id_hex" bs=1 skip=4 count=32 status=none | base64)
    else
        echo "Warning: Validator key file $KEYRING_DIR/$validator_id_hex not found"
        validator_private_key="KEY_FILE_NOT_FOUND"
    fi
    
    # Output debug format as specified
    echo "- $validator_id: validator_id=$validator_id_hex, adnl_id=$adnl_id_hex, validator_pvt_key=$validator_private_key, adnl_pvt_key=$adnl_private_key"
    
    # Add comma if not the first validator
    if [ "$first_validator" = false ]; then
        echo "," >> "$TEMP_JSON"
    else
        first_validator=false
    fi
    
    # Create JSON object for this validator (without the unwanted fields)
    cat << EOF >> "$TEMP_JSON"
  {
    "election_id": $election_id,
    "validator_key_id": "$validator_id",
    "validator_adnl_key_id": "$adnl_id"
  }
EOF
done

# Close the JSON array
echo "]" >> "$TEMP_JSON"

# Copy the rust config template
cp "$CONFIG_RUST_JSON" "$OUTPUT_JSON"

# Check if validator_keys section exists in the original config
if jq -e '.validator_keys' "$CONFIG_RUST_JSON" > /dev/null 2>&1; then
    # Find the validator_keys section in the rust config and replace it with our generated keys
    jq --slurpfile keys "$TEMP_JSON" '.validator_keys = $keys[0]' "$CONFIG_RUST_JSON" > "$OUTPUT_JSON"
else
    # Add the validator_keys section if it doesn't exist
    jq --slurpfile keys "$TEMP_JSON" '. + {validator_keys: $keys[0]}' "$CONFIG_RUST_JSON" > "$OUTPUT_JSON"
fi

# Check if validator_key_ring section exists and if not, create it
if ! jq -e '.validator_key_ring' "$OUTPUT_JSON" > /dev/null 2>&1; then
    jq '. + {validator_key_ring: {}}' "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
else
    # Clear existing validator_key_ring
    jq '.validator_key_ring = {}' "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
fi

# Extract each validator key and add it to the validator_key_ring
validators_json=$(cat "$TEMP_JSON")
validator_count=$(echo "$validators_json" | jq '. | length')

for ((i=0; i<$validator_count; i++)); do
    validator_key_id=$(echo "$validators_json" | jq -r ".[$i].validator_key_id")
    validator_adnl_key_id=$(echo "$validators_json" | jq -r ".[$i].validator_adnl_key_id")
    
    # Get the private key from our extracted data
    validator_key=""
    adnl_key=""
    for validator_base64 in $validators; do
        validator=$(echo "$validator_base64" | base64 -d)
        current_id=$(echo "$validator" | jq -r '.id')
        if [ "$current_id" == "$validator_key_id" ]; then
            current_id_hex=$(base64_to_hex "$current_id")
            if [ -f "$KEYRING_DIR/$current_id_hex" ]; then
                validator_key=$(dd if="$KEYRING_DIR/$current_id_hex" bs=1 skip=4 count=32 status=none | base64)
            fi
            
            # Get ADNL key for this validator
            adnl_id=$(echo "$validator" | jq -r '.adnl_addrs[0].id')
            adnl_id_hex=$(base64_to_hex "$adnl_id")
            if [ -f "$KEYRING_DIR/$adnl_id_hex" ]; then
                adnl_key=$(dd if="$KEYRING_DIR/$adnl_id_hex" bs=1 skip=4 count=32 status=none | base64)
            fi
            break
        fi
    done
    
    # Add validator key to validator_key_ring
    jq --arg key_id "$validator_key_id" --arg key "$validator_key" \
        '.validator_key_ring[$key_id] = {"type_id": 1209251014, "pub_key": null, "pvt_key": $key}' \
        "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
    
    # Add ADNL key to validator_key_ring
    jq --arg key_id "$validator_adnl_key_id" --arg key "$adnl_key" \
        '.validator_key_ring[$key_id] = {"type_id": 1209251014, "pub_key": null, "pvt_key": $key}' \
        "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
done

# Update the ADNL keys - find the validator ADNL key and put it in the right place
for ((i=0; i<$validator_count; i++)); do
    validator_adnl_key_id=$(echo "$validators_json" | jq -r ".[$i].validator_adnl_key_id")
    
    # Get the private key for this ADNL ID
    adnl_private_key=""
    for validator_base64 in $validators; do
        validator=$(echo "$validator_base64" | base64 -d)
        adnl_id=$(echo "$validator" | jq -r '.adnl_addrs[0].id')
        if [ "$adnl_id" == "$validator_adnl_key_id" ]; then
            adnl_id_hex=$(base64_to_hex "$adnl_id")
            if [ -f "$KEYRING_DIR/$adnl_id_hex" ]; then
                adnl_private_key=$(dd if="$KEYRING_DIR/$adnl_id_hex" bs=1 skip=4 count=32 status=none | base64)
                break
            fi
        fi
    done
    
    # Update the adnl_node.keys[1] (tag 2) with the validator ADNL key
    # First check if adnl_node.keys exists
    if ! jq -e '.adnl_node.keys' "$OUTPUT_JSON" > /dev/null 2>&1; then
        # Create the structure if it doesn't exist
        jq '.adnl_node = (.adnl_node // {}) | .adnl_node.keys = []' "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
    fi
    
    # Check if keys[1] exists
    if jq -e '.adnl_node.keys[1]' "$OUTPUT_JSON" > /dev/null 2>&1; then
        # Update existing tag 2 key
        jq --arg key "$adnl_private_key" '.adnl_node.keys[1].data.pvt_key = $key' "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
    else
        # Add tag 2 key if it doesn't exist
        jq --arg key "$adnl_private_key" '
            .adnl_node.keys = (.adnl_node.keys // []) + [{
                "tag": 2,
                "data": {
                    "type_id": 1209251014,
                    "pub_key": null,
                    "pvt_key": $key
                }
            }]
        ' "$OUTPUT_JSON" > "${OUTPUT_JSON}.tmp" && mv "${OUTPUT_JSON}.tmp" "$OUTPUT_JSON"
    fi
done

# Cleanup
rm "$TEMP_JSON"

echo "Generated config file: $OUTPUT_JSON" 