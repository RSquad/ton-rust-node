#!/usr/bin/env sh

set -e -o pipefail

run () {
    cargo fuzz run \
          $@ -- \
          -trace_malloc=[12] \
          -max_len=268435456 \
          $LFUZ_OPTIONS
}

export LSAN_OPTIONS='fast_unwind_on_malloc=0'
export ASAN_OPTIONS='fast_unwind_on_malloc=0'

FILE="fuzz-$(date +%F_%H:%M:%S:%N).log"

# We are trying to tee stderr separately from stdout.
# Saint Mother of Redirection
{ run $@ 2>&1 1>&3 | tee -a "$FILE" 1>&2 ; } 3>&1 | tee -a "$FILE"
