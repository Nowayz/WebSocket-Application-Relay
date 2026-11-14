#!/usr/bin/env python3
"""Verify SOCK_DIAG against real queued sockets and the independent ss decoder."""
import os
import re
import shutil
import socket
import struct
import subprocess
import sys
import unittest

import socket_resources as resources


@unittest.skipUnless(sys.platform == 'linux', 'Linux SOCK_DIAG UAPI')
class SocketResources(unittest.TestCase):
    def test_udp_allocation_matches_ss_and_socket_buffer_limit(self):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as receiver, \
             socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sender:
            receiver.bind(('127.0.0.1', 0))
            sender.sendto(b'x' * 1024, receiver.getsockname())
            observed = resources.snapshot(os.getpid(), receiver.getsockname()[1])
            self.assertEqual(observed['owned_sockets'], 1)
            self.assertEqual(observed['missing_skmem_sockets'], 0)
            self.assertGreaterEqual(observed['skmem']['rmem_alloc_bytes'], 1024)
            self.assertEqual(observed['skmem']['receive_buffer_limit_bytes'],
                             receiver.getsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF))
            if shutil.which('ss'):
                text = subprocess.check_output(['ss', '-m', '-u', '-a', '-n',
                                                f'sport = :{receiver.getsockname()[1]}'], text=True)
                fields = re.search(r'skmem:\(([^)]+)\)', text).group(1).split(',')
                actual = dict(re.fullmatch(r'([a-z]+)(\d+)', field).groups() for field in fields)
                self.assertEqual(int(actual['r']), observed['skmem']['rmem_alloc_bytes'])
                self.assertEqual(int(actual['rb']), observed['skmem']['receive_buffer_limit_bytes'])
            self.assertEqual(receiver.recv(2048), b'x' * 1024)

    def test_truncated_netlink_and_attribute_data_fail_instead_of_undercounting(self):
        for data in [b'x', struct.pack('=IHHII', 20, 20, 0, 1, 0)]:
            with self.assertRaises(ValueError):
                list(resources._messages(data))
        for data in [b'x', struct.pack('=HH', 3, 7), struct.pack('=HH', 8, 7)]:
            with self.assertRaises(ValueError):
                list(resources._attributes(data))


if __name__ == '__main__':
    unittest.main()
