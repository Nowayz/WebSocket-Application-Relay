#!/usr/bin/env python3
"""Compare allocated ELF64 sections, permitting GNU build-ID bytes to differ."""
import argparse
import hashlib
import json
from pathlib import Path
import struct


def sections(path):
    data = path.read_bytes()
    assert data[:6] == b'\x7fELF\x02\x01', 'expected little-endian ELF64'
    offset = struct.unpack_from('<Q', data, 40)[0]
    size, count, names = struct.unpack_from('<HHH', data, 58)
    headers = [struct.unpack_from('<IIQQQQIIQQ', data, offset+i*size) for i in range(count)]
    header = headers[names]
    strings = data[header[4]:header[4]+header[5]]
    result = {}
    for h in headers:
        if not h[2] & 2:
            continue
        name = strings[h[0]:].split(b'\0', 1)[0].decode()
        result[name] = {'type': h[1], 'flags': h[2], 'address': h[3], 'offset': h[4],
                        'size': h[5], 'alignment': h[8],
                        'sha256': None if h[1] == 8 else hashlib.sha256(data[h[4]:h[4]+h[5]]).hexdigest()}
    return result


def compare(first, second):
    a, b = sections(first), sections(second)
    assert a.keys() == b.keys()
    for name in a:
        for key in a[name]:
            if name == '.note.gnu.build-id' and key == 'sha256':
                continue
            assert a[name][key] == b[name][key], (name, key)
    return {'passed': True, 'measured_sha256': hashlib.sha256(first.read_bytes()).hexdigest(),
            'selected_sha256': hashlib.sha256(second.read_bytes()).hexdigest(),
            'measured': a, 'selected': b}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('measured', type=Path)
    parser.add_argument('selected', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    with args.output.open('x') as output:
        json.dump(compare(args.measured, args.selected), output, indent=2)
        output.write('\n')
    print('PASS: allocated sections including code/data/layout identical except GNU build ID')
