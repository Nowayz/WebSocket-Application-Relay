#!/usr/bin/env python3
"""Independent controller-clock fixtures preserve backlog and reject bad evidence."""
import copy
import importlib.util
from pathlib import Path
import unittest
import os
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('mixed', Path(__file__).with_name('mixed-capacity.py'))
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)


def fixture():
    fields = ['scheduled_ns', 'wake_ns', 'sent_ns', 'acknowledged_ns', 'completed_ns']
    return {'ready_ns': 1_000_000, 'windows': [dict(zip(fields, [t*1_000_000 for t in row]))
            for row in [[1, 3, 5, 11, 12], [10, 12, 13, 16, 18], [20, 21, 22, 32, 34]]]}


class TimingTest(unittest.TestCase):
    def test_decomposition_preserves_publisher_backlog_and_worst_window(self):
        source = fixture()
        original = copy.deepcopy(source)
        result = mixed.fanout_timing_summary(source)
        self.assertEqual(source, original)
        self.assertEqual(result['windows'], 3)
        self.assertEqual(result['controller_ready_ms'], 1)
        self.assertEqual(result['stages']['after_last_send_to_all_acks'],
                         {'first_ms': 6, 'median_ms': 6, 'max_ms': 10, 'max_window_index': 2})
        self.assertEqual(result['stages']['publisher_work']['max_ms'], 2)
        self.assertEqual(result['stages']['wake_lag']['first_ms'], 2)
        self.assertEqual(result['stages']['scheduled_to_complete']['max_ms'], 14)
        self.assertEqual(result['stages']['membership_cleanup']['median_ms'], 2)

    def test_readiness_may_follow_the_first_scheduled_tick(self):
        timing = fixture()
        timing['ready_ns'] = 2_000_000
        self.assertTrue(mixed.fanout_timing_summary(timing)['available'])

    def test_bad_clocks_are_rejected_instead_of_turned_into_valid_latency(self):
        for field in ['scheduled_ns', 'wake_ns', 'sent_ns', 'acknowledged_ns', 'completed_ns']:
            for value in [-1, 1.5, True]:
                with self.subTest(field=field, value=value):
                    timing = fixture()
                    timing['windows'][0][field] = value
                    with self.assertRaises(ValueError): mixed.fanout_timing_summary(timing)
        for change in ['inverted', 'overlapping', 'empty', 'late-ready']:
            with self.subTest(change=change):
                timing = fixture()
                if change == 'inverted': timing['windows'][0]['sent_ns'] = 2_000_000
                if change == 'overlapping': timing['windows'][1]['wake_ns'] = 11_000_000
                if change == 'empty': timing['windows'] = []
                if change == 'late-ready': timing['ready_ns'] = 4_000_000
                with self.assertRaises(ValueError): mixed.fanout_timing_summary(timing)

    def test_older_or_disabled_timing_is_explicitly_unavailable(self):
        self.assertFalse(mixed.fanout_timing_summary(None)['available'])


class SnapshotTest(unittest.TestCase):
    def test_timed_sample_never_enumerates_descriptors(self):
        with patch.object(Path, 'iterdir', side_effect=AssertionError('unexpected FD scan')):
            sample = mixed.snapshot(os.getpid(), include_fds=False)
        self.assertIsNone(sample['fds'])
        self.assertGreater(sample['rss_kib'], 0)

    def test_default_gate_still_counts_descriptors(self):
        before = mixed.snapshot(os.getpid())['fds']
        with open('/dev/null'):
            self.assertEqual(mixed.snapshot(os.getpid())['fds'], before + 1)


if __name__ == '__main__':
    unittest.main()
