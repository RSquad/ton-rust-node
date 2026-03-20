#!/bin/bash
set -e
source ./test_run_net.sh

function find_block {
    LOOP_RES=0
    for (( N=1; N <= NODES; N++ ))
    do
        if grep -E -q "Applied(.*)$1" < "$TEST_ROOT/tmp/output_$N.log" ; then
            if [ "$2" != "LOOP" ]; then
                echo "Applied block ($1) - FOUND on node #$N!"
            else
                ((LOOP_RES++))
            fi
        else
            if [ "$2" != "LOOP" ] ; then
                echo "ERROR: Can't find applied block ($1) on node #$N!"
                PID="$(ps ax | grep configs_$N | grep -v grep | awk '{print $1}')"
                gdb -p "$PID" -ex "thread apply all bt" -ex "detach" -ex "quit" > "$TEST_ROOT/tmp/output_trace_$N.log"
                ./stop_network.sh
                exit 1
            fi
        fi
    done
    if [ "$2" == "LOOP" ]
    then
        echo $LOOP_RES
    fi
}
echo "Waiting for first master block"
counter=0
until [ "$(find_block '-1\:8000000000000000, 1' 'LOOP')" == "$NODES" ]
do
    sleep 10
    counter=$((counter + 1))
    if [ $counter -gt 5 ]; then
        find_block "-1\:8000000000000000, 1"
        echo "Reached timeout limit"
        bash "$TEST_ROOT/stop_network.sh"
        exit 1
    fi
done
find_block "-1\:8000000000000000, 1"

echo "Waiting for 50th master block"
until [ "$(find_block '-1\:8000000000000000, 50' 'LOOP')" == "$NODES" ]
do
    sleep 10
    counter=$((counter + 1))
    if [ $counter -gt 20 ]; then
        find_block "-1\:8000000000000000, 50"
        echo "Reached timeout limit"
        bash "$TEST_ROOT/stop_network.sh"
        exit 1
    fi
done
find_block "-1\:8000000000000000, 50"

echo "Waiting for 50th shard block"
counter=0
until [ "$(find_block '0:(.*), 50' 'LOOP')" == "$NODES" ]
do
    sleep 10
    counter=$((counter + 1))
    if [ $counter -gt 30 ]; then
        find_block "0:(.*), 50"
        echo "Reached timeout limit"
        bash "$TEST_ROOT/stop_network.sh"
        exit 1
    fi
done
find_block "0:(.*), 50"

bash "$TEST_ROOT/stop_network.sh"
echo "TEST PASSED"
