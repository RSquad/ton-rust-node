#!/bin/bash

NODES=$(pgrep node_singlehost | wc -l)
NODES=$((NODES - 1))
TEST_ROOT=$(pwd);
NODE_TARGET=$TEST_ROOT/../../../target/release/

pkill -9 node_singlehost
bash ./remove_junk.sh "$NODES" > /dev/null 2>&1
echo "Network stopped"