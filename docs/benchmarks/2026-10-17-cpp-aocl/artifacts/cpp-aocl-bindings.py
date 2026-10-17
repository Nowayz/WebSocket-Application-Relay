#!/usr/bin/env python3
"""Separate C++ loader diagnostics in disposable network/PID namespaces."""
import hashlib,json,os,shutil,subprocess,sys
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1]
CACHE=ROOT/"artifacts/reference"
CPP=CACHE/"build-4/app"
CLIENT=ROOT/"artifacts/baselines/active-pairs-client/inspection"
LIBRARIES={name:(ROOT/f"artifacts/aocl-libmem-5.3.2-{name}/lib/libaocl-libmem.so").resolve()
           for name in ("zen5","avx2","dispatch")}
record={"diagnostic_only":True,"cpp_sha256":hashlib.sha256(CPP.read_bytes()).hexdigest(),"runs":[]}
for name in ("glibc","zen5","avx2","dispatch","ignored-library"):
    destination=CACHE/f"cpp-aocl-binding-{name}.json"
    kind="cpp" if name=="glibc" else f"cpp-{name}"
    command=["bwrap","--unshare-net","--unshare-pid","--die-with-parent","--ro-bind","/","/",
             "--bind",str(CACHE),str(CACHE),"--proc","/proc","--dev","/dev","--",sys.executable,
             str(ROOT/"scripts/compare-cpp.py"),"--isolated","--host-netns",os.readlink("/proc/self/ns/net"),
             "--kind",kind,"--case","private","--relay",str(ROOT/"artifacts/baselines/c10m-sendmsg/war-relay"),
             "--client",str(CLIENT),"--client-source-root",str(CLIENT.parent),"--workers","4","--rooms","64",
             "--seconds","1","--pairs","4","--client-workers","4","--cpp-executable",str(CPP),
             "--cpp-system-libraries","--cpp-loop-count","4","--output",str(destination),"--loader-bindings"]
    for label,library in LIBRARIES.items(): command += ["--cpp-preload",f"{label}={library}"]
    if name=="ignored-library": command += ["--cpp-preload",f"ignored-library={ROOT/'Cargo.toml'}"]
    with (ROOT/f"artifacts/cpp-aocl-bindings-{name}.log").open("w") as log:
        run=subprocess.run(command,cwd=ROOT,stdout=log,stderr=log,timeout=35)
    result=json.loads(destination.read_text())
    shutil.copy2(destination,ROOT/f"artifacts/cpp-aocl-bindings-{name}.json")
    if name=="ignored-library":
        assert run.returncode==1 and not result["passed"] and "requested preload was not mapped" in result["error"],result.get("error")
        assert result["server_exit_code"]==-15 and not result["server_forced_kill"]
        record["ignored_library_rejected"]=True
        continue
    assert run.returncode==0 and result["passed"] and result["client_aocl_mapping_absent"],name
    assert result["diagnostic_only"] and result["server_exit_code"]==-15 and not result["server_forced_kill"]
    raw=Path(result["loader_bindings_file"])
    target=ROOT/f"artifacts/cpp-aocl-bindings-{name}.txt"
    shutil.copy2(raw,target)
    functions=("memcpy","memmove","memset","strncmp","strlen","memchr","bcmp")
    bindings=[line.strip() for line in raw.read_text().splitlines()
              if "binding file "+str(CPP)+" " in line and any(f"symbol `{function}'" in line for function in functions)]
    assert any("symbol `memcpy'" in line for line in bindings),name
    if name!="glibc": assert any("libaocl-libmem" in line and "symbol `memcpy'" in line for line in bindings),name
    record["runs"].append({"name":name,"command":command,"passed":True,"preload":result.get("server_preload"),
                           "client_aocl_mapping_absent":True,"bindings":bindings,"trace":f"artifacts/cpp-aocl-bindings-{name}.json"})
record["passed"]=True
(ROOT/"artifacts/cpp-aocl-bindings.json").write_text(json.dumps(record,indent=2)+"\n")
print(json.dumps({"passed":True,"effective_binding_cases":len(record["runs"]),"ignored_library_rejected":True}))
