# Large files remain ordinary relay messages

Both deployment profiles allow 16 MiB encoded messages and a 32 MiB per-session
output ceiling, matching the server defaults. The limit includes CBOR/routing
metadata. These ceilings are allocated only as needed; global decoder and queue
budgets still govern concurrent traffic. A 5 MB image needs no application-level
chunking or Base64 encoding:

```js
relay.send(peerId, new Uint8Array(await file.arrayBuffer()));
```

The former 64 KiB profile ceiling rejected a 128 KiB value with Close `1009`.
That was a deployment-policy regression, not a CBOR format limitation. Both the
message and output ceilings are now restored. Applications retain responsibility
for pacing large transfers, acknowledgment and retry under pressure. Byte-rate
limits and queued-write deadlines also apply; configure them for the admitted
message sizes and target network.

## Acceptance and socket setting

The native `5 MiB file values` client regression checks SHA-256 equality for
private delivery and two room recipients, following-message FIFO and a subsequent
control request. The release gate runs it against each packaged deployment
environment and requires large-body reuse, zero resources and normal shutdown.
The real browser check includes both 128 KiB and 5 MiB values.

The 100,000-user profile's former 16 KiB send-buffer request caused a large
transfer stall on the test host. Nagle was already disabled (`TCP_NODELAY=true`)
in all of these runs. The relay
does not cork application output or wait on an output batching timer. The exact
kernel interaction behind the small-buffer slowdown has not been packet-traced.
Using the same release executable, four workers on CPUs 0–3 and the same Node
client on CPUs 8–15:

| Requested send / receive buffer | Whole 5 MiB private-plus-room check |
| --- | --- |
| 16 / 64 KiB, two control repeats | 5,043 / 5,062 ms |
| 64 / 64 KiB, two repeats | 67 / 70 ms |
| OS send default / 64 KiB | 67 ms |
| 16 KiB / OS receive default | 4,025 ms |

The check sends two large publications to three destinations, verifies hashes
and markers, and includes joining/control work. These are local whole-check
times, not isolated server-hop latencies or Internet transfer guarantees. The
selected profile requests 64 KiB send/receive buffers. Linux commonly reports
twice the requested socket limits; account for kernel memory separately from
application queues. The larger limit is not preallocated for every idle socket.

Eight-worker, 100,000-user screens used two seconds of steady traffic and eight
observations per user, in control/candidate/candidate/control order. Every run
completed 16/16 offered fanout windows with exact delivery and cleanup. With
unpaced private pairs and inspection, worst receive latency was 3.45–3.62 ms
for the control versus 3.93–4.67 ms for the candidate; intended-send maxima were
5.41–7.03 versus 5.81–10.06 ms. Those worse candidate tails are retained.
A fixed-offered follow-up without unpaced pairs/inspection had worst receive
latencies of 3.77–4.21 versus 2.78–3.66 ms and CPU of 4.12–4.13 versus
4.14–4.15 seconds. These short screens support selecting the substantial large-
file improvement while keeping small-message responsiveness in the observed
range; they do not establish a tail-latency SLA or improve every tail sample.

The raw artifacts are retained by the maintainer outside the repository.
The mixed driver raises the message
and queue ceilings equally for both controls; only the send-buffer request
differs. The executable/build receipt is retained with them.
