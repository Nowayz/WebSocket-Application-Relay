# Install the server archive

These paths are relative to the extracted `war-relay-VERSION-HOST` server
archive. Verify `SHA256SUMS` beside the downloaded assets before extraction.
The binary is built for the host named in the archive and `release.json`.
The initial release is verified on Linux x86-64 with glibc; other platforms
require their own build and acceptance. The binary is dynamically linked to
the host C runtime. This project contains no maintained C/C++ implementation.

Run locally first:

```sh
./bin/war-relay --version
./bin/war-relay --help
./bin/war-relay --bind 127.0.0.1:1338 --admin-bind 127.0.0.1:1339
```

The server has no persisted routing state to migrate. SIGTERM stops admission,
flushes shutdown Close frames where possible, and retires sessions. Reconnecting
clients join new sessions; applications decide whether to replay their state.
The old Binary/Text wire formats require a client upgrade; read `docs/migration.md`.

On a systemd Linux host, install `bin/war-relay` as `/usr/local/bin/war-relay`
and `deploy/war-relay.service` as `/etc/systemd/system/war-relay.service`.
Copy and edit `deploy/war-relay.env.example` as `/etc/war-relay.env`, protect
secrets with root-only permissions, then reload systemd and enable/start the
service. Configure TLS ingress using `deploy/Caddyfile.example` and keep the
management listener private. Read `docs/operations.md` before external use.

For 100,000 connections, use `deploy/war-relay-100k.env.example` and install
`deploy/war-relay-100k.conf.example` as
`/etc/systemd/system/war-relay.service.d/100k.conf` instead of relying on the
base unit's smaller descriptor/memory ceilings. Adapt the profile to the host
and workload. Connection count alone does not establish a latency guarantee.

`browser/relay.js` is the standalone browser ES module; `browser/relay.d.ts`
provides its TypeScript declarations. Serve `browser/` over HTTP to run its
interactive check against the relay (use HTTPS/WSS for a public site). The
separate `war-relay-client-VERSION.tgz` asset installs in a Node project with:

```sh
npm install --omit=optional --ignore-scripts /path/to/war-relay-client-VERSION.tgz
```

Node 22+ can then import `{ Relay }` from `@war/relay-client`. The client tarball
includes its usage documentation. Neither installing the server archive nor the
client silently starts a service. Keep `third-party/` with redistributed server
archives and `browser/LICENSE*` with the standalone browser bundle; these contain
the dependency notices.

Keep the preceding server archive and matching client artifacts for rollback.
Drain the current service, stop it, replace the executable, and start the selected
version. Existing WebSockets must reconnect. Downgrading to a Binary/Text server
also requires its matching old client; wire formats are not interchangeable.
