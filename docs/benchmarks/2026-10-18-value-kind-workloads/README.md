# Equal-size byte/ASCII/UTF-8 workload coverage — 2026-10-18

Selected diagnostic feature: inspection client and both runners accept --value-kind bytes|ascii|utf8. The default remains bytes and the runners omit the flag for that default, preserving older frozen client commands. Text modes require the CBOR wire; original raw C++ byte transport is not mislabeled as CBOR text validation. The modern C++ CBOR wire remains eligible.

Every prepared private/fanout template contains 1,024 content bytes and a three-byte CBOR string header (1,027 encoded value bytes). ASCII uses the original contents under a text header. UTF-8 uses repeated é after the same eight-byte sequence marker, giving 1,024 bytes but 516 characters. Each mode keeps all 64 templates, sender/sequence and full-frame/FIFO validation. Templates and expected frames are prepared before timing; the hot loops continue cloning existing immutable packets. No server protocol or runtime mode is added.

Value type is reported in ready/measured client gates and runner arguments. The mixed runner applies it to sidecar private/fanout templates; its capacity probes remain bytes, as do churn/control/variable fixtures. This distinction matters when interpreting mixed traffic. No new payload allocation, decoding or conversion is added inside timed send/receive loops.

Eight client tests pass in five seconds, including the existing corruption and bounded-acknowledgement tests and a new all-64-template check for every value kind. It verifies default wire bytes exactly, decoded lengths/types, UTF-8 character count and complete envelopes. Strict example Clippy passes. Controller receipt/collision regressions also pass.

Six fresh two-second unpaced mixed fixtures use 64 private pairs, 2,048 fanout readers, window four and 4/8 pinned server/client workers. Order bytes/ASCII/UTF-8/UTF-8/ASCII/bytes; setup and retirement are additional. All pass payload/FIFO/counter/zero-copy/resource checks and normal exits.

| Value kind | Server CPU-seconds per million checked deliveries |
| --- | --- |
| Bytes | 1.292, 1.244 |
| ASCII text | 1.321, 1.268 |
| Multibyte UTF-8 text | 1.598, 1.564 |

ASCII overlaps byte results; UTF-8 is consistently more costly here. This unpaced traffic mix varies with scheduling, so these descriptive normalized figures are not a fixed-work penalty estimate or a server optimization result. Equal sizes prevent a trivial byte-count explanation, but content and validation paths differ. A separate two-second UTF-8 profile shows core::str::converts::from_utf8 at 42.68% of sampled native user cycles, zero lost samples. This is one profile and excludes kernel cycles; the percentage is not a whole-server speedup prediction. It identifies a text-validation target before changing the parser or adding a dependency.

A two-second mixed runner smoke with 1,600 capacity users and 512 UTF-8 fanout readers completes all 32 offered windows; both ready/measured sidecar gates report utf8. Full private/fanout CSV and timeline hashes and cleanup pass. This is harness integration, not a 100,000-user text acceptance claim. A one-second old frozen-client case proves bytes omits the new flag; a one-second convenience-client ASCII case proves the updated default executable accepts it. The default benchmark client is atomically updated only after guarding its previous hash; native/bridge/manual services are unchanged.

Reproduce after building the intended client:

```sh
cargo build --release --example inspection
python3 scripts/inspection.py --server target/release/war-relay \
  --mode none --rooms 0 --seconds 2 --inspectors 0 --pairs 64 \
  --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 \
  --pin-workers --pin-client-workers --value-kind utf8 \
  --output artifacts/new-utf8-screen.json
```

Adapt CPU masks and freeze binaries for comparisons; use fresh output names and separate profiles from ordinary timing. Raw receipts contain the exact measured frozen paths/identities. Builds/tests never overlap timed traffic. Build metadata pins selected source and new/previous convenience-client hashes, with root library overlays excluded. The selected patch applies to the clean base in build.json; restore value_tests.rs under examples/inspection/. No Cargo dependency was added. All raw data and test/build logs are retained; the large perf data stays at its local path/hash in summary.json. Run python verify.py for offline verification.
