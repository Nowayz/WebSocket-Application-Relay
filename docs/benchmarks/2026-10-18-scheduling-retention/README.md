# Scheduling queue retention diagnostic — 2026-10-18

Decision: retain the existing ready/notification queue policy. Two short real-socket fixtures show modest retained queue capacity at empty-worker gates; they do not justify adding allocation/reclamation branches to active scheduling.

| Fixture | Traffic phases | Maximum reported ready + pending storage per empty worker |
| --- | --- | ---: |
| Four population waves, 8,192 users | Four one-second phases, 10% active, rooms of two | 1,536 bytes (128 ready slots + 64 pending slots) |
| Fanout, 2,048 readers, four messages/window | Two seconds | 16,384 bytes (1,024 slots in each queue) |

Setup, checked joins/routing, Close/EOF and retirement add wall time. Four server workers and eight client workers use disjoint CPU masks. Both fixtures pass and normal process exits are recorded. Every population wave retires all owned TCP connections and live reservations. Fanout final connection/session/read/output/variable gauges and allocated output queue capacity are zero. These scheduling queues hold tokens, not payloads or peers; retaining their capacity cannot retain message bodies.

An isolated diagnostic emits queue capacities only when the connection arena and ready queue are both empty, after draining pending notifications. One additional diagnostic-only mutex acquisition reads the pending capacity. The tuple represents one worker, not a synchronized aggregate. The four workers' identities are not included, so do not sum repeated observations as if each represented a different worker. At these observed gates, four times the reported maximum is a conservative aggregate bound (6 KiB for the wave fixture, 64 KiB for fanout), not proof of a universal bound or a traffic peak. Tokens are eight bytes on this host. Allocator overhead and resident-memory retention are separate.

The fanout case has larger queues than the private-wave fixture, but still only tens of KiB across workers at the observed empty gates. A forced release would trade this warm storage for subsequent allocations. No candidate reclamation policy was implemented and no CPU/RSS gain is claimed. More extreme workloads may retain more; this diagnostic does not establish a leak or a global capacity guarantee.

The patch applies to the clean base in summary.json; three pre-existing dirty source overlays were excluded. Full commands, native/client executable identities, phase gates and counters remain in compressed receipts. Diagnostic timings are excluded from ordinary comparisons. Run python verify.py for offline verification. No selected runtime or manual service changed.
