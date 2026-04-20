#!/bin/sh
# Generates a minimal node config for sync test with random ADNL keys.
# Usage: gen-node-config.sh <ip> > node-0.json
set -eu

IP="${1:?Usage: gen-node-config.sh <ip>}"

DHT_KEY=$(openssl rand -base64 32)
FN_KEY=$(openssl rand -base64 32)
CTL_KEY=$(openssl rand -base64 32)

cat <<EOF
{
  "log_config_name": "/main/logs.config.yml",
  "ton_global_config_name": "/main/global.config.json",
  "internal_db_path": "/db",
  "restore_db": true,
  "sync_by_archives": true,
  "states_cache_mode": "Moderate",
  "skip_saving_persistent_states": false,
  "adnl_node": {
    "ip_address": "${IP}:30303",
    "keys": [
      { "tag": 1, "data": { "type_id": 1209251014, "pvt_key": "${DHT_KEY}" } },
      { "tag": 2, "data": { "type_id": 1209251014, "pvt_key": "${FN_KEY}" } }
    ]
  },
  "control_server": {
    "address": "0.0.0.0:50000",
    "clients": { "list": [] },
    "server_key": { "type_id": 1209251014, "pvt_key": "${CTL_KEY}" }
  },
  "metrics": { "address": "0.0.0.0:9100" },
  "gc": {
    "enable_for_archives": true,
    "archives_life_time_hours": 48,
    "enable_for_shard_state_persistent": true,
    "cells_gc_config": { "gc_interval_sec": 900, "cells_lifetime_sec": 86400 }
  },
  "cells_db_config": {
    "states_db_queue_len": 1000,
    "prefill_cells_counters": false,
    "cells_cache_size_bytes": 4000000000,
    "counters_cache_size_bytes": 4000000000
  }
}
EOF
