from pathlib import Path
import ctypes,json,subprocess
libc=ctypes.CDLL(None)
libc.malloc.argtypes=[ctypes.c_size_t];libc.malloc.restype=ctypes.c_void_p
libc.malloc_usable_size.argtypes=[ctypes.c_void_p];libc.malloc_usable_size.restype=ctypes.c_size_t
libc.free.argtypes=[ctypes.c_void_p]
rows=[]
for size in [1024,1027,1033]:
 p=libc.malloc(size);rows.append({'requested_bytes':size,'usable_bytes':libc.malloc_usable_size(p)});libc.free(p)
r={'scope':'Local allocator class probe, not a throughput benchmark. Recorded perf libc build ID matches installed libc.','allocation_sizes':rows,'libc_build_id':subprocess.check_output(['readelf','-n','/usr/lib/libc.so.6'],text=True),'profile_build_ids':subprocess.check_output(['perf','buildid-list','-i','artifacts/client-cost-profile.perf.data'],text=True)}
Path('artifacts/client-allocator-evidence.json').write_text(json.dumps(r,indent=2)+'\n')
Path('artifacts/client-allocator-locks.txt').write_text(subprocess.check_output(['objdump','-d','--start-address=0xa62d0','--stop-address=0xa6320','/usr/lib/libc.so.6'],text=True))
