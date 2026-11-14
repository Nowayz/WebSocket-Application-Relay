# Selected transport: CPU and memory audit, 2026-11-07

The selected implementation is not demonstrated to be maximally memory efficient.
A new occupancy diagnostic measured 83.16 MiB of empty collection capacity and
41.20 MiB of unused inline previous-path storage across 100,000 connections.
These are representation costs in the current workload, not a promised additive
RSS saving. Retaining some capacity can avoid CPU work; each replacement still
needs an ordinary whole-workload comparison.

Selected image: `91337694ecef14ff3bb80bf37d13786aa5d4653e462096aeb3ddd92b71e19ea1`.
All 374 product inputs remain unchanged. Four fresh flat user-cycle profiles use
this exact image and existing fixed clients. Type sizes come from a separate
immutable build of the same source. Dynamic occupancy comes from an isolated
instrumented copy, with one snapshot per connection after its first post-join
application record is consumed. Builds, profiles and workloads ran serially.

## Where live memory goes

The exact selected 100,000-session heap contains 1,790,254,690 requested live
bytes (1,707.32 MiB). Every byte resolves to executable allocation sites and the
groups below sum to the profile header. This uses a replacement allocator and
must not be equated with ordinary RSS or added to it.

| Allocation group | MiB | Bytes per connection, amortized |
| --- | ---: | ---: |
| Quinn connection owner, including inline protocol core | 509.64 | 5,344 |
| Packet encryption keys | 219.76 | 2,304 |
| Protocol vector backing allocations, including global tables | 171.46 | 1,798 |
| Application task cells | 170.90 | 1,792 |
| Rustls QUIC session shell | 98.42 | 1,032 |
| Protocol hash tables, including endpoint CID indexing | 96.54 | 1,012 |
| Header protection keys | 48.83 | 512 |
| HTTP/3 capsule task cells | 48.83 | 512 |
| Remaining allocations | 342.94 | 3,596 |

The 5,344-byte owner is an Arc header (16), ConnectionInner (5,328), and its
embedded fields. ConnectionInner contains a 5,088-byte mutex-protected State,
232 bytes of shared notifications, and 8 bytes of padding. State contains the
4,656-byte protocol Connection plus 416 bytes of integration state. The mutex
overhead is 16 bytes, or about 1.53 MiB across these connections. It is not the
dominant memory cost.

Within the protocol core, major fields include streams (704 bytes), packet spaces
(560), statistics (536), previous-path option (432), current path (408), peer
parameters (352), RNG (320), remote CIDs (208), and timers (160). These fields are
already inside the owner total, not additional allocations.

The 1,792-byte application task contains a 1,656-byte core, 32-byte header,
48-byte trailer and 56-byte end padding. The session future is 1,560 bytes.
Replacing this with a pointer merely moves its storage unless its state/lifetime
representation also shrinks.

## Measured empty storage

The instrumented capacity run verified all 100,000 users, 400,000 measured
deliveries, and cleanup. Exactly 100,000 first-record snapshots were collected:

| Storage | Logical contents at each snapshot | Reserved bytes, total | MiB |
| --- | --- | ---: | ---: |
| Receive assembler backing arrays | 300,000 streams; zero chunks and zero payload bytes | 67,200,000 | 64.09 |
| Endpoint event queues | Zero queued events | 19,200,000 | 18.31 |
| Application event queues | Zero queued events | 800,000 | 0.76 |
| Inline previous-path field | Zero previous paths | 43,200,000 | 41.20 |

