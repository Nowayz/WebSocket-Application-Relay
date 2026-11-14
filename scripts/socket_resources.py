#!/usr/bin/env python3
"""Read Linux socket accounting through SOCK_DIAG, independently of process RSS.

The UAPI layouts are linux/inet_diag.h and linux/sock_diag.h. Buffer limits are
not allocations, and skmem counters are not all disjoint: report each separately.
Slab object estimates exclude allocator/slab overhead and other socket resources.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import struct
import time

SKMEM_NAMES = ('rmem_alloc_bytes', 'receive_buffer_limit_bytes', 'wmem_alloc_bytes',
               'send_buffer_limit_bytes', 'forward_alloc_bytes', 'wmem_queued_bytes',
               'optmem_bytes', 'backlog_bytes', 'drops')
PROTOCOLS = {'tcp': socket.IPPROTO_TCP, 'udp': socket.IPPROTO_UDP}
SLABS = {(socket.IPPROTO_TCP, socket.AF_INET): 'TCP',
         (socket.IPPROTO_TCP, socket.AF_INET6): 'TCPv6',
         (socket.IPPROTO_UDP, socket.AF_INET): 'UDP',
         (socket.IPPROTO_UDP, socket.AF_INET6): 'UDPv6'}


def slab_sizes():
    result = {}
    for name in SLABS.values():
        path = Path('/sys/kernel/slab') / name / 'object_size'
        try:
            result[name] = {'object_size_bytes': int(path.read_text()), 'source': str(path)}
        except OSError as error:
            result[name] = {'unavailable': str(error), 'source': str(path)}
    return result


def owned_inodes(pid):
    result = set()
    for fd in Path(f'/proc/{pid}/fd').iterdir():
        try:
            target = os.readlink(fd)
        except FileNotFoundError:
            continue
        if target.startswith('socket:['):
            result.add(int(target[8:-1]))
    return result


def _messages(data):
    offset = 0
    while offset < len(data):
        if len(data) - offset < 16:
            raise ValueError('truncated netlink header')
        size, kind, flags, sequence, sender = struct.unpack_from('=IHHII', data, offset)
        if size < 16 or offset + size > len(data):
            raise ValueError('invalid netlink message length')
        yield kind, flags, sequence, sender, data[offset+16:offset+size]
        offset += (size + 3) & ~3


def _attributes(data):
    offset = 0
    while offset < len(data):
        if len(data) - offset < 4:
            raise ValueError('truncated inet_diag attribute')
        size, kind = struct.unpack_from('=HH', data, offset)
        if size < 4 or offset + size > len(data):
            raise ValueError('invalid inet_diag attribute length')
        yield kind & 0x3fff, data[offset+4:offset+size]
        offset += (size + 3) & ~3


def dump(protocol, family):
    """One complete dump; an interrupted dump fails instead of reporting a subset."""
    # Request INET_DIAG_SKMEMINFO (attribute 7) and all connection states.
    request = struct.pack('=BBBBI', family, protocol, 1 << 6, 0, 0xffffffff)
    request += bytes(40) + struct.pack('=II', 0xffffffff, 0xffffffff)
    sequence = 1
    packet = struct.pack('=IHHII', 16 + len(request), 20, 0x301, sequence, 0) + request
    with socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 4) as diag:
        diag.bind((0, 0))
        diag.settimeout(10)  # Diagnostic failure deadline, not a population/byte cap.
        diag.sendto(packet, (0, 0))
        while True:
            # Obtain the actual datagram length; never silently truncate a dump.
            size, _, _, _ = diag.recvmsg_into([bytearray()], 0, socket.MSG_PEEK | socket.MSG_TRUNC)
            data, _, flags, address = diag.recvmsg(size)
            if flags & socket.MSG_TRUNC or address[0] != 0:
                raise RuntimeError('truncated or non-kernel SOCK_DIAG datagram')
            for kind, flags, seq, sender, payload in _messages(data):
                if seq != sequence:
                    raise RuntimeError('SOCK_DIAG sequence mismatch')
                if flags & 0x10:  # NLM_F_DUMP_INTR
                    raise RuntimeError('SOCK_DIAG dump interrupted; no complete observation')
                if kind == 3:  # NLMSG_DONE may include a negative errno.
                    if len(payload) >= 4 and (error := struct.unpack_from('=i', payload)[0]):
                        raise OSError(-error, os.strerror(-error))
                    return
                if kind == 2:  # NLMSG_ERROR
                    error = struct.unpack_from('=i', payload)[0]
                    if error:
                        raise OSError(-error, os.strerror(-error))
                    continue
                if kind != 20 or len(payload) < 72:
                    raise ValueError('unexpected or short inet_diag response')
                local_port, remote_port = struct.unpack_from('!HH', payload, 4)
                row = {'family': payload[0], 'state': payload[1], 'local_port': local_port,
                       'remote_port': remote_port, 'inode': struct.unpack_from('=I', payload, 68)[0],
                       'receive_queue': struct.unpack_from('=I', payload, 56)[0],
                       'send_queue': struct.unpack_from('=I', payload, 60)[0]}
                for attribute, value in _attributes(payload[72:]):
                    if attribute == 7:
                        if len(value) < 4*len(SKMEM_NAMES) or len(value) % 4:
                            raise ValueError('short or misaligned INET_DIAG_SKMEMINFO')
                        row['skmem'] = dict(zip(SKMEM_NAMES, struct.unpack_from('=9I', value)))
                yield row


def snapshot(pid, protocol, local_port=None, remote_port=None, sizes=None):
    start = time.monotonic()
    namespace = os.readlink(f'/proc/{pid}/ns/net')
    if namespace != os.readlink('/proc/self/ns/net'):
        raise ValueError('SOCK_DIAG target must share the current network namespace')
    inodes = owned_inodes(pid)
    sizes = slab_sizes() if sizes is None else sizes
    result = {'pid': pid, 'protocol': protocol, 'local_port': local_port, 'remote_port': remote_port,
              'owned_sockets': 0, 'missing_skmem_sockets': 0, 'families': {},
              'skmem': dict.fromkeys(SKMEM_NAMES, 0),
              'network_namespace': namespace,
              'port_matched_time_wait': 0 if local_port is not None or remote_port is not None else None,
              'socket_object_estimate_bytes': 0, 'socket_object_estimate_unavailable': 0,
              'slab_sizes': sizes}
    seen = set()
    for family in (socket.AF_INET, socket.AF_INET6):
        for row in dump(PROTOCOLS[protocol], family):
            if local_port is not None and row['local_port'] != local_port:
                continue
            if remote_port is not None and row['remote_port'] != remote_port:
                continue
            if protocol == 'tcp' and row['state'] == 6 and result['port_matched_time_wait'] is not None:
                # TIME_WAIT has no live process fd. Port matches are reported
                # separately, never attributed to current process-owned sockets.
                result['port_matched_time_wait'] += 1
            if row['inode'] not in inodes or (protocol == 'tcp' and row['state'] != 1):
                continue
            if row['inode'] in seen:
                raise RuntimeError('duplicate owned inode in SOCK_DIAG dump')
            seen.add(row['inode'])
            result['owned_sockets'] += 1
            name = SLABS[(PROTOCOLS[protocol], family)]
            result['families'][name] = result['families'].get(name, 0) + 1
            size = sizes[name].get('object_size_bytes')
            if size is not None and (type(size) is not int or size <= 0):
                raise ValueError('slab object size must be a positive integer')
            if size is None:
                result['socket_object_estimate_unavailable'] += 1
            else:
                result['socket_object_estimate_bytes'] += size
            if 'skmem' not in row:
                result['missing_skmem_sockets'] += 1
            else:
                for key, value in row['skmem'].items():
                    result['skmem'][key] += value
    result['elapsed_seconds'] = time.monotonic() - start
    result['scope'] = ('Process-owned connected TCP sockets or bound UDP sockets, filtered by the requested '
                       'ports. skmem fields are separate counters, not an additive total. Buffer limits '
                       'are not allocations. Slab object estimate excludes slab overhead, protocol buffers, '
                       'route/conntrack state and other kernel resources. TIME_WAIT port matches have no '
                       'live fd and are reported separately. This is not complete kernel memory accounting.')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pid', type=int)
    parser.add_argument('--protocol', choices=PROTOCOLS)
    parser.add_argument('--local-port', type=int)
    parser.add_argument('--remote-port', type=int)
    parser.add_argument('--slab-sizes-only', action='store_true')
    parser.add_argument('--slab-sizes', type=Path)
    args = parser.parse_args()
    if args.slab_sizes_only:
        result = slab_sizes()
    else:
        if args.pid is None or args.protocol is None:
            parser.error('--pid and --protocol are required for a socket snapshot')
        sizes = json.loads(args.slab_sizes.read_text()) if args.slab_sizes else None
        result = snapshot(args.pid, args.protocol, args.local_port, args.remote_port, sizes)
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
