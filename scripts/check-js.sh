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
start_relay() {
  WAR_AUTH_PASSWORD=js-test-host WAR_ADMIN_TOKEN='' target/debug/war-relay \
    --bind 127.0.0.1:19138 --admin-bind 127.0.0.1:19139 --idle-timeout-secs "$1" >>"$task_logs/relay.log" 2>&1 &
  relay_pid=$!
}
wait_services() {
  for attempt in {1..100}; do
    if ! kill -0 "$relay_pid" 2>/dev/null || ! kill -0 "$bridge_pid" 2>/dev/null; then
      cat "$task_logs/relay.log" "$task_logs/bridge.log"
      return 1
    fi
    if curl --fail --silent http://127.0.0.1:19139/healthz >/dev/null; then return 0; fi
    sleep 0.05
  done
  cat "$task_logs/relay.log" "$task_logs/bridge.log"
  return 1
}
start_relay 0
target/debug/war-legacy-bridge --bind 127.0.0.1:19137 --upstream ws://127.0.0.1:19138 >"$task_logs/bridge.log" 2>&1 &
bridge_pid=$!
wait_services
WAR_TEST_IDLE_ENABLED='' WAR_TEST_URL=ws://127.0.0.1:19138 WAR_TEST_BRIDGE_URL=ws://127.0.0.1:19137 npm test --prefix clients/js
kill "$relay_pid"
wait "$relay_pid"
relay_pid=''
start_relay 1
wait_services
WAR_TEST_IDLE_ENABLED=1 WAR_TEST_URL=ws://127.0.0.1:19138 WAR_TEST_BRIDGE_URL=ws://127.0.0.1:19137 \
  node --test --test-timeout=10000 --test-name-pattern='application idle policy' clients/js/relay.test.mjs
