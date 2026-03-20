# TON Node Sync Test Watcher

This directory contains an automated sync test watcher for `ton-node`.

The watcher is a Node.js HTTP server (`server.js`) that:
- Starts on `NODE_WATCHER_HTTP`
- Runs sync test cases automatically on startup
- Manages `ton-node` lifecycle (start/stop)
- Optionally wipes `/db` between test cases
- Produces JSON/HTML status and report endpoints
- Optionally sends a summary/report to Slack

## Files

- `server.js`: watcher and test orchestration logic
- `package.json`: Node.js dependencies and start script
- `Dockerfile`: containerized build/runtime for the sync test watcher

## Test Cases

The watcher executes tests sequentially:

1. `Stop -> Wipe DB -> Start -> Wait for Sync`
2. `Stop -> Start -> Wait for Sync`

Sync is considered complete when both of these log-derived ages are `< 10s`:
- `Applied master block ... Ns old`
- `Applied block ... Ns old`

After all tests complete, the process exits with code `0`.

## HTTP API

Only `GET` is supported.

- `/status`
  - Returns watcher/node status, PID, uptime, sync flags, and config values.
- `/getlogs?last=N`
  - Returns the last `N` lines from `/logs/node-watcher.log`.
  - Default `N=100`, maximum `N=3000`.
- `/report`
  - Returns an HTML report for current/last test execution.

## Environment Variables

Required:
- `NODE_WATCHER_HTTP`
  - Listen address in `<host>:<port>` format (example: `0.0.0.0:32080`).

Optional:
- `SERVER_IP` (default: `127.0.0.1`)
- `NODE_RUN_ARGS` (default: `-c /main`)
- `SYNC_TEST_NETWORK` (label in report/slack)
- `SYNC_TEST_NODE_ID` (label in report/slack)

Slack (optional):
- `SLACK_WEBHOOK_URL`
- `SLACK_BOT_TOKEN`
- `SLACK_CHANNEL_ID`

## Logs and Data Paths

Inside the runtime/container, watcher expects:
- Node watcher log: `/logs/node-watcher.log`
- Node log: `/logs/output.log`
- Node DB: `/db`
- Node config base path usually under `/main` (via `NODE_RUN_ARGS`)

## Local Run (without Docker)

From this directory:

```bash
npm install
NODE_WATCHER_HTTP=127.0.0.1:32080 npm start
```

Notes:
- `ton-node` must be available in `PATH`.
- Ensure `/db`, `/logs`, and config path referenced by `NODE_RUN_ARGS` are valid for your environment.

## Dockerfile Usage

This project includes a dedicated `Dockerfile` at:
- `node/tests/test_sync/Dockerfile`

Build from repository root:

```bash
docker build -f node/tests/test_sync/Dockerfile -t ton-sync-test:local .
```

Run example:

```bash
docker run --rm \
  -p 32080:32080 \
  -e NODE_WATCHER_HTTP=0.0.0.0:32080 \
  -e SERVER_IP=127.0.0.1 \
  -e NODE_RUN_ARGS='-c /main' \
  -v $(pwd)/node/tests/test_sync/main:/main \
  -v $(pwd)/node/tests/test_sync/db:/db \
  -v $(pwd)/node/tests/test_sync/logs:/logs \
  ton-sync-test:local
```

Then query:

```bash
curl http://127.0.0.1:32080/status
curl 'http://127.0.0.1:32080/getlogs?last=200'
```

## Behavior Notes

- The watcher rotates `/logs/output.log` before each test case.
- `ton-node` is started detached and monitored by PID.
- On shutdown signals (`SIGINT`, `SIGTERM`), watcher tries graceful node stop and server close.
