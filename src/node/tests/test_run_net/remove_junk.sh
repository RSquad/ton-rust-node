#!/bin/bash

NODES=$1
TEST_ROOT=$(pwd);
NODE_TARGET=$TEST_ROOT/../../../target/release/

rm -rdf $TEST_ROOT/tmp/*
for (( N=0; N <= $NODES; N++ ))
do
    rm -rf -d $NODE_TARGET/node_db_$N > /dev/null 2>&1
    rm -rf -d $NODE_TARGET/configs_$N > /dev/null 2>&1
done