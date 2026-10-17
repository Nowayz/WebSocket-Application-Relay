#!/usr/bin/env python3
"""Compare full join/echo/Close/EOF lifecycles with server-only AOCL libraries."""
import hashlib
import json
from pathlib import Path
import statistics
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/"scripts"))
from stability import run_case

binary=ROOT/"artifacts/baselines/c10m-sendmsg/war-relay"
client=ROOT/"artifacts/baselines/capacity-final/stress"
labels=["glibc","zen5","avx2"]
record={"server_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),
        "client_sha256":hashlib.sha256(client.read_bytes()).hexdigest(),
        "runs":[],"summary":{},"passed":False}
destination=ROOT/"artifacts/aocl-churn.json"
for repetition in range(3):
    order=labels[repetition:]+labels[:repetition]
    for position,label in enumerate(order):
        preload=(ROOT/f"artifacts/aocl-libmem-5.3.2-{label}/lib/libaocl-libmem.so").resolve() if label!="glibc" else None
        case=run_case("churn",5,16,server_binary=binary,stress_binary=client,server_cpus="0-3",client_cpus="4-7",
                      source_addresses=256,server_preload=preload)
        trace=ROOT/f"artifacts/aocl-churn-{repetition}-{position}-{label}.json"
        trace.write_text(json.dumps(case,indent=2)+"\n")
        assert case["passed"],(label,case)
        result=next(row for row in case["output"] if row.get("event")=="result")
        cpu=case["final"]["process"]["cpu_seconds"]-case["baseline"]["process"]["cpu_seconds"]
        row={"repetition":repetition,"position":position,"candidate":label,"trace":str(trace.relative_to(ROOT)),
             "result":result,"cpu_ns_per_lifecycle":cpu*1e9/result["verified_deliveries"],"passed":True}
        record["runs"].append(row)
        destination.write_text(json.dumps(record,indent=2)+"\n")
for label in labels:
    rows=[row for row in record["runs"] if row["candidate"]==label]
    rates=[row["result"]["deliveries_per_second"] for row in rows]
    record["summary"][label]={"passed_runs":len(rows),"median_lifecycles_per_second":statistics.median(rates),
        "min_lifecycles_per_second":min(rates),"max_lifecycles_per_second":max(rates),
        "median_server_cpu_ns_per_lifecycle":statistics.median(row["cpu_ns_per_lifecycle"] for row in rows),
        "median_p99_upper_ms":statistics.median(row["result"]["latency_p99_upper_ms"] for row in rows)}
record["passed"]=True
destination.write_text(json.dumps(record,indent=2)+"\n")
print(json.dumps({"completed":True,"summary":record["summary"]}),flush=True)
