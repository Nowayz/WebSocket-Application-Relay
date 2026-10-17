#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
dep="${WAR_UWS_SOURCE:-artifacts/modern-uwebsockets}"
if [[ ! -f "$dep/src/App.h" || ! -f "$dep/uSockets/src/libusockets.h" ]]; then
  echo 'Run cpp/setup.sh to fetch the pinned uWebSockets and uSockets dependencies.' >&2
  exit 1
fi
git -C "$dep" apply --reverse --check "$(pwd)/cpp/patches/war-transport.patch"
make -C "$dep/uSockets" WITH_LTO=0 CFLAGS='-O3 -march=native' > artifacts/cpp-usockets-build.log 2>&1
mkdir -p artifacts/candidates/cpp-modern
${CXX:-clang++} -std=c++20 -O3 -march=native -DNDEBUG -pthread -flto=thin \
  -DUWS_NO_ZLIB -DLIBUS_NO_SSL -Wall -Wextra -Wpedantic \
  -isystem "$dep/src" -isystem "$dep/uSockets/src" \
  -c cpp/relay.cpp -o artifacts/candidates/cpp-modern/relay.o
llvm-ar rcs artifacts/candidates/cpp-modern/libwar-relay.a artifacts/candidates/cpp-modern/relay.o
${CXX:-clang++} -std=c++20 -O3 -march=native -DNDEBUG -pthread -flto=thin -fuse-ld=lld \
  cpp/main.cpp artifacts/candidates/cpp-modern/libwar-relay.a "$dep/uSockets/uSockets.a" \
  -o artifacts/candidates/cpp-modern/war-relay-cpp
${CXX:-g++} -std=c++20 -O2 -Wall -Wextra -Werror cpp/tests/cbor.cpp -o artifacts/candidates/cpp-modern/cbor-test
artifacts/candidates/cpp-modern/cbor-test
python3 - <<'PY'
import hashlib,json,subprocess,pathlib
root=pathlib.Path('.')
dep=pathlib.Path('artifacts/modern-uwebsockets')
files=['cpp/relay.cpp','cpp/relay.hpp','cpp/main.cpp','cpp/cbor.hpp','cpp/build.sh','cpp/tests/cbor.cpp','cpp/patches/war-transport.patch']
binary='artifacts/candidates/cpp-modern/war-relay-cpp'
data={'compiler':subprocess.check_output(['clang++','--version'],text=True).splitlines()[0], 'uws_revision':subprocess.check_output(['git','-C',str(dep),'rev-parse','HEAD'],text=True).strip(),'usockets_revision':subprocess.check_output(['git','-C',str(dep/'uSockets'),'rev-parse','HEAD'],text=True).strip(),'source_sha256':{p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest() for p in files},'binary_sha256':hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),'profile':'C++20 O3 march=native LTO, plaintext raw epoll, compression disabled'}
pathlib.Path('artifacts/candidates/cpp-modern/build.json').write_text(json.dumps(data,indent=2)+'\n')
PY
