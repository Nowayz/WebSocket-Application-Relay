#!/usr/bin/env python3
"""Independent tiny fixtures protect percentile semantics and invalid-input handling."""
import importlib.util
from pathlib import Path
import subprocess
import sys
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

    def test_observed_maxima_expose_stalls_hidden_by_per_user_p99(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'users.csv'
            path.write_text('id,samples,p99_ms,max_ms,schedule_p99_ms,schedule_max_ms\n'
                            '1,1000,1,2,2,4\n2,1000,1,100,2,200\n')
            values, samples, maxima = comparison.load(path)
            p99 = comparison.summarize(values, samples, maxima, [2])
            self.assertEqual(p99['metrics']['send_to_receive']['users_with_p99_at_or_below_ms_percent']['2'], 100)
            values, samples, maxima = comparison.load(path, statistic='max')
            result = comparison.summarize(values, samples, maxima, [2], statistic='max')
            actual = result['metrics']['send_to_receive']
            self.assertNotIn('user_p99_ms', actual)
            self.assertEqual(actual['user_max_ms']['maximum'], 100)
            self.assertEqual(actual['users_with_max_at_or_below_ms_percent']['2'], 50)
            self.assertEqual(result['metrics']['intended_send_to_receive']['user_max_ms']['maximum'], 200)

    def test_nanosecond_fanout_columns_normalize_both_statistics_to_milliseconds(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'fanout.csv'
            path.write_text('id,samples,p99_ns,max_ns,schedule_p99_ns,schedule_max_ns\n'
                            '1,384,1000001,2000001,3000001,4000001\n')
            for statistic, expected in [('p99', 1.000001), ('max', 2.000001)]:
                values, samples, maxima = comparison.load(path, statistic=statistic)
                self.assertEqual(values['send_to_receive'], [expected])
                self.assertEqual(maxima['intended_send_to_receive'], 4.000001)
                self.assertEqual(samples, [384])

    def test_maximum_mode_rejects_a_histogram_percentile_label(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'plot'
            result = subprocess.run([sys.executable, str(Path(comparison.__file__)),
                                     '--case', 'fixture=unused.csv', '--output', str(output),
                                     '--statistic', 'max', '--histogram-bounds'],
                                    capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('observed maxima are exact', result.stderr)
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_reject_empty_duplicate_or_invalid(self):
        for body in ['', '1,10,1,2,4,5\n1,10,1,2,4,5\n',
                     '1,0,1,2,4,5\n', '1,10,nan,2,4,5\n', '1,10,3,2,4,5\n']:
            with self.subTest(body=body), self.assertRaises(ValueError):
                self.fixture(body)

    def test_reject_inverted_percentiles_or_maxima_between_clocks(self):
        for body in ['1,10,2,3,1,4\n', '1,10,1,5,2,4\n']:
            with self.subTest(body=body), self.assertRaises(ValueError):
                self.fixture(body)

    def test_histogram_clock_order_remains_required_when_bounds_exceed_maxima(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'bounded.csv'
            for body in ['1,4,2,1.5,1,3\n', '1,4,2,1.5,4,1\n']:
                path.write_text('id,samples,p99_upper_ms,max_ms,schedule_p99_upper_ms,schedule_max_ms\n' + body)
                with self.subTest(body=body), self.assertRaises(ValueError):
                    comparison.load(path, histogram_bounds=True)

    def test_maximum_mode_rejects_inverted_nanosecond_clocks(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'fanout.csv'
            path.write_text('id,samples,p99_ns,max_ns,schedule_p99_ns,schedule_max_ns\n'
                            '1,384,1000000,5000000,2000000,4000000\n')
            with self.assertRaises(ValueError):
                comparison.load(path, statistic='max')


if __name__ == '__main__':
    unittest.main()
