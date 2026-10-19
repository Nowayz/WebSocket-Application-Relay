# Latest selected native: 100,000-user short mixed acceptance

On 2026-10-18, a fresh clean build of the latest selected source passed one six-second offered mixed workload with all 100,000 private users, 8,192 fanout recipients, paced churn and management inspection. Setup, gate scans and exhaustive routing/Close/departure retirement add wall time; this is not a six-second complete invocation or a long-running SLA test.

The source/executable identity is in raw/build.json. It includes selected single-entry output, bounded channel-page selection, exact public helper storage and the smaller Axum feature set. The three pre-existing root overlays are excluded. This is consolidated acceptance, not a fresh paired performance comparison; no speedup relative to earlier reports or C++ is inferred.

| Check | Observed |
| --- | --- |
| Private population | 100,000 unique users, 24 measured probes each; 2,400,000 observations |
| Fanout offered/completed | 96/96 windows, four messages per window, 8,192 recipients |
| Timed fanout deliveries | 3,145,728; 384 observations per recipient |
| Median private user's observed maximum | 1.353 ms |
| Population p99 of private users' maxima | 4.920 ms |
| Worst private user's observed maximum | 7.756 ms |
| Median fanout recipient's observed maximum | 59.010 ms |
| Population p99 of fanout recipients' maxima | 64.508 ms |
| Worst fanout recipient's observed maximum | 64.844 ms |
| Server measured CPU | 20.40 CPU-seconds |
| Server sampled RSS at measured gate | 117,288 KiB, about 114.5 MiB; excludes kernel/client memory |
| Private generator maximum pre-send delay | 3.767 ms; no one-second cohort exceeds its 250 ms period |
| Exits | Native server, capacity client and sidecar all normal |

Private and fanout delays differ substantially. Completion and bounded retirement do not establish equally low latency across traffic classes; large fanout response tails remain a performance target. The population plot shows each user's finite maximum separately for both classes, with distinct vertical scales. Private per-user p99 equals maximum at only 24 observations. Fanout has 384 samples per reader. These include generator/client scheduling and kernel work and do not isolate relay-hop time. No production or long-term tail guarantee follows.

Every user's routing is verified before and after traffic. Each timed private payload/sender/sequence and fanout recipient/sequence is checked. All 96 offered fanout windows complete, separately from eventual-delivery correctness. Exact aggregate lifecycle/input/output/reuse counters pass. Final connections/sessions/queued reservations/variables/read reservations/native queue capacity/copied-routing bytes are zero, server descriptor count returns to baseline and all child exits are zero. Client worker affinity is checked before and after timing. No build, profile or other benchmark overlaps offered traffic.

The fixture uses eight server workers, sixteen individually pinned client workers across the capacity client and sidecar, four private probes per second/user, sixteen fanout windows per second, eight churn workers paced at four cycles per second and two CBOR inspectors. Full command, masks, identities, stages, per-user CSVs and counters are retained. Use the current performance guide command with seconds=6, fresh artifact names and a deliberately frozen build to reproduce. The files in raw contain the exact invocation and associated build metadata. Manual services are not replaced by this acceptance build.

Run python verify.py for checksums, decoded CSV identities, unique-user/sample counts, population statistics and offer/cleanup gates. Run python plot.py to regenerate the standalone PNG/SVG. All raw data are compressed losslessly; CSV/timeline decoded hashes match the original runner receipt. No required run was left active.
