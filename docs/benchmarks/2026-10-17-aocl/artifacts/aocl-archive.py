#!/usr/bin/env python3
"""Archive the completed AOCL checkpoint; raw perf files and binaries stay local."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess

ROOT=Path(__file__).resolve().parents[1]
DEST=ROOT/"docs/benchmarks/2026-10-17-aocl"
MANIFEST=ROOT/"docs/benchmarks/2026-10-17-aocl.json"
BASE_REVISION="ec1b55b8b837a92c6c74e0ddedd081997fbc2752"

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def read(name):
    return json.loads((ROOT/"artifacts"/name).read_text())

comparisons={}
ordinary=0
for name in ("aocl-dense-1k","aocl-dense-1m","aocl-fanout-mixed","aocl-single-1m"):
    data=read(name+".json")
    assert len(data["runs"])==9 and all(row["passed"] for row in data["runs"])
    for row in data["runs"]:
        case=read(row["trace"])
        assert case["server_exit_code"]==0 and not case["server_forced_kill"]
        metrics=case["final"]["metrics"]
        if name=="aocl-fanout-mixed":
            result=next(entry for entry in case["output"] if entry.get("event")=="result")
            assert int(metrics["war_routing_copied_bytes_total"])+int(metrics["war_routing_reused_bytes_total"])==result["sent_value_bytes"]
        else:
            assert int(metrics["war_routing_copied_bytes_total"])==0
        assert all(int(case["final"]["metrics"][key])==0 for key in
                   ("war_connections","war_sessions","war_queued_bytes","war_read_buffer_reserved_bytes"))
    comparisons[name]={"artifact":f"2026-10-17-aocl/artifacts/{name}.json","summary":data["summary"],"passed_runs":9}
    ordinary+=len(data["runs"])
capacity=read("aocl-capacity.json")
assert capacity["passed"] and len(capacity["runs"])==9
for row in capacity["runs"]:
    case=json.loads((ROOT/row["trace"]).read_text())
    assert case["passed"] and case["server_exit_code"]==case["client_exit_code"]==0
    assert not case.get("forced_kill_pids")
comparisons["aocl-capacity"]={"artifact":"2026-10-17-aocl/artifacts/aocl-capacity.json","summary":capacity["summary"],"passed_runs":9}
ordinary+=9
churn=read("aocl-churn.json")
assert churn["passed"] and len(churn["runs"])==9
comparisons["aocl-churn"]={"artifact":"2026-10-17-aocl/artifacts/aocl-churn.json","summary":churn["summary"],"passed_runs":9}
ordinary+=9
assert ordinary==54

tests=read("aocl-rust-tests.json")
assert tests["passed"] and len(tests["runs"])==51
for label in ("zen5","dispatch","avx2"):
    log=(ROOT/f"artifacts/aocl-rust-tests-{label}.log").read_text()
    assert sum(int(n) for n in re.findall(r"test result: ok\. (\d+) passed",log))==141
    assert all(row["exit_code"]==0 for row in tests["runs"] if row["build"]==label)
assert "100% tests passed out of 157" in (ROOT/"artifacts/aocl-validator-zen5.log").read_text()
assert len(read("aocl-validator-boundaries.json")["cases"])==156
assert len(read("aocl-validator-avx2-boundaries.json")["cases"])==78
assert read("aocl-corpus-zen5.json")["case"]["passed"]
assert read("aocl-fragments.json")["passed"]
assert read("aocl-harness-checks.json")["passed"]
assert read("aocl-resolvers.json")["passed"]
bindings=read("aocl-bindings.json")
assert len(bindings["runs"])==4 and all(row["client_aocl_mapping_absent"] for row in bindings["runs"])
assert read("aocl-manual-acceptance.json")["passed"]

sources=[ROOT/"Cargo.toml",ROOT/"Cargo.lock"]+sorted((ROOT/"src").rglob("*.rs"))
runtime_changes=subprocess.check_output(["git","diff",BASE_REVISION,"--name-only","--","src","vendor","Cargo.toml","Cargo.lock"],cwd=ROOT,text=True)
assert not runtime_changes
patch=subprocess.check_output(["git","diff",BASE_REVISION,"--binary","--",".gitattributes","scripts","docs/STATUS.md","docs/operations.md","docs/performance.md"],cwd=ROOT)
(ROOT/"artifacts/aocl-changes.patch").write_bytes(patch)

files=[p for p in (ROOT/"artifacts").glob("aocl-*") if p.is_file() and not p.name.endswith(".perf.data")]
files.append(ROOT/".gitattributes")
for name in ("benchmark.py","compare.py","stability.py","capacity.py","capacity_pressure.py",
             "input-corpus.py","fragment-waves.py","fragment-fairness.py","reference.py"):
    files.append(ROOT/"scripts"/name)
files+=list((ROOT/"artifacts/aocl-harness-initial").glob("*.py"))
for name in ("aocl-dense-1k","aocl-dense-1m","aocl-fanout-mixed","aocl-single-1m"):
    files+=list((ROOT/"artifacts"/(name+"-traces")).glob("*.json"))
for label in ("zen5","dispatch","avx2"):
    files.append(ROOT/f"artifacts/aocl-libmem-5.3.2-{label}/CMakeCache.txt")
upstream=ROOT/"artifacts/reference/aocl-libmem-5.3.2"
for name in ("BUILD_RUN.md","README.md","LICENSE.txt","NOTICES","CMakeLists.txt","docs/User_Guide.md",
             "tools/validator/validator_guide.md","src/cpu_features.c","src/libmem_ifunc_dispatcher.c"):
    files.append(upstream/name)
for name in ("artifacts/baselines/c10m-sendmsg/build.json","artifacts/baselines/c10m-sendmsg/runtime.patch",
             "artifacts/baselines/capacity-final/build.json","examples/fragment_waves.rs"):
    files.append(ROOT/name)
archived=[]
for source in sorted(set(files)):
    relative=source.relative_to(ROOT)
    target=DEST/relative
    target.parent.mkdir(parents=True,exist_ok=True)
    shutil.copy2(source,target)
    assert digest(source)==digest(target)
    archived.append({"path":str(target.relative_to(MANIFEST.parent)),"source_path":str(relative),
                     "sha256":digest(target),"bytes":target.stat().st_size})

binaries={}
for label,name in (("native","artifacts/baselines/c10m-sendmsg/war-relay"),
                   ("capacity_client","artifacts/baselines/capacity-final/capacity"),
                   ("stress_client","artifacts/baselines/capacity-final/stress"),
                   ("fragment_client","target/release/examples/fragment_waves")):
    path=ROOT/name
    binaries[label]={"path":name,"sha256":digest(path),"bytes":path.stat().st_size}
profile=ROOT/"artifacts/aocl-large-profile.perf.data"
manifest={"schema":1,"date":"2026-10-17","parent_revision":BASE_REVISION,
    "decision":"Retain glibc. No AOCL candidate establishes a useful whole-workload benefit; keep the server-only evaluation support and corrected fragment-harness assertion.",
    "native_runtime_changed":False,"native_source_hashes":{str(path.relative_to(ROOT)):digest(path) for path in sources},
    "mixed_size_routing_policy":"Existing compaction detaches small views from large receive allocations; about 0.0452 percent of mixed-fanout routed value bytes are copied once. Reused plus copied value bytes match the client's expected total. Fixed-size cases have zero routed-body copies.",
    "baseline_manifest":{"path":"2026-10-17-c10m.json","sha256":digest(MANIFEST.parent/"2026-10-17-c10m.json")},
    "binaries":binaries,"aocl_builds":read("aocl-build.json"),"ordinary_passed_trials":ordinary,
    "comparisons":comparisons,"capacity_verified_timed_messages":sum(row["measurement"]["round_trip"]["samples"] for row in capacity["runs"]),
    "churn_verified_lifecycles":sum(row["result"]["verified_deliveries"] for row in churn["runs"]),
    "validation":{"rust_tests_per_library":141,"library_builds":3,"test_executables_per_library":17,
        "completed_upstream_size_tests":157,"completed_upstream_boundary_alignment_cases":234,
        "protocol_corpus_cases":776,"protocol_corpus_exchanges":2311,
        "fragment_cases":["glibc","zen5","dispatch","avx2","no-reclaim"],"fragment_messages_per_case":24,
        "initial_exhaustive_validation":"Stopped after 162 passes to bound cost; not a completed suite.",
        "fragment_harness_fix":"Original per-connection equality fails on glibc and AOCL after worker-shared receive storage. Require the 40 KiB worker/witness ceiling, exact resets, sample coverage and final zero; disabled reclamation retains 64 MiB."},
    "diagnostic":{"profile_raw_path":str(profile.relative_to(ROOT)),"profile_sha256":digest(profile),
        "profile_bytes":profile.stat().st_size,"samples":4911,"lost_samples":0,"expected_stop_signal":"SIGINT",
        "raw_profile_tracked":False,"ordinary_timing_uses_profile":False,
        "collector_privilege":"Separate root perf diagnostic; no sysctl or system preload changes."},
    "source_snapshot_notes":"Initial three comparisons use artifacts/aocl-harness-initial. Later ordinary comparisons use the archived current compare/stability/benchmark sources. Snapshots retain repository-relative paths; restore those paths to replay helpers. Runtime binary provenance is in the pinned baseline manifests.",
    "files":archived}
MANIFEST.write_text(json.dumps(manifest,indent=2)+"\n")
for entry in archived:
    assert digest(MANIFEST.parent/entry["path"])==entry["sha256"]
print(json.dumps({"manifest":str(MANIFEST.relative_to(ROOT)),"archived_files":len(archived),
                  "archived_bytes":sum(row["bytes"] for row in archived),"ordinary_passed_trials":ordinary,"verified":True}))
