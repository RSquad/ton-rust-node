#!/usr/bin/env bash
# CI wrapper for the nodectl e2e test.
# Runs run_singlehost_nodectl.py with CI-appropriate settings.
set -euo pipefail
cd "$(dirname "$0")"

export PARTICIPANTS_WAIT_SECONDS=900   # CI runners are slower
export NOBUILD=0                       # always rebuild in CI
export KEEP_NODECTL_ON_SUCCESS=0       # stop everything after test
export PRINT_SENSITIVE=0               # don't print sensitive data in logs

exec python3 run_singlehost_nodectl.py
