#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('counters', Path(__file__).with_name('analyze-perf-counters.py'))
counters = importlib.util.module_from_spec(spec)
spec.loader.exec_module(counters)


def fixture(values):
    return [f'worker-name-12;{value};{"msec" if event=="task-clock" else ""};{event};100;100.00;;\n'
            for event, value in zip(counters.EVENTS, values)]


class CounterTest(unittest.TestCase):
    capture = {'profile_threads': [{'tid': 12, 'role': 'server', 'name': 'worker-name'}]}

    def test_ratios_and_thread_attribution(self):
        result = counters.analyze(self.capture, fixture([1000, 2e9, 4e9, 4e6, 20, 0]))
        group = result['groups']['server']
        self.assertEqual(group['observed_ipc'], 2)
        self.assertEqual(group['observed_cycles_per_task_clock_ghz'], 2)
        self.assertEqual(group['observed_cache_misses_per_1000_instructions'], 1)
        self.assertTrue(group['ratios_complete_for_all_threads'])

    def test_unavailable_stays_explicit(self):
        group = counters.analyze(self.capture, fixture(['<not counted>'] * 6))['groups']['server']
        self.assertEqual(len(group['unavailable']), 6)
        self.assertIsNone(group['counted_totals']['cycles'])
        self.assertIsNone(group['observed_ipc'])
        self.assertFalse(group['ratios_complete_for_all_threads'])

    def test_cpu_set_and_process_accounting_stay_separate(self):
        run = dict(self.capture,
                   counter_cpu_start={'ticks_per_second': 100, 'cpus': {'cpu0': [0] * 8}},
                   counter_cpu_end={'ticks_per_second': 100, 'cpus': {'cpu0': [100, 0, 200, 0, 0, 30, 40, 0]}},
                   counter_cpu_masks={'server': [0]}, measurement={'server_cpu_seconds': 2.5})
        result = counters.analyze(run, fixture([1000, 2e9, 4e9, 4e6, 20, 0]))['cpu_accounting']['server']
        self.assertEqual(result['process_cpu_gate_seconds'], 2.5)
        self.assertEqual(result['perf_task_clock_seconds'], 1)
        self.assertEqual(result['cpu_set_busy_seconds'], 3.7)
        self.assertEqual(result['cpu_set_softirq_seconds'], .4)

    def test_malformed_duplicate_missing_and_unattributed_fail(self):
        good = fixture([1] * 6)
        for rows in [good[:-1], good + good[:1], ['incomplete\n'], [r.replace('-12;', '-13;') for r in good]]:
            with self.subTest(rows=rows), self.assertRaises(ValueError):
                counters.analyze(self.capture, rows)


if __name__ == '__main__':
    unittest.main()
