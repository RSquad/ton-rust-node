#!/usr/bin/env bash
# CI wrapper for the nodectl e2e test.
# Runs run_singlehost_nodectl.py with CI-appropriate settings.
set -euo pipefail
cd "$(dirname "$0")"

export PARTICIPANTS_WAIT_SECONDS=600   # CI runners are slower
export NOBUILD=0                       # always rebuild in CI
export KEEP_NODECTL_ON_SUCCESS=0       # stop everything after test

exec python3 run_singlehost_nodectl.py
