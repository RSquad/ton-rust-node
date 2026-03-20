#!/bin/bash
# Run test network with simplex consensus enabled
# Usage: ./run_test_net_simplex.sh [--sc]
#
# Options:
#   --sc    Enable simplex for shards only (default: masterchain + shards)

source .env/bin/activate

if [ "$1" == "--sc" ]; then
    echo "Running test network with simplex (shards only)..."
    python3 test_run_net.py --simplex
else
    echo "Running test network with simplex (masterchain + shards)..."
    python3 test_run_net.py --simplex-mc
fi
