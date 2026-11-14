#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
# The client tests drive in-process WebTransport fixtures. Node has no
# WebTransport implementation, so no relay is started here.
npm ci --prefix clients/js --omit=optional --ignore-scripts --no-audit --no-fund
npm test --prefix clients/js
