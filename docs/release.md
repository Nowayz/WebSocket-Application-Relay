# Stable release gate

The release baseline is version 0.1.0: one Rust server and one native CBOR
JavaScript/TypeScript client. Performance experiments and adjacent features are
deferred until this baseline is packaged and verified.

## Build the three assets

Use a clean committed checkout, Rust 1.91+, Node 22+, npm and Python 3.12+:

```sh
python3 scripts/package-release.py --output artifacts/release-0.1.0
python3 scripts/check-release.py --release artifacts/release-0.1.0 \
  --output artifacts/release-0.1.0-check
(cd artifacts/release-0.1.0 && sha256sum --check SHA256SUMS)
```

The builder reads the committed Git archive, checks matching server/client
versions, rebuilds the release server, checks TypeScript, builds the browser
bundle, and creates a host-specific server archive, a source archive including
patched Rust dependencies, and an installable Node client tarball. Client
dependencies come from the checked-in npm lockfile; optional native addons and
install scripts are omitted. Source dependencies outside the vendored patches
still require network/cache access. This is a traceable build, not a claim of
bit-for-bit reproducibility across compilers or machines.

`release.json`, `SHA256SUMS`, package logs and the frozen build receipt identify
the source commit, tools, source digests and resulting assets. The checker verifies
asset digests, installs the client in a fresh consumer directory and runs all
client tests against the archive's executable. It also checks package exports,
license/types, management authentication, pagination, drain/rejected admission/
resume, six zero resource gauges, and normal terminal Close on SIGTERM. A failure
retains its logs and is not a passed release gate. Output directories cannot be
reused. No script tags, publishes, installs or restarts an existing service.

The server archive includes license/notice files for the dependency packages
reported by the actual Cargo build, plus an indexed digest list. The standalone
browser directory includes the project and bundled CBOR codec licenses. The
artifact gate verifies these files before testing the installed client.

## Acceptance checklist

- [x] Rust/CBOR product boundary and vendored patch provenance audited.
- [x] Routing, FIFO, ownership, refusal accounting and bounded resource tests.
- [x] Native observer/overload/input/descriptor recovery and shutdown checks.
- [x] Short mixed 100,000-user screens with exact deliveries and cleanup.
- [x] Archive build, clean client installation and packaged lifecycle gate.
- [x] Actual browser check using the release bundle and executable.
- [x] Final source/asset identities and release acceptance recorded here.

Formatting, Clippy, Rust tests, JS tests, types and browser compilation remain
required checks in CI. The CI release steps additionally build these archives and
runs their installation/lifecycle gate. Local acceptance and GitHub CI are
distinct; a local run does not attest to a remote workflow result.

## Support and limits

Linux x86-64/glibc is the initially verified server platform. The browser client
uses standard WebSocket/ES2022 features; Node 22+ is required for its Node usage.
The archive includes [installation instructions](../deploy/README.md), deployment
examples, migration notes, protocol and operating documentation. It carries no
old Binary/Text compatibility bridge or C/C++ comparison implementation.

The 100,000-connection profile is workload-dependent. Short local tests establish
delivery, cleanup and responsiveness for their recorded workload; they do not
establish a production tail-latency SLA or months-long reliability. Cold recipient
view construction, observer-barrier scans and synchronous pressure fallback remain
documented performance limits. Broadcasts are best-effort during pressure and
partly submitted output is not rolled back after later refusal. There is no
durable message store or automatic replay/reconnect.

Before deployment, the operator must choose host sizing, public TLS/Origins,
secrets and application authorization, and verify the profile in that environment.
These are deployment choices, not reasons to keep adding features to this release.
Publishing a GitHub release, uploading packages and switching a running service
are separate from producing and verifying the local release candidate.

## Initial local acceptance: 2026-10-19

The verified assets are built from commit
`9c7701ae47a1ac64e786921d894dd2789e9a8d66` on
`codex/departure-quantum`. This acceptance record is a subsequent documentation
change; the archives retain their original source commit and checksums.
The candidate is locally verified and has not been published or deployed.

| Gate | Observed result |
| --- | --- |
| Source fidelity | All 112 Cargo inputs from the tested routing-fix snapshot match the archive build |
| Core verification | 239 Rust tests passed, one manual diagnostic ignored; formatting and Clippy passed |
| Fixture tooling | 88 checks across 21 Python guard files passed |
| Product/provenance | 226 maintained files, one Rust server target; 62 pinned vendored files verified |
| Client distribution | Clean tarball install; all 66 JS tests passed against the packaged release server |
| Types/browser build | TypeScript and WebAssembly bundler passed in the copied release source |
| Packaged lifecycle | Authenticated management, pagination, drain/rejected admission/resume, six zero resource gauges and normal SIGTERM Close passed |
| Packaged overload | Native partial-observer rejection fixture passed with exact primary delivery, denied buffered follow-up and normal audience Close/EOF |
| Actual browser | Chromium 155: all seven bundle checks passed; no warning/error logs; six resource gauges zero and server exit 0 |
| Redistribution | Project/client licenses, browser codec notice, and notices for 109 compiled Rust dependency packages verified |
| Receipt integrity | All three archive digests passed; changed digest rejected before server startup; existing output refused |

The short mixed 100,000-user and native departure/fanout checks in
[rejection-accounting.md](rejection-accounting.md) apply to identical Rust inputs.
The packaged binary is a separate compiler-baseline build; those timing results
are not represented as measurements of this executable. No further performance
changes are required for this release baseline.

Executable SHA-256:
`487db3f7a6ea6977fba32062a7fc8b1bdb55c132fff099ead9a2e7925f38e4ce`.

| Release asset | SHA-256 |
| --- | --- |
| `war-relay-0.1.0-x86_64-unknown-linux-gnu.tar.gz` | `c369943eba91be2b0a2c5ba27a1d0b8e2159db2eccb611456f135320e6fbabf1` |
| `war-relay-0.1.0-source.tar.gz` | `d4bcc0bf259cdd3e67539e2d734f438492442480d16edb2b11b796e3080b2607` |
| `war-relay-client-0.1.0.tgz` | `6f4d5e5da6debe5039e36006d80a0475632e692b9a9e229be9361f731ee55c81` |

Local evidence is retained under `/home/phil/dev/war_next/artifacts/`:
`release-0.1.0/`, `release-0.1.0-check/`,
`release-0.1.0-browser-final/`, and `rejection-accounting-20261019/`.
The earlier browser fixture passed its UI checks but its harness sent SIGINT to
the child before reading final metrics; it is not the cleanup acceptance record.
The isolated final fixture verified cleanup before stopping its child normally.

The branch is now pushed with [draft PR #3](https://github.com/Nowayz/WebSocket-Application-Relay/pull/3).
The first remote CI run exposed a race in the shutdown fixture's accounting:
the self-reply barrier can retain a reservation briefly after the client receives
it. The test now settles welcome reservations, includes that barrier in its
bound, requires actual large output and returns one counter observation instead
of racing a second read. All 239 Rust tests and Clippy passed locally again.
The runtime implementation is unchanged by this correction.

The candidate above is superseded for publication until CI verifies the corrected
source archive. Use the artifacts and commit identified by the successful PR
check, rather than publishing this earlier source tarball. Publication/tagging
and target-host deployment remain separate steps; the existing local relay/bridge
services were not restarted or replaced.
