"""WebTransport benchmark setup and verified localhost certificate pins."""
import os
import re


def prepare(args, server_command, environment, port):
    server_command += ['--dev-cert', '--udp-receive-buffer-bytes', str(getattr(args, 'udp_receive_buffer_bytes', 4 * 1024 * 1024))]
    environment['RUST_LOG'] = 'war_relay=info'
    return f'https://127.0.0.1:{port}/war'


def certificate(logs):
    # pread does not move the descriptor offset shared with the server logger.
    text = os.pread(logs.fileno(), os.fstat(logs.fileno()).st_size, 0).decode()
    text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
    match = re.search(r'certificate_sha256=([0-9a-fA-F:]{95})', text)
    if match is None:
        raise RuntimeError('server did not report its development certificate pin')
    return ['--certificate', match[1]]
