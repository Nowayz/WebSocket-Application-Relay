#!/usr/bin/env python3
import hashlib
import json
from pathlib import Path
from analyze import summarize
ROOT = Path(__file__).resolve().parent
for name, expected in json.loads((ROOT / 'sha256.json').read_text()).items():
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
for name in ['small.json', '100k-first.json', '100k-repeat.json']:
    summarize(ROOT / name)
print('PASS: evidence hashes, fixture identity, exact population/queue/copy gates, per-witness event samples')
