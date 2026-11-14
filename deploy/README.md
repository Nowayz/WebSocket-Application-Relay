# Install the server archive

These paths are relative to the extracted `war-relay-VERSION-HOST` server
archive. Verify `SHA256SUMS` beside the downloaded assets before extraction.
The binary is built for the host named in the archive and `release.json`.
The initial release is verified on Linux x86-64 with glibc; other platforms
require their own build and acceptance. The binary is dynamically linked to
the host C runtime. This project contains no maintained C/C++ implementation.

Run locally first. `--dev-cert` generates a temporary localhost certificate and
logs its SHA-256 pin, which a local client passes as `serverCertificateHashes`:

```sh
./bin/war-relay --version
./bin/war-relay --help
./bin/war-relay --dev-cert --bind 127.0.0.1:1338 --admin-bind 127.0.0.1:1339
```

Clients connect to `https://HOST[:PORT]/war` over WebTransport (HTTP/3 on UDP).
The relay terminates TLS itself; no reverse proxy sits in front of it. The
server has no persisted routing state to migrate. SIGTERM stops admission,
closes sessions with code 1001 and retires them. Reconnecting clients join new
sessions; applications decide whether to replay their state.

On a systemd Linux host, install `bin/war-relay` as `/usr/local/bin/war-relay`
and `deploy/war-relay.service` as `/etc/systemd/system/war-relay.service`.
Put a publicly trusted certificate chain and key for the relay's hostname at
`/etc/war-relay/fullchain.pem` and `/etc/war-relay/privkey.pem` (root-only);
the unit hands them to the service as systemd credentials. Any ACME client can
obtain them. The relay reads them at startup, so restart it after renewal.
Copy and edit `deploy/war-relay.env.example` as `/etc/war-relay.env`, protect
secrets with root-only permissions, then reload systemd and enable/start the
service. Open UDP 443 (or your `WAR_BIND` port) in the firewall, keep the
management listener private, and read `docs/operations.md` before external use.

For 100,000 sessions, use `deploy/war-relay-100k.env.example` and install
`deploy/war-relay-100k.conf.example` as
`/etc/systemd/system/war-relay.service.d/100k.conf` instead of relying on the
base unit's smaller memory ceilings, and raise `net.core.rmem_max` to at least
the profile's UDP receive buffer. Adapt the profile to the host and workload.
Session count alone does not establish a latency guarantee.

`browser/relay.js` is the standalone browser ES module; `browser/relay.d.ts`
provides its TypeScript declarations. Serve `browser/` over HTTP(S) to run its
interactive check against the relay. The check
includes 128 KiB and 5 MiB binary values. Both supplied deployment profiles use
the server's 16 MiB encoded-message limit and a 32 MiB per-session output ceiling,
so an image can be sent as one message. These are admission ceilings, allocated
only as needed; shared global budgets still bound retained input/output. The
message limit includes the CBOR value and routing metadata. Applications needing
larger messages can configure `WAR_MAX_MESSAGE_BYTES` together with
`WAR_QUEUE_BYTES` and the global budgets; see `docs/operations.md`.
The separate `war-relay-client-VERSION.tgz` asset installs in a Node project with:

```sh
npm install --omit=optional --ignore-scripts /path/to/war-relay-client-VERSION.tgz
```

A project can then import `{ Relay }` from `@war/relay-client`. It needs a
runtime with `WebTransport`, such as a current browser. The client tarball
includes its usage documentation. Neither installing the server archive nor the
client silently starts a service. Keep `third-party/` with redistributed server
archives and `browser/LICENSE*` with the standalone browser bundle; these contain
the dependency notices.

Keep the preceding server archive and matching client artifacts for rollback.
Drain the current service, stop it, replace the executable, and start the selected
version. Existing sessions must reconnect.
