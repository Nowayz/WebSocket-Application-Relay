#!/usr/bin/env python3
"""Independent tiny fixtures protect percentile semantics and invalid-input handling."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('comparison', Path(__file__).with_name('plot-user-latency-comparison.py'))
comparison = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparison)


class ComparisonTest(unittest.TestCase):
    def fixture(self, body):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'users.csv'
            path.write_text('id,samples,p99_ms,max_ms,schedule_p99_ms,schedule_max_ms\n' + body)
            return comparison.load(path)

    def test_nearest_rank_and_separate_backlog(self):
        values, samples, maxima = self.fixture('1,10,1,2,4,5\n2,10,2,3,8,9\n3,10,100,101,1000,1001\n')
        result = comparison.summarize(values, samples, maxima, [2])
        self.assertEqual(result['total_packet_samples'], 30)
        actual = result['metrics']['send_to_receive']
        self.assertEqual(actual['user_p99_ms']['median'], 2)
        self.assertEqual(actual['user_p99_ms']['p99'], 100)
        self.assertAlmostEqual(actual['users_with_p99_at_or_below_ms_percent']['2'], 200 / 3)
        self.assertEqual(result['metrics']['intended_send_to_receive']['user_p99_ms']['median'], 8)

    def test_histogram_bounds_are_distinct_and_may_exceed_observed_maximum(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'bounded.csv'
            path.write_text('id,samples,p99_upper_ms,max_ms,schedule_p99_upper_ms,schedule_max_ms\n1,4,2,1.5,4,3\n')
            values, samples, maxima = comparison.load(path, histogram_bounds=True)
            result = comparison.summarize(values, samples, maxima, [2], histogram_bounds=True)
            metric = result['metrics']['send_to_receive']
            self.assertEqual(metric['user_p99_upper_ms']['p99'], 2)
            self.assertNotIn('user_p99_ms', metric)
            self.assertEqual(metric['users_with_p99_upper_bound_at_or_below_ms_percent']['2'], 100)
            with self.assertRaises(KeyError):
                comparison.load(path)

    def test_reject_empty_duplicate_or_invalid(self):
        for body in ['', '1,10,1,2,4,5\n1,10,1,2,4,5\n',
                     '1,0,1,2,4,5\n', '1,10,nan,2,4,5\n', '1,10,3,2,4,5\n']:
            with self.subTest(body=body), self.assertRaises(ValueError):
                self.fixture(body)


if __name__ == '__main__':
    unittest.main()
