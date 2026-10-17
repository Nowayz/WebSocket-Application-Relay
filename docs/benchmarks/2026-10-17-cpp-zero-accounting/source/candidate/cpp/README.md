# Modern C++ WAR relay

Experimental embeddable C++20 implementation of WAR CBOR v1, built on pinned modern
uWebSockets/uSockets with the native Linux epoll backend. It coexists with the
original `relay.cpp` and the Rust implementation. It is a separate candidate;
its availability does not change the selected Rust runtime.

```sh
cpp/setup.sh
cpp/build.sh
WAR_AUTH_PASSWORD=example WAR_ADMIN_TOKEN=example \
  artifacts/candidates/cpp-modern/war-relay-cpp \
  --bind 127.0.0.1:1348 --admin-bind 127.0.0.1:1349 \
  --workers 8 --pin-workers --max-connections 110000
```

These example ports keep the C++ process separate from the existing Rust relay.
Do not start independent relay instances on the same listening port: SO_REUSEPORT
is used to share a port among workers within one relay, not to share routing state
between processes.

The build produces `libwar-relay.a` and the CLI. A CMake target is also available:

```sh
cmake -S cpp -B artifacts/cpp-cmake -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_COMPILER=clang++
cmake --build artifacts/cpp-cmake -j
ctest --test-dir artifacts/cpp-cmake --output-on-failure
```

An embedding application includes only `relay.hpp` and uses a standard stop token:

```cpp
war::Config config;
config.port = 1348;
config.admin_port = 1349;
config.workers = 8;
config.max_connections = 110000;
std::jthread relay([config](std::stop_token stop) { war::run(config, stop); });
// ... application work ...
relay.request_stop(); // worker loops and transports are joined before run returns
```

Link the static relay library, uSockets and pthreads, or use the CMake `war-relay`
target. The `build.sh` archive uses ThinLTO and should be linked with Clang/lld;
the CMake build also supports producing an ordinary native static archive.
`run` reports bind/affinity startup failures after unwinding its worker set.
The CLI reads secrets from environment variables; embedding supplies them in Config.

The dependency revisions are pinned in `setup.sh`. Only uSockets is needed;
TLS/QUIC/compression/fuzzing submodules are not fetched. Dependency source and
build products live under ignored `artifacts/`. The relay's own code uses the repository's MIT license. Dependency headers retain
their Apache-2.0 notices; the upstream license is included under `patches/`.
The narrow, recorded
`patches/war-transport.patch` checks masking as soon as two header bytes arrive,
sends protocol/size Close receipts, normalizes malformed peer Close frames and
adds a physical transport-close notification. The hook carries no UserData access:
uWebSockets may have already destroyed that data at application Close. The build
refuses an unapplied patch and records compiler,
flags, source hashes, dependency revisions, and executable hash in
`artifacts/candidates/cpp-modern/build.json`.

## Ownership and concurrency

Each worker owns a uWebSockets app, SO_REUSEPORT listener, event loop, receive
scratch storage, and its sockets. Socket methods and destruction run on that
owner alone. Shutdown first quiesces in-flight producers across all loops, then
closes complete and partial HTTP/WebSocket transports and releases queued owners. Peer objects have immutable owner identity and shared ownership;
queued deliveries cannot address freed socket storage. A displaced session
cannot remove the replacement session's ID mapping.

Membership, ID assignment, retained variables and observer permissions are
protected by a shared metadata guard. Reads coexist; mutations use an exclusive
guard. A bounded cross-thread inbox carries deliveries to socket owners. Wakeups
are coalesced while an inbox is scheduled. Producers append under one inbox mutex;
the consumer swaps reusable vector batches and works outside that mutex. Each
callback visits at most 256
queued recipients. The producer never waits for a full queue: pressure closes
the slow destination. Per-peer message/byte limits and a global retained-output
byte budget bound queued work. The global budget covers remote deliveries and
uWebSockets output bytes together. The accounting includes receive-callback
cork bytes and retains a closing peer's output charge/admission slot until its
physical transport-close notification. Automatic protocol/control output is
reconciled after uWebSockets produces it; it is not a strict cap on every
transport allocation. Close framing has a small reservation and can fall back to
immediate EOF under exhaustion. Kernel socket storage, decoder buffers, fixed
worker storage and allocator capacity are separate costs.

Local delivery bypasses the inbox when that peer has no pending remote work.
Otherwise replies queue behind already selected data. This is necessary for a
permission opt-out reply to remain an ordering barrier. Source FIFO is preserved;
there is no total order promised across unrelated concurrent senders.

A complete fanout is selected/submitted under one metadata read guard. Huge
single-message fanouts can still delay metadata writers and the publishing loop.
The bounded inbox consumer quantum does not make that initial traversal bounded.
This implementation makes that tradeoff explicit rather than claiming lock-free
or wait-free routing.

## CBOR and copies

`cbor.hpp` validates definite CBOR using a fixed 65-entry stack, with WAR's
64-container depth and 65,536-item limits. Text is checked for valid UTF-8.
Opaque values remain encoded; no application object tree is allocated.

The pinned transport adapter also accounts for uWebSockets' 16 KiB loop cork
buffer when it contains output for a peer; dependency upgrades must review that
layout assumption and replay the pressure/sanitizer checks.

The incoming routing prefix is replaced in uWebSockets' writable callback
storage when it has enough room for the sender ID. The value itself stays in
place. If the prefix would grow, worker scratch storage supplies the envelope.
Cross-thread delivery materializes one shared immutable encoded message,
reused across its queued destinations. This ownership copy is required because
uWebSockets receive storage expires at callback return. Small synchronous sends
are also copied into uWebSockets' transport buffer. Larger uncorked plaintext
sends may take its writev path. This is not an end-to-end zero-copy transport.
Retained variables copy only their own key/value bytes.

## Validation and comparison

```sh
python3 cpp/check.py
python3 cpp/runtime-check.py
python3 cpp/pressure-check.py
python3 cpp/compare.py --seconds 6 --include-rust-candidate --output artifacts/cpp-modern-final-comparison.json
python3 cpp/capacity-compare.py --output artifacts/cpp-modern-final-100k.json
```

The full 776-case, 2,311-exchange corpus is imported from the independent shared raw wire corpus;
it checks valid/invalid CBOR, fragmentation, pipelining, exact payload bytes,
variable isolation, disconnect events, malformed frames and final cleanup. A
separate transport diagnostic preserves stock-versus-patched differences. The
corpus also passes with AddressSanitizer and UndefinedBehaviorSanitizer. Additional
checks exercise observers, permission ACK ordering, takeover, active/partial-HTTP
shutdown, failed binds, embedding, and a real unread destination. The comparison uses the same frozen
Rust load client and CBOR protocol against both servers, without a legacy bridge.
Fresh server processes run sequentially in rotated order. Worker CPU placement,
binary hashes, checked client output, latency bounds, CPU use and RSS are recorded.

This candidate currently provides routing/control semantics and explicit output,
variable and membership limits. It does not yet provide Rust's complete origin/IP
admission policy, aggregate decoder-storage admission/reclamation, heartbeat,
application/join/write deadlines, graceful drain API, rate buckets, authenticated
channel inspection JSON or full metrics schema. It binds only to loopback and
supports plaintext. These gaps must stay visible in performance reports; a narrow
successful benchmark does not establish production parity or 100,000-user
operational reliability.

Detailed measured results and their limits are in [BENCHMARKS.md](BENCHMARKS.md).