Update 2026-11-13: the endpoint-event queues, the assembler's first growth and
the inline previous-path field have since been addressed; see
[vendor/README.md](../vendor/README.md) and [performance.md](performance.md#memory).
The table above remains the record for the audited image.

The assembler keeps its BinaryHeap capacity after consuming chunks. A four-slot
backing allocation is 224 bytes; there are three receive streams per connection
here. This is a strong representation target. Freeing it after every read would
add allocation churn, so evaluate a compact common case or lifecycle-aware reuse.

The snapshots also contain 300,000 allocated receive-map entries with capacity
300,000 and the same counts for send maps. These are usable HashMap capacities;
they exclude hash-table control bytes. There is no evidence of excessive *spare
entry capacity* in these maps at this point. Send buffers still hold about
68.3 MB of unacknowledged payload across staggered snapshots; they cannot be
classified as empty or discarded. Initial and Handshake packet spaces are absent,
confirming those retired spaces are not accumulating in every connection.

These are staggered per-connection snapshots, not one simultaneous idle snapshot.
Empty capacity can be useful later. The previous cold-path layout already failed
the joint performance gate; measured unused storage does not override that result.

Statistics occupy about 51.12 MiB and RNG objects about 30.52 MiB at this scale.
Statistics are an observability contract, while RNG state is security sensitive.
Neither should be deleted or described as a free saving.

## CPU and allocation churn

The fresh profiles report zero lost hardware samples. Broad, disjoint symbol
prefix groups give the following approximate fractions of **user cycles**:

| Group | Private | Room fanout | Mixed | Datagram |
| --- | ---: | ---: | ---: | ---: |
| Protocol processing | 19.75% | 22.18% | 23.96% | 21.83% |
| Quinn integration/I/O | 9.63% | 5.65% | 7.67% | 10.58% |
| Crypto | 18.95% | 15.54% | 14.97% | 15.45% |
| Tokio scheduling, notifications, timers | 4.46% | 18.81% | 14.95% | 18.26% |
| Bytes ownership/buffer operations | 10.14% | 5.02% | 6.80% | 2.76% |
| libc, including unsymbolized internals | 20.57% | 14.72% | 15.03% | 15.23% |

These are self samples, not inclusive call trees or total-system CPU. The host
rejected DWARF attachment. Unsymbolized libc internals are not all assigned to
malloc or to a particular Rust caller. Crypto already executes VAES/AVX2 routines.
Prominent functions include populate_packet, poll_transmit, poll_read_buf,
ConnectionDriver::poll, Sleep::reset, and Bytes reference operations. The workload
matters: scheduling/timer costs are much more prominent in fanout and datagrams.

The separate replacement-allocator capture records 3,636,050 allocation events
requesting 1,458,442,295 bytes between ready and measured, during a workload with
400,000 verified deliveries: about 9.09 events and 3,646 requested bytes per
delivery. Monitoring, gate and background protocol work are included. Requested
bytes are allocation traffic, not simultaneously retained bytes or copied bytes.

The largest resolved stacks include UDP receive storage (about 447.6 MB),
application record buffers (410.0 MB), and packet tracking (at least 494.2 MB).
Source inspection confirms that ingress copies socket scratch into owned
BytesMut storage and application RecordReader allocates a body then transfers it
to Bytes. Payload ownership can outlive the read; reusing these buffers needs an
explicit lifetime and budget design, not unconditional overwriting.

One SentPacket is 96 bytes, but its BTreeMap leaf is 1,160 bytes (11 packet/value
slots plus keys and metadata). The observed packet-tracking requests have that
exact leaf size. The selected code releases the root when the last packet retires
to save idle memory. Repeated active sends therefore allocate new roots. This is
an intentional memory/CPU tradeoff. Earlier worker-local and shared tree pools
were rejected; fewer allocation requests did not establish a whole-workload win.
Any new small-live-set representation must preserve packet-number range queries,
ACK/loss handling, bounded memory and ordering.

## Purposeful crypto costs and next targets

Each packet-key allocation is 576 bytes: 544 bytes for the ring key wrapper,
12-byte IV, two 8-byte limits and padding. The inner AES/GCM state includes a
244-byte expanded AES schedule and 256-byte GHASH precomputation. About four
packet-key objects per connection correspond to current and prepared next keys
in both directions. Header keys contribute two 256-byte objects per connection.
The protocol precomputes next keys to avoid timing and denial-of-service issues;
these are not abandoned handshake objects. This explains the current cost but
does not prove it is the smallest possible secure implementation. See
[RFC 9001 key update](https://www.rfc-editor.org/rfc/rfc9001.html#section-6).

The next bounded investigations should target assembler storage, empty event
queues, small-live-set packet tracking, and packet/record ownership. Measure
scheduling and timer work on the fanout/datagram path separately. Keep existing
crypto and protocol behavior; a full handwritten SIMD/concurrency stack is not
supported by this audit.

After session retirement, requested live bytes fall to about 70.85 MB. Of that,
34.60 MB is endpoint CID-table capacity and 22.02 MB is connection-slab capacity.
Retained process RSS is not proof that live session objects leaked. Releasing
global capacity can affect later connection bursts and must be assessed over
reuse, not only final cleanup.

The [machine-readable audit](reports/webtransport-hotspot-memory-audit-2026-11-07.json)
contains hashed evidence, full field layouts, reconciled allocation stacks,
occupancy totals, CPU symbols and diagnostic limitations. Failed attachment,
controller and build attempts are preserved and excluded. No product change,
optimization gain, browser acceptance or deployment is claimed.
