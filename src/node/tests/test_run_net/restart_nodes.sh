#!/bin/bash

NODES=$(pgrep node_singlehost | wc -l)
NODES=6

echo "Stopping $NODES nodes..."

pkill node_singlehost
while pgrep -x "node_singlehost" > /dev/null
do
    sleep 1
done

echo "Rebuilding..."
if ! cargo build --release
then
    exit 1
fi

TEST_ROOT=$(pwd);
NODE_TARGET=$TEST_ROOT/../../../target/release/

echo "Restarting $NODES nodes..."

cd $NODE_TARGET

cp node node_singlehost

for (( N=0; N < $NODES; N++ ))
do
    echo "  Starting node #$N..."
    ./node_singlehost --configs configs_$N -z . >> "$TEST_ROOT/tmp/output_$N.log" 2>&1 &
done
