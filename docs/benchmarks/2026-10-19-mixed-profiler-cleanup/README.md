# Mixed-load profiler timeout cleanup

The mixed-capacity runner waited ten seconds for the profiler in both its normal stop path and its finally block. If perf ignored SIGINT, the second timeout escaped before workload-child termination, log closure and final receipt writing. A controlled integration test reproduced missing final exit_codes in both paths: normal post-measurement stop and an earlier workload error.

The selected implementation shares one small stop helper between those paths. It sends SIGINT, waits the existing ten seconds, then kills and waits at most five seconds for reaping. Forced termination is recorded as profile.forced_kill and cannot validate a profile. The finally block records profiler cleanup exceptions and continues workload cleanup and receipt writing. Earlier workload errors remain recorded. No relay source or traffic policy changed. Other child cleanup deadlines remain as before; this is not a general process-supervisor redesign.

Verification:

- `python scripts/test-mixed-cleanup.py`: both formerly failing full-main cleanup paths pass; all three workload children terminate, forced profiler exit is -9, waits are exactly [10, 5], and the saved receipt is failed with the original error. Normal and already-exited profiler helper paths also pass. Two tests including four subcases take 0.004 seconds. Controlled objects avoid sleeping through timeouts.
- A separate actual Python child installed SIG_IGN for SIGINT, signaled readiness over stdout, and paused. Calling the selected stop helper killed and reaped it after 10.0034 seconds; [receipt](real-child.json). This checks real OS process retirement without traffic. The caller had a final bounded kill/reap safeguard.
- Existing `scripts/test-mixed-profile-failure.py`: four actual 32-user, 16-reader fault cases pass in 7.967 seconds total. Early profiler exit, exit during measurement, missing profile data and empty data remain failures, with target retirement checked. [Log](profile-regressions.log).
- Existing `scripts/test-mixed-output.py`: output preservation and worker-mask checks pass in 1.038 seconds. [Log](output-regressions.log).
- A separate ordinary 32-user, 16-reader two-second mixed run passed: two of two offered fanout windows, correctness_passed=true, passed=true, exit_codes=[0,0,0]. [Receipt](success.json). This verifies ordinary orchestration, not 100k fairness or a performance gain.

Real workload checks used existing frozen executables: metrics-static-20261019-native-candidate/war-relay, latency-timeline-client/capacity, and client-read-window-20261019-control/inspection under artifacts/candidates. Full paths, commands and hashes are retained in the success receipt. No builds or runtime source changes were needed. Existing manually running services and the three pre-existing dirty source files were preserved.

Before/after controlled logs are included. Reproduce the ordinary run with fresh output paths:

```sh
python scripts/mixed-capacity.py \
  --server artifacts/candidates/metrics-static-20261019-native-candidate/war-relay \
  --capacity-client artifacts/candidates/latency-timeline-client/capacity \
  --sidecar artifacts/candidates/client-read-window-20261019-control/inspection \
  --users 32 --seconds 2 --readers 16 --scope room --window 4 \
  --inspectors 0 --pairs 0 --churn-workers 0 --fanout-windows-per-second 1 \
  --output artifacts/mixed-cleanup-success-fresh.json
```

The fixed CPU masks require the recorded host topology. No benchmark latency or throughput conclusion is drawn from these diagnostic runs.
