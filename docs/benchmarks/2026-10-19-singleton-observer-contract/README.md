# Singleton room and observer contract, 2026-10-19

Source base `8c6fbc9`. Selected change: two focused tests plus protocol documentation; no production logic, allocation policy or performance change. The small-room receive path already avoids RoomFanout continuation allocation when its audience fits the quantum. The source is excluded from its own room broadcast, but a singleton room can still have authorized global channel-message observers. A one-room-member early return is therefore not a valid general optimization.

The first test publishes with only the source, adds one room peer, then removes it and publishes again. It checks no self-echo, the newly joined member's delivery, no stale delivery after removal, recipient-visit work counts, no continuation, zero outstanding queue reservations and zero routing-copy counters. The second authenticates a global observer, enables channel-message listening, publishes, disables listening and publishes, then re-enables and publishes again. It checks exactly two ordered deliveries to the enabled observer, no delivery while disabled, no source echo, no continuation, and released queue reservations. The existing Sequence test output verifies encoded values/FIFO. These are synchronous in-process routing checks, not real socket throughput measurements or concurrent permission-change stress.

The two selected cases pass against the existing clean runtime. An isolated mutation adds an early return whenever a non-global broadcast source is its room's sole member, before checking observers. The observer test then fails with the explicit message "enabled observer missed the singleton broadcast": actual delivery count 0, expected 1. The mutation is removed before final verification and is never applied to the working relay or built as a release service. The log demonstrates a lost-delivery regression, not merely a disagreement about internal work accounting.

After restoration, all 102 clean-snapshot library tests pass in 0.56 seconds, including existing socket/zero-copy/fairness/close checks. Clippy with tests and warnings denied passes, as does formatting. Only src/server/fairness_tests.rs differs from the committed source in the tested snapshot. The pre-existing three source overlays remain byte-identical and are not part of this build. The selected test file is already behind the existing cfg(test) module declaration; no production declaration changes.

No timing benchmark, 100,000-user rerun or multi-hour campaign is needed for a test-only change. The absence of a RoomFanout continuation is not a claim that the entire path allocates nothing: a lazy membership snapshot can still allocate. No isolated allocation, RSS or speed benefit is claimed. The purpose is to preserve delivery semantics before another optimization attempts to skip work. Existing services and convenience binaries remain unchanged.

From a clean checkout of the selected commit:

```sh
cargo test --lib singleton_
cargo test --lib
cargo clippy --lib --tests -- -D warnings
```

The snapshot source-hash manifest, selected test patch/source, original relay source, isolated mutation patch/source and actual logs are retained here. Run `python3 verify.py` in this directory for the portable evidence audit; it does not compile or launch a relay. The original-relay archive is the clean committed implementation, not the protected dirty working file.
