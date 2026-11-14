#!/usr/bin/env python3
"""CPU snapshots must preserve stat field positions across Linux comm names."""
import math
from pathlib import Path
import unittest
from unittest.mock import patch

import benchmark


def stat(name):
    fields = ['S'] + ['0'] * 49
    fields[11:15] = ['250', '125', '9000', '8000']
    return f'123 ({name}) ' + ' '.join(fields) + '\n'


def read_fixture(path, *_args, **_kwargs):
    if str(path).endswith('/status'):
        return 'VmRSS:\t1024 kB\nVmSize:\t4096 kB\nRssAnon:\t512 kB\nThreads:\t1\n'
    if str(path).endswith('/stat'):
        return stat(read_fixture.name)
    raise AssertionError(path)


def directories(path):
    if str(path).endswith('/task'):
        return iter([Path('/proc/123/task/124')])
    if str(path).endswith('/fd'):
        return iter([])
    raise AssertionError(path)


class CpuAccountingTest(unittest.TestCase):
    def capture(self, capture, name, ticks=100):
        read_fixture.name = name
        with patch.object(Path, 'read_text', read_fixture), \
             patch.object(Path, 'iterdir', directories), \
             patch.object(benchmark.os, 'sysconf', return_value=ticks):
            return capture(123)

    def test_process_and_thread_totals_ignore_spaces_and_parentheses_in_comm(self):
        for name in ['relay', 'relay worker', 'relay ) worker', 'relay (a) )b']:
            with self.subTest(name=name):
                result = self.capture(benchmark.snapshot, name)
                self.assertEqual(result['cpu_seconds'], 3.75)
                self.assertEqual(result['thread_cpu_seconds'], {'124': 3.75})

    def test_user_and_system_times_exclude_children_and_use_clock_ticks(self):
        for ticks in [100, 250, 1000]:
            with self.subTest(ticks=ticks):
                result = self.capture(benchmark.snapshot, 'relay', ticks)
                self.assertEqual(result['user_cpu_seconds'], 250 / ticks)
                self.assertEqual(result['system_cpu_seconds'], 125 / ticks)
                self.assertEqual(result['cpu_seconds'], 375 / ticks)
                self.assertTrue(math.isclose(result['cpu_seconds'],
                    result['user_cpu_seconds'] + result['system_cpu_seconds']))


if __name__ == '__main__':
    unittest.main()
