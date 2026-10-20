#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
task_logs=$(mktemp -d)
relay_pid=''
cleanup() {
  if [[ -n "$relay_pid" ]]; then kill "$relay_pid" 2>/dev/null || true; wait "$relay_pid" 2>/dev/null || true; fi
  rm -rf "$task_logs"
}
trap cleanup EXIT
# Deployment settings belong to the caller, not these disposable fixtures.
# Unset names without reading or logging their potentially secret values.
for fixture_var in "${!WAR_@}"; do unset "$fixture_var"; done
unset fixture_var
# Archived inputs may predate a shared Cargo cache. Rebuild in-tree crates,
# including patched dependencies, before accepting the fixture executables.
find src vendor -type f -exec touch -c {} +
touch -c Cargo.toml Cargo.lock
# Ask Cargo where it put these binaries, including custom target directories.
cargo build --locked --bins --message-format=json-render-diagnostics >"$task_logs/build.jsonl"
node - "$task_logs/build.jsonl" >"$task_logs/binaries" <<'JS'
const fs = require('node:fs');
const manifest = fs.realpathSync('Cargo.toml');
const paths = new Map();
for (const line of fs.readFileSync(process.argv[2], 'utf8').trim().split('\n')) {
  const artifact = JSON.parse(line);
  if (artifact.reason === 'compiler-artifact' && artifact.manifest_path === manifest && artifact.target.kind.includes('bin') && artifact.executable) {
    paths.set(artifact.target.name, artifact);
  }
}
for (const name of ['war-relay']) {
  if (!paths.has(name)) throw new Error(`Cargo did not report ${name}`);
  const artifact = paths.get(name);
  if (artifact.fresh !== false) throw new Error(`Cargo did not confirm recompilation of ${name}`);
  console.log(artifact.executable);
}
JS
mapfile -t binaries <"$task_logs/binaries"
npm ci --prefix clients/js --omit=optional --ignore-scripts --no-audit --no-fund
start_relay() {
  WAR_AUTH_PASSWORD=js-test-host WAR_ADMIN_TOKEN='' "${binaries[0]}" \
    --bind 127.0.0.1:19138 --admin-bind 127.0.0.1:19139 --workers 4 --idle-timeout-secs 1 >>"$task_logs/relay.log" 2>&1 &
  relay_pid=$!
}
wait_services() {
  local ready_deadline=$((SECONDS + 5))
  while ((SECONDS < ready_deadline)); do
    if ! kill -0 "$relay_pid" 2>/dev/null; then
      cat "$task_logs/relay.log"
      return 1
    fi
    # Bound both HTTP completion and TCP connection, including a stalled peer.
    if node - <<'JS'
const http = require('node:http');
const fail = () => process.exit(1);
setTimeout(fail, 1000);
http.get('http://127.0.0.1:19139/readyz', response => {
  if (response.statusCode !== 200) return fail();
  response.on('error', fail);
  response.on('end', () => process.exit(0));
  response.resume();
}).on('error', fail);
JS
    then return 0; fi
    sleep 0.05
  done
  cat "$task_logs/relay.log"
  echo 'JavaScript fixture readiness deadline exceeded.' >&2
  return 1
}

start_relay
wait_services
WAR_TEST_IDLE_ENABLED=1 WAR_TEST_URL=ws://127.0.0.1:19138 npm test --prefix clients/js
# The suite must retire its sockets and accounted memory before fixture shutdown.
for attempt in {1..100}; do
  if curl --fail --silent --max-time 1 http://127.0.0.1:19139/metrics >"$task_logs/final-metrics" \
    && awk '$1 ~ /^(war_connections|war_sessions|war_queued_bytes|war_variable_bytes|war_read_buffer_reserved_bytes|war_output_queue_capacity_bytes)$/ { count++; busy += ($2 != 0) } END { exit !(count == 6 && busy == 0) }' "$task_logs/final-metrics"; then
    echo 'JavaScript fixture resources retired: all six checked gauges are zero.'
    exit 0
  fi
  sleep 0.05
done
cat "$task_logs/final-metrics"
echo 'JavaScript fixture resources did not retire.' >&2
exit 1
