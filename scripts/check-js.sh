#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --bins
npm ci --prefix clients/js --ignore-scripts --no-audit --no-fund
task_logs=$(mktemp -d)
relay_pid=''
bridge_pid=''
cleanup() {
  if [[ -n "$bridge_pid" ]]; then kill "$bridge_pid" 2>/dev/null || true; wait "$bridge_pid" 2>/dev/null || true; fi
  if [[ -n "$relay_pid" ]]; then kill "$relay_pid" 2>/dev/null || true; wait "$relay_pid" 2>/dev/null || true; fi
  rm -rf "$task_logs"
}
trap cleanup EXIT
WAR_AUTH_PASSWORD=js-test-host WAR_ADMIN_TOKEN='' target/debug/war-relay \
  --bind 127.0.0.1:19138 --admin-bind 127.0.0.1:19139 >"$task_logs/relay.log" 2>&1 &
relay_pid=$!
target/debug/war-legacy-bridge --bind 127.0.0.1:19137 --upstream ws://127.0.0.1:19138 >"$task_logs/bridge.log" 2>&1 &
bridge_pid=$!
for attempt in {1..100}; do
  if ! kill -0 "$relay_pid" 2>/dev/null || ! kill -0 "$bridge_pid" 2>/dev/null; then
    cat "$task_logs/relay.log" "$task_logs/bridge.log"
    exit 1
  fi
  if curl --fail --silent http://127.0.0.1:19139/healthz >/dev/null; then break; fi
  sleep 0.05
done
WAR_TEST_URL=ws://127.0.0.1:19138 WAR_TEST_BRIDGE_URL=ws://127.0.0.1:19137 npm test --prefix clients/js
