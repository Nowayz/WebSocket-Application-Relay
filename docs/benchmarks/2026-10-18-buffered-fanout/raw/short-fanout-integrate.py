from pathlib import Path
import hashlib,json
base=Path('artifacts/rust-fanout-ready64-batch-work')
selected=(base/'src/relay.rs').read_text();root=Path('artifacts/short-fanout-preexisting/src/relay.rs').read_text()
a=selected.index('    /// Stage only a validated ordinary-room multicast');b=selected.index('    pub(crate) fn publish(',a)
method=selected[a:b].replace('!state.listeners(CHANNEL_MESSAGE).is_empty()','state.listener_count(CHANNEL_MESSAGE) != 0')
root=root.replace('    pub(crate) fn publish(',method+'    pub(crate) fn publish(',1)
a=root.index('/// One immutable membership snapshot');b=root.index('fn deliver_room(',a)
start=selected.index('/// One immutable membership view');end=selected.index('fn deliver_room(',start)
root=root[:a]+selected[start:end]+root[b:]
a=selected.index('/// A bounded lookahead reservation');b=selected.index('/// A charged synchronous submission',a)
root=root.replace('/// A charged synchronous submission',selected[a:b]+'/// A charged synchronous submission',1)
a=selected.index('impl Peer {');b=selected.index('    pub(crate) fn close(',a)
root=root.replace('impl Peer {\n',selected[a:b],1)
Path('src/relay.rs').write_text(root)
for name in ['src/server.rs','src/server/fairness_tests.rs','src/relay/continuation_tests.rs','vendor/tungstenite/src/protocol/mod.rs','vendor/tungstenite/src/protocol/frame/mod.rs']:
 Path(name).write_bytes((base/name).read_bytes())
old=json.loads(Path('artifacts/short-fanout-preexisting.json').read_text())
for name in ['cpp/relay.cpp','src/relay/listener_tests.rs']:
 assert hashlib.sha256(Path(name).read_bytes()).hexdigest()==old[name],name
