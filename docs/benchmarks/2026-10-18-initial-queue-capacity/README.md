# Initial native output-queue capacity

Decision: retain the existing runtime. Explicitly reserving one or two initial queue slots substantially reduces retained queue storage, but neither screen establishes acceptable latency consistency for selection. The patches remain experiments; no runtime setting, allocation policy or executable was promoted.

The current VecDeque's first allocation holds four 80-byte Pending slots. A candidate adds `reserve_exact(1)` only when capacity is zero; a second uses two slots. Both retain ordinary subsequent growth, the existing release policy, exact queue accounting, original body owners/FIFO and write/deadline limits. Private output that completes immediately never enters this path.

Twenty fresh-process trials measure two seconds each: eight initial one-slot runs, four reversed-order one-slot confirmation runs, and eight two-slot runs. Every trial combines 64 private pairs and 2,048 fanout readers; fanout windows contain one or four 1 KiB messages. Four native workers are pinned to CPUs 0-3 and eight client workers to CPUs 4-7,20-23. Inspection and membership churn are disabled. Builds/tests do not overlap timed traffic. Setup and checked retirement add time outside the two-second measured phases; total ordinary measured traffic is 40 seconds.

| Experiment | Window | Variant | Retained queue bytes | CPU-s/million deliveries | Private p99 upper / max (ms) | Fanout p99 upper / max (ms) |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| one-slot | 1 | control | 655,360 | 1.3179 | 2.097 / 3.645 | 4.194 / 4.926 |
| one-slot | 1 | candidate | 163,840 | 1.2972 | 4.194 / 7.792 | 8.389 / 8.765 |
| one-slot | 1 | candidate | 163,840 | 1.3136 | 2.097 / 3.713 | 4.194 / 5.400 |
| one-slot | 1 | control | 655,360 | 1.2869 | 2.097 / 6.247 | 4.194 / 6.526 |
| one-slot | 4 | control | 655,360 | 1.3635 | 2.097 / 7.510 | 16.777 / 17.550 |
| one-slot | 4 | candidate | 163,840 | 1.2465 | 2.097 / 3.237 | 16.777 / 13.205 |
| one-slot | 4 | candidate | 234,640 | 1.2541 | 2.097 / 6.812 | 16.777 / 16.504 |
| one-slot | 4 | control | 655,360 | 1.2457 | 2.097 / 3.329 | 16.777 / 13.415 |
| one-slot-confirm | 1 | candidate | 163,840 | 1.3451 | 2.097 / 8.400 | 4.194 / 11.878 |
| one-slot-confirm | 1 | control | 655,360 | 1.3522 | 2.097 / 3.414 | 4.194 / 5.590 |
| one-slot-confirm | 1 | control | 655,360 | 1.2853 | 2.097 / 3.195 | 4.194 / 4.443 |
| one-slot-confirm | 1 | candidate | 163,840 | 1.3373 | 2.097 / 3.249 | 4.194 / 4.763 |
| two-slot | 1 | control | 655,360 | 1.3737 | 2.097 / 3.767 | 4.194 / 5.678 |
| two-slot | 1 | candidate | 327,680 | 1.3310 | 2.097 / 3.879 | 4.194 / 5.180 |
| two-slot | 1 | candidate | 327,680 | 1.3795 | 4.194 / 5.661 | 4.194 / 6.554 |
| two-slot | 1 | control | 655,360 | 1.4298 | 4.194 / 4.042 | 4.194 / 4.665 |
| two-slot | 4 | control | 655,360 | 1.3627 | 4.194 / 6.057 | 16.777 / 15.399 |
| two-slot | 4 | candidate | 327,680 | 1.3620 | 4.194 / 4.381 | 16.777 / 16.829 |
| two-slot | 4 | candidate | 327,680 | 1.2633 | 4.194 / 6.922 | 16.777 / 17.747 |
| two-slot | 4 | control | 655,360 | 1.2473 | 2.097 / 4.053 | 16.777 / 13.355 |

The one-slot version reduces retained queue capacity from 655,360 to 163,840 bytes in all four candidate one-message trials (75%). One of those trials has a higher private and fanout p99 histogram bucket; its candidate fanout maxima also reach 8.765 and 11.878 ms versus control maxima no higher than 6.526 ms. A reversed-order confirmation does not establish a consistently low maximum.

The two-slot version retains 327,680 bytes in all four candidate trials (50% below controls). Its p99 buckets match controls within each window-size screen, but both four-message candidate fanout maxima (16.829 and 17.747 ms) exceed both controls (15.399 and 13.355 ms). CPU ranges overlap. Larger queues grow normally and may need additional first-burst allocations. Lower slot memory alone does not justify selecting this tradeoff under the user's responsiveness priority.

These are unpaced small-population screens; private/fanout delivery mix varies. Histogram upper bounds and finite maxima cannot establish a per-user SLA, a cause for the observed differences or performance at 100,000 users. The measured memory reduction is real, but its full-platform benefit is not established. The rejection avoids a longer campaign or a more complicated queue representation.

All twenty receipts pass payload/FIFO/exact-counter checks, clean exits, zero routing-copy counters and final resource retirement. Both isolated candidate variants pass all 91 clean library tests (0.51 and 0.50 seconds); Clippy passes all targets with warnings denied. The source patches apply to clean base b92da66. Build manifests confirm recompilation and pin all sources/binaries; the one-slot confirmation adds only a source comment to that candidate. The three protected dirty files, manual services and convenience binaries were preserved.

Run `python docs/benchmarks/2026-10-18-initial-queue-capacity/verify.py` to audit the archive and recompute the result offline. Raw receipts, scripts, patches, clean source, build records and validation logs are retained.
