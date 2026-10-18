# Remove the bridge's upstream Nagle delay — 2026-10-18

## Kept fix

Code `db798cee46055f1d6766d1d0e158c4f0e602fb41` switches the legacy bridge's upstream connection from `connect_async(upstream)` to `connect_async_with_config(upstream, None, true)`. The pinned connector's third argument is `disable_nagle`; the default wrapper passes false. Its implementation sets `TCP_NODELAY` on the TCP socket when true, before the WebSocket connection is established.

The bridge already enables NODELAY on its accepted legacy socket, and the native relay already uses it. This closes the inconsistent upstream hop for consecutive small realtime packets. The change is one connection-policy flag plus its explicit import/comment. It introduces no buffer, queue, message-limit, framing, timeout, data-copy, ownership or scheduling change. TCP congestion control/backpressure remains in force. Small consecutive writes can produce more TCP segments rather than being coalesced; responsiveness is the intended priority.

## Short before/after experiment

A disposable local upstream peer disables `TCP_QUICKACK` on its own accepted socket before each pair and after the first frame. It withholds application responses until both frames arrive, avoiding a response that would immediately acknowledge the first. The legacy test client and fixture server both have NODELAY enabled, isolating the bridge's upstream policy.

After independently observing arrival of the first packet, the parent waits 1 ms and sends a second tiny legacy private packet. The recorded forwarding interval starts immediately before that second send call and ends when the fixture application reads its full forwarded CBOR frame. It includes client encoding, bridge scheduling/translation, TCP and fixture scheduling; it is not a kernel-only timestamp or full request/reply RTT. The intentional 1 ms spacing is excluded from that interval.

Every trial independently checks literal CBOR bytes and sequence data, echoes both messages through the bridge and verifies the full legacy outputs in FIFO order. Each fresh session verifies its welcome and a complete normal 1000 Close/EOF exchange. There are no machine-wide sysctl changes, public listeners, packet loss or artificial sleep inside the bridge.

Control and candidate are clean-source builds in the same project path, with the same compiler, dependency lock, release settings and environment. Only the upstream connector flag/import/comment changes. Fresh processes run **control / candidate / candidate / control**, with eight trials each. No build/profile overlaps a timing case.

| Build | Median second-packet forwarding, repetition 1 | Repetition 2 | Maximum, repetitions |
|---|---:|---:|---:|
| Default upstream Nagle | 39.7254 ms | 39.6495 ms | 39.9670; 40.7946 ms |
| Upstream NODELAY | 0.01817 ms | 0.01742 ms | 0.03048; 0.03668 ms |

All 32 matched trials pass full data/FIFO and Close/EOF checks. A preliminary probe of the previous manual executable `8e8b5c77…` also reproduces the approximately 40 ms stall; it is retained separately from the matched controls. A subsequent probe of the final clean selected executable records **0.03606 ms median / 0.05965 ms maximum** over another eight trials, with the same byte and closure checks. Every selected/candidate second-packet observation is below every matched control observation.

This demonstrates removal of an ACK-timer-sized forwarding stall **under the controlled delayed-ACK condition**. It is not a claim that every production message saves 40 ms, a native-relay throughput speedup, a p99 guarantee, or a 100,000-user result. Normal quick-ACK conditions can hide the old issue. The selected build differs from the prototype source only in the explanatory comment and build location; the final executable is tested separately and bound to the committed source.

## Real-relay compatibility and actual adoption

Clean selected-source verification passes **78 library tests**. The workspace passes **81 library tests**, including its three pre-existing listener-index tests. Strict library/bridge Clippy and changed-file rustfmt pass. No new test-only production helper or runtime abstraction is added.

The selected bridge passes the real JavaScript interoperability suite against the unchanged native relay. The idle-disabled pass has six tests passing and one explicitly skipped. A second pass enables a one-second native application-idle policy and passes **all seven tests, zero skipped**:

- CBOR objects/bytes/text/uint64 IDs and concurrent variable requests.
- The unchanged original `relay.js` through the migration bridge.
- Binary/text interoperability between native and legacy sessions.
- Privileged commands, channel-list layout and fixed-ID takeover.
- Normal close handshakes before and after joining.
- Clean unsupported-structured-value closure while native routing remains usable.
- Clean application-idle 1001 closure through both native and legacy browser sockets.

Both temporary processes exit zero in each pass, and connection/session/queued/variable gauges return to zero.

The local manual bridge is promoted only after verifying the old process/executable identity, no established clients on port 1337 and zero native live-state gauges. Its environment and arguments are preserved. The old process is verified stopped before starting the new one; automatic rollback is available if the live probe fails. The live probe verifies complete 1 KiB private and broadcast bodies, exact departure notification, two normal 1000 Close/EOF exchanges and native resource retirement. It passes without rollback or interruption of a live client.

Selected bridge: source **`db798ce`**, executable **`e8b0362e3ea5fb9b20605271ee90f3a58df58fa0bc1db5508e8812196e2dee83`**, PID **1055440**, port **1337**. The convenience `target/release/war-legacy-bridge` is atomically aligned with the frozen tested executable. Manual metadata agrees with its actual process hash. Native PID **966632** / `3db9ebeb…` and static UI PID **219427** remain unchanged. All three original user-modified C++/relay files remain byte-identical.

## Source and reproduction

The archive contains complete raw timings, literal-wire probe, reference client source, matched binaries/source/build recipe, final clean executable/build metadata, all test logs, real-relay scripts and guarded adoption receipt. `dependency-source/connect.rs` is the actual pinned vendored connector showing the default false flag and NODELAY setter. The clean selected module and Cargo files are included. `dependencies.json` binds the preceding full clean library/vendor and frozen native/old-bridge provenance.

`manifest.json` protects the archive and decoded binary hashes. The offline verifier checks the exact source delta, binary/source pins, trial populations/ordering/non-overlap, test outcomes, selected build association and live-adoption receipt:

```sh
python3 docs/benchmarks/2026-10-18-bridge-upstream-nodelay/verify.py
```

To rerun the short probe, restore the saved artifact script under repository `artifacts/`, then use a fresh output path:

```sh
python3 artifacts/bridge-nagle-probe.py \
  --bridge artifacts/candidates/bridge-upstream-nodelay-selected/war-legacy-bridge \
  --output artifacts/new-bridge-delayed-ack-probe.json
```

This Linux fixture uses only temporary local sockets and takes well under a second in the recorded runs. Its latency output is a diagnostic observation, not a universal timing threshold. The saved ABBA campaign supplies the paired controls. Compressed executable snapshots require executable mode after decompression. To reproduce the clean build, export the recorded Git revision and use its recorded command in an isolated directory; exclude the user's unrelated dirty files. Historical evidence remains unchanged.
