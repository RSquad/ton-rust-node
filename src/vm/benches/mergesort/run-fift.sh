#!/bin/sh -ex

# make code boc
$TON/build/crypto/func -A -P mergesort.fc >mergesort.fif
echo "boc>B \"mergesort.boc\" B>file" >>mergesort.fif
$TON/build/crypto/fift -I $TON/crypto/fift/lib mergesort.fif

# run code boc
echo "\"mergesort.boc\" file>B B>boc <s 1000 | rot 3 runvmx" >mergesort-run.fif
$TON/build/crypto/fift -I $TON/crypto/fift/lib mergesort-run.fif
