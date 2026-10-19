# Short mixed-packet-size workload coverage

Selected diagnostic feature: `scripts/inspection.py` and its Rust client accept `--private-payload-bytes`, default 1,024. Private template contents include an eight-byte sequence marker, so smaller values are rejected before connecting or starting a fixture. This minimum follows the verification format, not a production relay restriction. Fanout templates remain 1 KiB. The mixed-capacity runner is unchanged and does not forward this option.

The existing 64 immutable request/expected-response templates are prepared before measurement. Binary, ASCII and multibyte UTF-8 content modes retain full sender/body/FIFO verification. Odd UTF-8 sizes retain a trailing ASCII byte after complete two-byte characters. Forwarded-value accounting now uses each pair's actual encoded length, including its CBOR string header, instead of the former constant 1,027. Ready/measured gates report the private size. Timed loops retain existing packet clones and add no payload allocation. The runner omits the flag at the default size, preserving frozen-client compatibility.

Four sequential two-second runs use one frozen native executable and one freshly built client: 1 KiB / 64 KiB / 64 KiB / 1 KiB private values. Each has four private pairs, 2,048 fanout readers, fanout window four, four individually pinned native workers and eight individually pinned client workers on separate CPU sets. This is unpaced saturation, not equal offered work or population fairness. Inspectors/churn are disabled, loopback/no TLS, default memory/output limits, heartbeat/rate limits disabled. Builds, tests and profiles did not overlap timed traffic. Setup and checked teardown add wall time.

| Private content | Run | Private verified | Small fanout verified | Private max ms | Fanout max ms | Native CPU s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 KiB | 0 | 406,976 | 2,990,080 | 3.416389 | 8.192617 | 4.17 |
| 64 KiB | 1 | 131,040 | 2,637,824 | 4.671041 | 11.081231 | 4.87 |
| 64 KiB | 2 | 87,808 | 3,112,960 | 5.159257 | 10.253276 | 4.75 |
| 1 KiB | 3 | 690,032 | 3,358,720 | 2.347780 | 8.585515 | 4.84 |

Large private messages produce higher observed maxima here, but completed packet/byte volumes vary substantially, including between returning controls. These results identify packet-size coverage as useful; they do not isolate scheduling cost or select a byte-based scheduling quota. Runtime scheduling, routing and output remain unchanged. A next causal diagnostic should measure bytes/work per connection turn before changing that budget.

All four runs pass exact body/FIFO/counter/census/zero-copy/resource-retirement gates and normal client/server exits. Four additional one-second socket cases pass: eight-byte binary, nine-byte UTF-8-mode (only the marker and trailing ASCII fit), 25-byte genuine multibyte UTF-8 with an odd tail, and 64 KiB multibyte UTF-8. A one-second old frozen-client run passes with the new flag omitted. Connections, sessions, queued/variable bytes, read reservations and output capacity return to zero in every case. The four mixed cases also retire all native arena slots; the small two-connection cases retain 4–8 empty slots under the existing small-arena reuse policy. These are nine traffic fixtures totaling 13 measured seconds, not a 100,000-user acceptance or long-duration latency guarantee.

Nine client tests pass in five seconds, including all-64 default-wire preservation, variable sizes across CBOR header boundaries, full envelope parsing and existing reader-corruption/deadline checks. Strict example/test Clippy passes. Seven runner preflight tests pass in 0.54 seconds, including three invalid private sizes without receipt creation or child launch. No native/bridge code or service binary was changed. Existing dirty source overlays were excluded from the clean build.

The four initial mixed receipts still contain the old generic scope sentence saying 1 KiB templates; their explicit args and ready/measured private_payload_bytes fields correctly record each workload. The runner prose was corrected afterward. Original receipts are retained unchanged. Later socket receipts use the corrected prose. Frozen build hashes, exact commands, source files and logs are archived here. Run `python verify.py` to check the archived evidence without starting traffic.

Example after building/freeze-verifying the updated client:

```sh
python scripts/inspection.py --server PATH_TO_NATIVE --client PATH_TO_INSPECTION \
  --private-payload-bytes 65536 --mode none --rooms 0 --seconds 2 \
  --inspectors 0 --pairs 4 --churn-workers 0 --workers 4 --client-workers 8 \
  --fanout-readers 2048 --fanout-window 4 --fanout-close-window 32 \
  --server-cpus 0-3 --client-cpus 4-7,20-23 --pin-workers --pin-client-workers \
  --output artifacts/FRESH_NAME.json
```

CPU masks describe this host. Select disjoint allowed cores elsewhere. Keep runs sequential, use fresh output paths and separate profiles from ordinary measurements.
