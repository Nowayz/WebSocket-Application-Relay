# Stable release gate

A release is one Rust WebTransport server and one CBOR JavaScript/TypeScript
client, packaged and verified together. Its `release.json` and `SHA256SUMS`
identify the selected source and assets. No release has yet been accepted from
the WebTransport-only source; run the gate below before publishing one.

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
asset digests, installs the client in a fresh consumer directory and runs its
tests. It also starts the archive's executable and checks package exports,
license/types, management authentication, pagination, drain/
resume, seven zero resource gauges, and a clean exit on SIGTERM. A failure
retains its logs and is not a passed release gate. Output directories cannot be
reused. No script tags, publishes, installs or restarts an existing service.

The gate repeats that lifecycle check under each shipped deployment
environment, so a profile that fails validation or startup is caught. Node has
no `WebTransport`, so message delivery through the packaged client, including a
5 MiB value, is checked by the browser page that
`build:browser` writes, run against the packaged executable.

The server archive includes license/notice files for the dependency packages
reported by the actual Cargo build, plus an indexed digest list. The standalone
browser directory includes the project and bundled CBOR codec licenses. The
artifact gate verifies these files before testing the installed client.

## Acceptance checklist

- [ ] Product boundary and vendored patch provenance audited.
- [ ] Routing, FIFO, ownership, refusal accounting and bounded resource tests.
- [ ] Short 100,000-session screens with exact deliveries and cleanup.
- [ ] Impaired-network freshness checks (loss, jitter, reordering).
- [ ] Archive build, clean client installation and packaged lifecycle gate.
- [ ] Actual browser check using the release bundle and executable.
- [ ] Final source/asset identities and release acceptance recorded here.

Formatting, Clippy, Rust tests, JS tests, types and browser compilation are the
required local checks.

## Support and limits

Linux x86-64/glibc is the verified server platform. The client needs a runtime
with `WebTransport` and ES2022, such as a current browser. The archive includes
[installation instructions](../deploy/README.md), deployment examples, protocol
and operating documentation.

The 100,000-session profile is workload-dependent. Short local tests establish
delivery, cleanup and responsiveness for their recorded workload; they do not
establish a production tail-latency SLA or months-long reliability. Cold recipient
view construction, observer-barrier scans and synchronous pressure fallback remain
documented performance limits. Broadcasts are best-effort during pressure and
partly submitted output is not rolled back after later refusal. There is no
durable message store or automatic replay/reconnect.

Before deployment, the operator must choose host sizing, a publicly trusted
certificate, allowed Origins, secrets and application authorization, and verify
the profile in that environment.
These are deployment choices, not reasons to keep adding features to this release.
Publishing a GitHub release, uploading packages and switching a running service
are separate from producing and verifying the local release candidate.
