#!/usr/bin/env python3
"""Attribute a captured capacity heap to first executable frames in its exact image.

Optimized inline sites can share symbols. This is allocation-site evidence, not
complete call-stack attribution. Bucket bytes must sum to the profile header.
"""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def image_address_at_zero_offset(binary):
    with binary.open('rb') as image:
        header = image.read(64)
        if header[:4] != b'\x7fELF' or header[4] != 2 or header[5] not in (1, 2):
            raise ValueError('allocation-site analysis requires a 64-bit ELF image')
        order = '<' if header[5] == 1 else '>'
        offset = struct.unpack_from(order + 'Q', header, 32)[0]
        size, count = struct.unpack_from(order + 'HH', header, 54)
        for index in range(count):
            image.seek(offset + index * size)
            kind, _, file_offset, address, *_ = struct.unpack(order + 'IIQQQQQQ', image.read(56))
            if kind == 1 and file_offset == 0:
                return address
    raise ValueError('ELF has no zero-offset LOAD segment')


def analyze(receipt, phase):
    data = json.loads(receipt.read_text())
    event = next(event for event in data['events'] if event['event'] == phase)
    heap_info = event['server']['heap']
    heap, binary = Path(heap_info['path']), Path(data['args']['server']).resolve()
    if digest(binary) != data['server_sha256'] or digest(heap) != heap_info['sha256']:
        raise ValueError('heap or executable hash differs from the captured receipt')
    lines = heap.read_text().splitlines()
    maps, load_bias = [], None
    for line in lines:
        fields = line.split(maxsplit=5)
        if len(fields) != 6 or fields[5] != str(binary):
            continue
        start, end = (int(value, 16) for value in fields[0].split('-'))
        if int(fields[2], 16) == 0:
            load_bias = start - image_address_at_zero_offset(binary)
        if 'x' in fields[1]:
            maps.append((start, end))
    if load_bias is None or not maps:
        raise ValueError('profile lacks executable mappings for the captured image')
    sites, unresolved = defaultdict(lambda: [0, 0]), [0, 0]
    for line in lines:
        match = re.match(r'\s*(\d+):\s*(\d+)\s*\[.*?\]\s*@\s*(.*)', line)
        if not match or not int(match[1]):
            continue
        count, size = int(match[1]), int(match[2])
        frames = (int(value, 16) for value in match[3].split())
        address = next((address for address in frames if any(start <= address < end for start, end in maps)), None)
        bucket = sites[address - load_bias] if address is not None else unresolved
        bucket[0] += count
        bucket[1] += size
    ordered = sorted(sites, key=lambda address: sites[address][1], reverse=True)
    symbols = subprocess.run(['addr2line', '-C', '-f', '-e', str(binary)],
                             input=''.join(f'{address:x}\n' for address in ordered),
                             text=True, capture_output=True, check=True).stdout.splitlines()
    if len(symbols) != 2 * len(ordered):
        raise RuntimeError('unexpected addr2line output shape')
    rows = [{'offset': hex(address), 'objects': sites[address][0], 'bytes': sites[address][1],
             'mean_bytes': sites[address][1] / sites[address][0],
             'function': symbols[2 * index], 'location': symbols[2 * index + 1]}
            for index, address in enumerate(ordered)]
    expected = int(re.search(r':\s*(\d+)\s*\[', lines[0])[1])
    if sum(row['bytes'] for row in rows) + unresolved[1] != expected:
        raise ValueError('allocation buckets do not match the profile header')
    return {'scope': __doc__, 'receipt': str(receipt), 'phase': phase, 'heap': str(heap),
            'heap_sha256': heap_info['sha256'], 'server_sha256': data['server_sha256'],
            'analyzer_sha256': digest(Path(__file__)), 'requested_live_bytes': expected,
            'unresolved_objects': unresolved[0], 'unresolved_bytes': unresolved[1], 'sites': rows}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--receipt', type=Path, required=True)
    parser.add_argument('--phase', default='ready')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    result = analyze(args.receipt, args.phase)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x') as output:
        output.write(json.dumps(result, indent=2) + '\n')
