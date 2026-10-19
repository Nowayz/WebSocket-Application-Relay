# Bridge terminal decoder errors

Selected: both frontend bridge stages share a small decoder-error Close mapping. Oversized frames retain 1009; malformed framing gets 1002; invalid UTF-8 gets 1007. Transport resets and unrelated I/O failures retain termination without a new Close attempt. No configuration, dependency, queue or valid-payload representation changed.

The previous selected bridge sends 1009 for oversized frames, but drops TCP for unmasked frames, reserved opcodes, fragmented Ping and invalid UTF-8. The independent baseline reproduces all eight missing-Close cases before/after joining. The selected build passes all ten cases, including the existing two oversized checks. Each wire fixture sends only 3 to 14 bytes and verifies exact native CBOR self-delivery afterward. All final connection/session/queue/variable gauges return to zero; disposable processes exit normally.

Terminal decoder errors now use a bounded Close send/flush only. They do not continue parsing an invalid stream while waiting for an acknowledgement. Normal received and initiated Close handshakes still use their existing helpers. These checks establish error Close receipt, not a completed normal Close handshake for malformed traffic.

The existing durable TCP regression is expanded to five inputs in both bridge stages. All 91 clean library tests pass in 0.54 seconds; Clippy passes all targets with warnings denied. All 34 native/legacy JavaScript compatibility tests pass in about 1.1 seconds. No throughput or capacity claim is made, and no long test was required.

The patch applies to clean base 69e62d1. build.json confirms recompilation and identifies all snapshot sources and the frozen bridge. Raw baseline/selected receipts, regression source and test logs are retained. The three pre-existing dirty source files, running manual services and convenience executables were preserved. Run `python docs/benchmarks/2026-10-18-bridge-frame-errors/verify.py` to audit the evidence offline.
