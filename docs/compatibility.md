# Original relay compatibility

The Rust relay implements the upstream application behavior with CBOR as its
primary wire format. The optional legacy bridge preserves the original
little-endian binary and base64-prefixed text layouts for existing clients.
Native routing keeps application values in shared received storage through
recipient queues and vectored socket output; the compatibility work described
here adds verification tooling and does not alter that ownership path.

## Running the original executable

The preserved upstream commit is
`881c8daaf2cf05adea8f522b28cf372e8f82d4e9`. Its bundled Linux/x86-64 executable,
`app`, has SHA256
`ce95e4dea01a4db0e5c32942d5520b5f4a8ab1c87affdcd8120bc8aefa332547`.
The differential check executes an identical copy of that binary. It does not
recompile or patch the C++ source or executable.

The host lacks its TBB 2 and OpenSSL 1.1 libraries. The check downloads two pinned
packages from Debian's official archive, verifies their SHA256 values before
extraction, and supplies their libraries through the reference process's
`LD_LIBRARY_PATH`:

- [libtbb2 2020.3-1](https://archive.debian.org/debian/pool/main/t/tbb/libtbb2_2020.3-1_amd64.deb).
- [libssl1.1 1.1.1w-0+deb11u1](https://archive.debian.org/debian/pool/main/o/openssl/libssl1.1_1.1.1w-0+deb11u1_amd64.deb),
  which supplies both `libssl.so.1.1` and `libcrypto.so.1.1`.

The pinned checksums were checked against
[Debian bullseye's archived package index](https://archive.debian.org/debian/dists/bullseye/main/binary-amd64/Packages.xz).
The script does not install packages or change system libraries. It uses the
host's remaining compatible libraries and records hashes of the libraries
actually mapped by the reference process. This is the original executable
running with these local dependencies, rather than a reconstruction of its
entire historical operating system.

The executable, Rust relay, bridge, and independent test client run inside one
`bubblewrap` network namespace, with a private process namespace for cleanup.
The filesystem is mounted read-only except for
`artifacts/reference`, which holds the copied executable, packages, extracted
libraries, logs, and reports. The test verifies that its network namespace differs
from the caller's. The original hardcoded `1338` listener therefore cannot
conflict with a host relay. The reference's certificate paths are absent on this
host; its unmodified TLS setup falls back to plaintext. No TLS acceptance is
inferred from this check.

On Linux/x86-64 with Python 3.12+, `ar`, and working unprivileged `bwrap` network
namespaces:

```sh
cargo build --locked --release --bins
python3 scripts/reference.py --output artifacts/reference/report.json
```

The first run fetches the two packages; later runs verify and reuse the cache.
No Python packages are required. `--relay` and `--bridge` accept other built
binary paths. The script removes its temporary processes on success and failure
and writes the requested report after a successful comparison. The report records
binary, harness, dependency hashes, namespace identities, exact normalized
transcripts, final Rust metrics, and process exit codes. Rust services exit
normally on SIGTERM; the original process is terminated with SIGTERM after the
check.

## Compared behavior

The independent Python client constructs masked WebSocket frames directly and
checks received bytes before producing a report. It uses neither the new client
encoder nor the bridge's encoder to construct legacy packets. The unchanged
upstream `relay.js` also has separate real-socket coverage in `check-js.sh`.

Forty-eight common cases cover:

- Text and binary joins; empty, 16-byte, binary/non-UTF-8, and 16-byte UTF-8 channel
  names; unique nonreserved IDs; rejection of overlong text and binary names.
- Binary and text broadcasts, cross-channel private delivery, self delivery,
  unknown targets, global broadcasts, individual listener flags, and avoiding
  duplicate private delivery when a listener is also the target.
- Payloads that put complete legacy binary messages at WebSocket lengths 125,
  126, 65,535, and 65,536; exact arbitrary bytes and UTF-8 text; two-message
  ordering at each recipient.
- Binary and text messages containing 1,024 empty continuation frames, including
  a UTF-8 codepoint split before those continuations and completed afterward.
- Ignored text relay commands and malformed listener lengths; authenticated
  channel-list names, populations, and layout; shared, isolated, empty, and
  missing variables, including binary keys and values.
- Fixed-ID assignment, subsequent private delivery, takeover routing to the new
  owner, private-message listeners receiving their own sent message, and ordinary
  client departure with an exact Close acknowledgement.

Each exchange includes an ordered variable-query reply as a barrier where the
sender remains open. The test checks every live recipient, including peers that
should receive nothing, and observes a 75 ms quiet interval after the expected
frames arrive. Ordering is checked per recipient. Random IDs are replaced with
session labels only after exact byte comparisons pass. Channel-list entries are
sorted because their upstream map iteration order is unspecified. Large payloads
are represented by size and digest in the report after full-byte comparison.
This finite timing window and selected packet corpus do not establish behavior
for every possible scheduling or network condition.

## Explicit behavioral improvements

The separate [performance comparison](performance.md#direct-c-comparison-2026-10-16)
records that the preserved executable has one working listener in this
environment. A rebuilt four-worker source configuration binds all listeners,
but short concurrent churn checks expose a repeated welcome ID, self-delivery
mismatch and timeout. Those diagnostics are recorded separately from the
48-case compatibility suite below; they do not have successful throughput
results. The [diagnostic record](benchmarks/2026-10-16-cpp-rebuilt-churn-diagnostics.json)
includes the exact configuration, client logs and source hashes.

The harness separately asserts eight difference cases. These are existing Rust
behaviors, now checked against an actual C++ process:

| Case | Original executable | Rust relay through the bridge |
| --- | --- | --- |
| Overlong binary channel Close reason | Includes a terminal NUL | Omits the NUL; retains code `1002` |
| Overlong text channel Close reason | Includes a terminal NUL | Omits the NUL; retains code `1002` |
| Fixed-ID takeover | Disconnect event reports ID zero; Close reason includes a terminal NUL | Reports the departed owner's ID; retains code `4001` with a normal reason string |
| Departure from `re_globl` | A peer with the disconnect listener flag receives the same event twice | Delivers the event once |
| Departed channel membership | Includes departed members until garbage collection | Removes membership immediately |
| Last-member variables | A quick rejoin can read the departed channel's retained variable before garbage collection | Releases the ordinary channel and its variables when its final member leaves |
| Unauthenticated channel query | Silently ignores the command and leaves the connection open | Closes with `1008` / `Authentication Required` |
| Assigning the current ID | Disconnects its own session with `4001` | Succeeds and keeps the session usable |

The two retirement checks complete before the original 30-second garbage
collection interval; the script refuses a run that misses its 25-second guard.
It does not measure the duration or correctness of the original garbage
collector. The Rust relay's independent connection, queue, variable, rate, and
deadline limits also remain in force. The fixture configures the isolated Rust
process with the upstream public test password; normal Rust startup has no
hardcoded password.

## Recorded acceptance

Three fresh-process runs on 2026-10-15 passed all 46 common cases and all eight
explicit difference checks on each implementation:

- [Run 1](benchmarks/2026-10-15-cpp-reference-1.json).
- [Run 2](benchmarks/2026-10-15-cpp-reference-2.json).
- [Run 3](benchmarks/2026-10-15-cpp-reference-3.json).

Each implementation also acknowledged eight remaining client closes during
teardown. Every run returned the Rust process to zero sessions, connections,
queued bytes, and variable bytes, with only the persistent global channel left.
Native write errors, write timeouts, Close failures, and transport disconnects
were zero. One native handshake error per run is expected from the TCP readiness
probe, which connects and closes before performing an upgrade.

The [2026-10-16 bounded-parsing check](benchmarks/2026-10-16-fairness-reference.json)
passes all 48 common cases and the same eight explicit differences. The two
added cases exercise 1,024 empty continuation frames in binary and text input,
including split UTF-8, through the original executable and the Rust bridge.
These exceed the bridge's 256-frame parsing quota and verify exact payload
preservation across the new cooperative steps. Both Rust services shut down
successfully; final staging, queue, connection, session and variable gauges were
zero with no native write, timeout, Close or transport failures.

This is application and legacy wire compatibility evidence. CBOR framing,
resource bounds, shared-body ownership, receive-buffer retention, and performance
have their own Rust, client, and workload checks. The comparison does not measure
C++ versus Rust throughput, allocation counts, production load, or TLS behavior.
