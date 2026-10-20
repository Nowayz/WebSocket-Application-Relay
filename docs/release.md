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
- [ ] Archive build, clean client installation and packaged lifecycle gate.
- [ ] Actual browser check using the release bundle and executable.
- [ ] Final source/asset identities and release acceptance recorded here.

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
