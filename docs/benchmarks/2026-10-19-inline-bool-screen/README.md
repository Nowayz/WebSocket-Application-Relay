# Reject inline boolean acknowledgement screen

Decision: unselected. Production frame and socket code remain unchanged.

Boolean control replies already allocate nothing: they combine the existing inline control prefix with a static one-byte CBOR body. A candidate instead appends the boolean byte to the inline prefix and leaves the body empty. The frame size, wire bytes, byte/message admission, FIFO and ownership contracts stay unchanged. The existing vectored writer is deliberately retained. There is no batching change, unsafe code or new dependency.

A focused fixed-work real-socket client opens eight separate channels, then each connection sends and checks 12,500 authentication commands, for exactly **100,000 verified true boolean acknowledgements per run**. Each reply is compared byte-for-byte. Isolated channels avoid departure notifications interfering with outstanding requests. Every peer performs a Close handshake. This is an artificial control-heavy workload, not ordinary forwarding or 100,000-user traffic.

Four fresh processes run control/candidate/candidate/control. The server has four pinned workers on CPUs 0–3; the identical client has four runtime threads within a disjoint 4–7,20–23 mask. Server rate limits and heartbeat/idle timers are disabled for this fixed-work fixture. Server CPU is sampled through `/proc` before client launch and after client completion; the interval also includes connection setup and retirement. The host CPU tick is 10 ms, so these small CPU totals have coarse resolution. Elapsed time is whole-client elapsed time, not per-request latency. No builds or other benchmarks overlap the runs.

| Trial | Server CPU seconds | Whole-client elapsed seconds |
| --- | --- | --- |
| Control 1 | 0.16 | 0.106 |
| Candidate 1 | 0.18 | 0.115 |
| Candidate 2 | 0.17 | 0.123 |
| Control 2 | 0.19 | 0.115 |

CPU ranges overlap and the candidate shows no elapsed-time advantage. The screen provides no reason to replace the current static-body representation. It does not prove a universal regression. No longer performance or full-population campaign is warranted for this unselected change.

All four fixtures verify 100,000 input messages, 100,008 deliveries including welcomes, and eight accepted/joined/disconnected peers. Checked queue, session, variable, read-reservation and output-capacity gauges return to zero; error/pressure counters are zero and processes exit normally. The existing scalar-control vector test passes on the candidate, covering all integer and command-byte widths and both boolean values. No new test is added for this rejected prototype.

Frozen executable/build identities, complete raw receipts, the candidate patch and probe/controller sources are retained here. The control is `artifacts/candidates/native-inline-u64/war-relay`; candidate is `artifacts/candidates/native-inline-bool-candidate/war-relay`; the actual executed client is `artifacts/candidates/native-inline-bool-client-v2/bool_commands`. The first client build was not executed; its shared-channel design was replaced before testing. The probe/controller are diagnostic artifact sources, not installed production code. Existing manual services and unrelated dirty files remain unchanged. Run `python verify.py` for the offline receipt audit.
