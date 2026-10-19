# Input byte scheduling budget: measured, not selected

Retain the current scheduler. Starting from c6dad6d, the new mixed-size fixture identified higher small-fanout maxima with 64 KiB private traffic. Two bounded diagnostics establish a real difference in per-turn work; four ordinary two-second comparisons do not establish a repeatable application benefit from adding an input byte budget. No production code, public configuration or running service changes are selected.

## Mechanism and scope

The copied diagnostic adds a stack guard around the input loop and thread-local fixed-size histogram storage. It counts completed messages and their encoded bytes, with elapsed time from entering the input loop through routing/output to return. It emits one aggregate line per worker on exit. No per-message atomics, payload allocation or payload copy is introduced. Timer/policy handling before that loop, partial decoder reads without a completed message and resumed fanout turns are outside this scope. OS preemption may contribute to elapsed times.

In the control diagnostic, the largest turn processes 16 private messages, 1,048,816 encoded bytes (16 x 65,551). The 7,221 turns in the 1–2 MiB histogram bucket average 191.596 microseconds and reach 976.381 microseconds. Ordinary fanout input turns also perform recipient work: the 1–2 KiB bucket averages 182.948 microseconds. Message bytes alone therefore do not describe all turn cost.

The candidate adds a 256 KiB completed-message byte budget alongside the existing 256-frame/recipient-work budget. It subtracts complete message sizes and reschedules after the first message crossing the threshold. It neither truncates messages nor preempts validation within a single large frame. Ownership, payload sharing, FIFO and admission limits stay unchanged. This four-statement candidate is archived as candidate.patch, not applied to selected source.

The separate candidate diagnostic reaches four private messages / 262,204 encoded bytes per turn. The slight threshold overshoot is the last complete message. Its 256–512 KiB bucket averages 45.439 microseconds over 26,849 turns. The diagnostic confirms the intended shorter turns; these differently loaded, instrumented observations do not establish a speedup. The runner's profile_diagnostic field only tracks its perf/heap options and is false here; both read-turn diagnostics are explicitly instrumented and excluded from ordinary results.

## Ordinary socket comparison

Each fresh process uses four private pairs with 64 KiB content, 2,048 1 KiB fanout readers, four-message fanout windows, four individually pinned native workers and eight individually pinned client workers on disjoint core sets. Measurement is two seconds, plus setup and verified retirement. No inspection/churn, TLS or remote network; heartbeat/rate limits disabled, default output/memory admission. Order is control/candidate/candidate/control. No builds, tests, profiles or analysis overlap timed runs.

| Build | Private verified | Fanout verified | Native CPU s | Private p99 upper ms | Private max ms | Fanout max ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Control 0 | 135,648 | 2,056,192 | 4.55 | 4.194304 | 8.835146 | 22.221528 |
| Candidate 1 | 139,312 | 2,146,304 | 4.40 | 4.194304 | 7.418779 | 11.976233 |
| Candidate 2 | 91,296 | 2,965,504 | 4.67 | 4.194304 | 4.048029 | 13.697141 |
| Control 3 | 129,440 | 2,433,024 | 4.79 | 2.097152 | 5.548575 | 12.308563 |

An appealing first candidate result does not persist clearly against the returning control. Private and fanout maxima overlap; both candidate private p99 histogram bounds exceed the returning control's bound. CPU ranges overlap and completed private/fanout mixes vary. This unpaced screen cannot compare fixed-work efficiency or user-weighted population tails. It is enough to decline adding another scheduling constraint; it does not prove the two implementations equivalent or a byte budget never useful.

All four ordinary and both diagnostic fixtures pass full payload/sender/FIFO, exact message/byte/census counters, zero routing-copy bytes, resource retirement and normal process exits. All 111 clean candidate library tests pass in 0.57 seconds; strict library/test Clippy passes. Candidate behavior is correct in these checks, but remains unselected for lack of repeatable workload improvement. No broader 100,000-user campaign is warranted by this screen. There are 12 measured traffic seconds total across these six cases.

## Reproduction and evidence

The native control is the frozen worker-count-20261019-current build. Its native runtime remains current; later commits before this study changed diagnostics/documentation and bridge Rustdoc. Build receipts pin the exact executable and source identities. The client is the frozen private-payload-20261019-client from the adjacent mixed-size report. Full commands and CPU masks are in the socket receipts. Choose fresh outputs, keep runs serial and adapt masks to available disjoint cores on another host.

The compressed base server source and three independent patches reconstruct all experimental variants. The diagnostics use the same instrumentation with/without the four-statement byte budget; ordinary candidate builds have no diagnostic instrumentation. Frozen manifests, raw socket receipts and test logs are retained here. Run `python verify.py` to check archived source reconstruction, executable identities, counters, retirement and diagnostic histogram maxima. It runs no workload or compiler. The verifier requires the system patch command and the adjacent mixed-size client's archived build manifest.
