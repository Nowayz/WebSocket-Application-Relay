#!/usr/bin/env python3
"""Admission probes must reject existing companion files before creating evidence."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class OutputTest(unittest.TestCase):
    runner = 'admission-reuse.py'

    def test_existing_receipt_or_log_preserves_evidence_without_launch(self):
        for suffix in ['.json', '.server.log']:
            for dangling in [False, True]:
                with self.subTest(suffix=suffix, dangling=dangling), tempfile.TemporaryDirectory() as temp:
                    folder = Path(temp)
                    output = folder / 'probe.json'
                    artifact = output.with_suffix(suffix)
                    marker = folder / 'started'
                    absent = folder / 'absent'
                    server = folder / 'server'
                    server.write_text('#!/usr/bin/env python3\nfrom pathlib import Path\n'
                                      f'Path({str(marker)!r}).touch()\nraise SystemExit(1)\n')
                    server.chmod(0o755)
                    evidence = b'previous evidence\n'
                    if dangling:
                        artifact.symlink_to(absent)
                    else:
                        artifact.write_bytes(evidence)
                    result = subprocess.run([sys.executable, str(ROOT / 'scripts' / self.runner),
                                             '--server', str(server), '--output', str(output)],
                                            capture_output=True, text=True, timeout=10)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(str(artifact), result.stderr)
                    self.assertFalse(marker.exists())
                    if dangling:
                        self.assertTrue(artifact.is_symlink())
                        self.assertFalse(absent.exists())
                    else:
                        self.assertEqual(artifact.read_bytes(), evidence)
                    if suffix != '.json':
                        self.assertFalse(output.exists(), 'companion collision left a partial receipt')


    def test_failed_live_check_reaps_its_server_and_records_failure(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            output = folder / 'probe.json'
            pid_file = folder / 'pid'
            server = folder / 'server'
            # Stay alive after rejecting the upgrade: only the probe can reap us.
            server.write_text(r"""#!/usr/bin/env python3
import http.server, os, socket, sys, threading
from pathlib import Path
Path(PID_FILE).write_text(str(os.getpid()))
class Metrics(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'war_connections 0\n'
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args): pass
def endpoint(flag):
    host, port = sys.argv[sys.argv.index(flag) + 1].split(':')
    return host, int(port)
admin = http.server.HTTPServer(endpoint('--admin-bind'), Metrics)
threading.Thread(target=admin.serve_forever, daemon=True).start()
with socket.socket() as listener:
    listener.bind(endpoint('--bind'))
    listener.listen()
    stream, _ = listener.accept()
    with stream:
        request = bytearray()
        while not request.endswith(b'\r\n\r\n'):
            request.extend(stream.recv(1))
        stream.sendall(b'HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n')
    threading.Event().wait()
""".replace('PID_FILE', repr(str(pid_file))))
            server.chmod(0o755)
            result = subprocess.run([sys.executable, str(ROOT / 'scripts' / self.runner),
                                     '--server', str(server), '--output', str(output)],
                                    capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            receipt = json.loads(output.read_text())
            self.assertFalse(receipt['passed'])
            self.assertIn('AssertionError', receipt['error'])
            self.assertEqual(receipt['exit_code'], -15)
            with self.assertRaises(ProcessLookupError):
                os.kill(int(pid_file.read_text()), 0)


class OverloadOutputTest(OutputTest):
    runner = 'overload-reuse.py'


if __name__ == '__main__':
    unittest.main()
