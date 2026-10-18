#!/usr/bin/env python3
"""Real Linux TCP alias regression: a foreign source endpoint is not a relay user."""
import importlib.util
import multiprocessing
import os
from pathlib import Path
import socket
import unittest

spec = importlib.util.spec_from_file_location('capacity', Path(__file__).with_name('capacity.py'))
capacity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capacity)


def hold_server(pipe):
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        listener.bind(('127.0.0.1', 0))
        listener.listen()
        listener.settimeout(5)
        pipe.send(listener.getsockname()[1])
        accepted = duplicate = None
        try:
            while (command := pipe.recv()) != 'stop':
                if command == 'accept':
                    accepted, _ = listener.accept()
                    duplicate = socket.socket(fileno=os.dup(accepted.fileno()))
                elif command == 'close':
                    duplicate.close()
                    accepted.close()
                pipe.send('done')
        finally:
            if duplicate is not None:
                duplicate.close()
            if accepted is not None:
                accepted.close()
            pipe.close()


class CensusTest(unittest.TestCase):
    def test_foreign_source_port_alias_and_duplicate_descriptors(self):
        context = multiprocessing.get_context('spawn')
        parent, child = context.Pipe()
        server = context.Process(target=hold_server, args=(child,))
        server.start()
        child.close()
        try:
            self.assertTrue(parent.poll(5), 'server setup')
            port = parent.recv()
            with socket.socket() as other, socket.socket() as foreign:
                other.bind(('127.0.0.2', 0))
                other.listen()
                other.settimeout(5)
                foreign.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                foreign.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
                foreign.bind(('127.0.0.1', port))
                foreign.settimeout(5)
                foreign.connect(other.getsockname())
                peer, _ = other.accept()
                with peer:
                    def census():
                        owned = set()
                        snapshot = capacity.process(server.pid, owned)
                        return capacity.established(port, owned), snapshot, owned

                    observed, _, _ = census()
                    self.assertEqual(observed['raw_matching_rows'], 1)
                    self.assertEqual(observed['foreign_matching_inodes'], 1)
                    self.assertEqual(observed['owned_established'], 0)
                    parent.send('accept')
                    with socket.create_connection(('127.0.0.1', port), timeout=5):
                        self.assertTrue(parent.poll(5), 'server accept')
                        self.assertEqual(parent.recv(), 'done')
                        observed, snapshot, owned = census()
                        self.assertEqual(observed['raw_matching_rows'], 2)
                        self.assertEqual(observed['owned_established'], 1)
                        self.assertEqual(observed['foreign_matching_inodes'], 1)
                        self.assertGreater(snapshot['socket_fd_count'], len(owned))
                    parent.send('close')
                    self.assertTrue(parent.poll(5), 'server close')
                    self.assertEqual(parent.recv(), 'done')
                    observed, _, _ = census()
                    self.assertEqual(observed['raw_matching_rows'], 1)
                    self.assertEqual(observed['owned_established'], 0)
                    self.assertEqual(observed['foreign_matching_inodes'], 1)
        finally:
            if server.is_alive():
                parent.send('stop')
            server.join(5)
            if server.is_alive():
                server.terminate()
                server.join(5)
            parent.close()
        self.assertEqual(server.exitcode, 0)


if __name__ == '__main__':
    unittest.main()
