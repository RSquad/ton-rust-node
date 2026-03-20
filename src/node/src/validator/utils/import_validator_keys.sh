#!/bin/bash

# import_validator_keys.sh - Import validator private keys using console CLI
# This script extracts validator private keys in the same way as extract_validator_keys.sh
# and imports them using the console CLI importprivatekey and addpermkey commands

# Display usage information
function show_usage {
    echo "Usage: $0 <cpp_node_path> <console_config_path>"
    echo ""
    echo "Parameters:"
    echo "  cpp_node_path       - Path to the C++ node directory containing db/config.json and db/keyring"
    echo "  console_config_path - Path to the console configuration JSON file"
    echo ""
    echo "Example:"
    echo "  $0 /path/to/cpp/node /path/to/console.json"
    echo ""
    echo "Note: The 'console' binary must be available in the shell PATH"
    echo "Note: Election dates and expiration times are extracted from individual validator configs"
    exit 1
}

# Check if we have the required arguments
if [ $# -ne 2 ]; then
    show_usage
fi

# Set paths from CLI arguments
CPP_NODE_PATH="$1"
CONSOLE_CONFIG="$2"

# Derive CPP config and keyring paths from the CPP node path
CONFIG_CPP_JSON="${CPP_NODE_PATH}/db/config.json"
KEYRING_DIR="${CPP_NODE_PATH}/db/keyring"

# Check if console binary is available
if ! command -v console &> /dev/null; then
    echo "Error: 'console' binary not found in PATH"
    echo "Please ensure the console binary is available in your shell context"
    exit 1
fi

# Check if config files exist
if [ ! -f "$CONFIG_CPP_JSON" ]; then
    echo "Error: $CONFIG_CPP_JSON does not exist"
    exit 1
fi

if [ ! -f "$CONSOLE_CONFIG" ]; then
    echo "Error: Console config $CONSOLE_CONFIG does not exist"
    exit 1
fi

# Check if keyring directory exists
if [ ! -d "$KEYRING_DIR" ]; then
    echo "Error: $KEYRING_DIR directory does not exist"
    exit 1
fi

# Check if jq is available
if ! command -v jq &> /dev/null; then
    echo "Error: 'jq' is required but not installed"
    exit 1
fi

# Function to convert base64 to hex uppercase
base64_to_hex() {
    echo "$1" | base64 -d | xxd -p -u | tr -d '\n'
}

# Function to convert binary key file to base64
key_file_to_base64() {
    local key_file="$1"
    if [ -f "$key_file" ]; then
        dd if="$key_file" bs=1 skip=4 count=32 status=none 2>/dev/null | base64 | tr -d '\n'
    else
        echo ""
    fi
}

echo "Starting validator key import process..."
echo "CPP Node Path: $CPP_NODE_PATH"
echo "Console Config: $CONSOLE_CONFIG"
echo "Using election dates and expiration times from individual validator configurations"
echo ""

# Use jq to parse the JSON and extract validator information
validators=$(jq -r '.validators[] | @base64' "$CONFIG_CPP_JSON" 2>/dev/null)

if [ -z "$validators" ]; then
    echo "Error: No validators found in $CONFIG_CPP_JSON"
    exit 1
fi

validator_count=0
imported_count=0
failed_count=0

echo "Found validators, processing..."
echo ""

for validator_base64 in $validators; do
    # Decode the base64 encoded validator object
    validator=$(echo "$validator_base64" | base64 -d)
    
    # Extract validator ID
    validator_id=$(echo "$validator" | jq -r '.id')
    validator_id_hex=$(base64_to_hex "$validator_id")
    
    # Extract election date from validator config
    validator_election_date=$(echo "$validator" | jq -r '.election_date')
    
    # Extract expire_at from validator config
    validator_expire_at=$(echo "$validator" | jq -r '.expire_at')
    
    # Extract ADNL address ID
    adnl_id=$(echo "$validator" | jq -r '.adnl_addrs[0].id')
    adnl_id_hex=$(base64_to_hex "$adnl_id")
    
    validator_count=$((validator_count + 1))
    
    echo "Processing validator #$validator_count:"
    echo "  Validator ID: $validator_id_hex"
    echo "  ADNL ID: $adnl_id_hex"
    echo "  Election Date: $validator_election_date"
    echo "  Expire At: $validator_expire_at"
    
    # Extract private keys from keyring
    validator_private_key=""
    adnl_private_key=""
    
    if [ -f "$KEYRING_DIR/$validator_id_hex" ]; then
        validator_private_key=$(key_file_to_base64 "$KEYRING_DIR/$validator_id_hex")
    else
        echo "  Warning: Validator key file $KEYRING_DIR/$validator_id_hex not found"
    fi
    
    if [ -f "$KEYRING_DIR/$adnl_id_hex" ]; then
        adnl_private_key=$(key_file_to_base64 "$KEYRING_DIR/$adnl_id_hex")
    else
        echo "  Warning: ADNL key file $KEYRING_DIR/$adnl_id_hex not found"
    fi
    
    # Dump key information to stdout
    echo ""
    echo "=== VALIDATOR #$validator_count KEY DUMP ==="
    echo "Validator Key ID (base64): $validator_id"
    echo "Validator Key ID (hex):    $validator_id_hex"
    echo "ADNL ID (base64):          $adnl_id"
    echo "ADNL ID (hex):             $adnl_id_hex"
    echo "Validator Private Key:     $validator_private_key"
    echo "ADNL Private Key:          $adnl_private_key"
    echo "========================================"
    echo ""
    
    # Variables to store key hashes for validator address association
    validator_key_hash_hex=""
    adnl_key_hash_hex=""
    
    # Import validator private key if available
    if [ -n "$validator_private_key" ] && [ "$validator_private_key" != "" ]; then
        echo "  Importing validator private key..."
        
        # Import the private key using console CLI
        import_output=$(console -C "$CONSOLE_CONFIG" -c "importprivatekey ed25519 $validator_private_key" 2>&1)
        
        if echo "$import_output" | grep -q "received public key hash"; then
            # Extract the key hash from the output
            validator_key_hash_hex=$(echo "$import_output" | grep "received public key hash:" | awk '{print $5}')
            validator_key_hash_base64=$(echo "$import_output" | grep "received public key hash:" | awk '{print $6}')
            
            echo "  ✓ Successfully imported validator key, hash: $validator_key_hash_hex"
            
            # Add permanent key for validation
            echo "  Adding permanent key for validation..."
            echo "console -C "$CONSOLE_CONFIG" -c addpermkey $validator_key_hash_hex $validator_election_date $validator_expire_at"
            addperm_output=$(console -C "$CONSOLE_CONFIG" -c "addpermkey $validator_key_hash_hex $validator_election_date $validator_expire_at" 2>&1)
            
            if echo "$addperm_output" | grep -q "success" || [ $? -eq 0 ]; then
                echo "  ✓ Successfully added permanent key for validation"
            else
                echo "  ✗ Failed to add permanent key: $addperm_output"
                failed_count=$((failed_count + 1))
                continue  # Skip ADNL processing if validator key setup failed
            fi
        else
            echo "  ✗ Failed to import validator private key: $import_output"
            failed_count=$((failed_count + 1))
            continue  # Skip ADNL processing if validator key import failed
        fi
    else
        echo "  ✗ No validator private key available to import"
        failed_count=$((failed_count + 1))
        continue  # Skip ADNL processing if no validator key
    fi
    
    # Import ADNL private key if available
    if [ -n "$adnl_private_key" ] && [ "$adnl_private_key" != "" ]; then
        echo "  Importing ADNL private key..."
        
        # Import the ADNL private key using console CLI
        import_output=$(console -C "$CONSOLE_CONFIG" -c "importprivatekey ed25519 $adnl_private_key" 2>&1)
        
        if echo "$import_output" | grep -q "received public key hash"; then
            # Extract the key hash from the output
            adnl_key_hash_hex=$(echo "$import_output" | grep "received public key hash:" | awk '{print $5}')
            echo "  ✓ Successfully imported ADNL key, hash: $adnl_key_hash_hex"
            
            # Associate ADNL key with validator key
            if [ -n "$validator_key_hash_hex" ] && [ -n "$adnl_key_hash_hex" ]; then
                echo "  Adding validator ADNL address..."
                addaddr_output=$(console -C "$CONSOLE_CONFIG" -c "addvalidatoraddr $validator_key_hash_hex $adnl_key_hash_hex $validator_expire_at" 2>&1)
                
                if echo "$addaddr_output" | grep -q "success" || [ $? -eq 0 ]; then
                    echo "  ✓ Successfully added validator ADNL address"
                    imported_count=$((imported_count + 1))
                else
                    echo "  ✗ Failed to add validator ADNL address: $addaddr_output"
                    failed_count=$((failed_count + 1))
                fi
            else
                echo "  ✗ Cannot add validator ADNL address: missing key hashes"
                failed_count=$((failed_count + 1))
            fi
        else
            echo "  ✗ Failed to import ADNL private key: $import_output"
            failed_count=$((failed_count + 1))
        fi
    else
        echo "  ✗ No ADNL private key available to import"
        failed_count=$((failed_count + 1))
    fi
    
    echo ""
done

echo "Import process completed!"
echo "Summary:"
echo "  Total validators found: $validator_count"
echo "  Successfully imported: $imported_count"
echo "  Failed imports: $failed_count"

if [ $failed_count -gt 0 ]; then
    echo ""
    echo "Warning: Some keys failed to import. Please check the error messages above."
    exit 1
else
    echo ""
    echo "All validator keys have been successfully imported and configured!"
fi 