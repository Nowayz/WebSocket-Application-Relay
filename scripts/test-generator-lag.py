#!/usr/bin/env python3
"""Sender backlog summaries reject invalid clocks instead of reporting healthy load."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('mixed', Path(__file__).with_name('mixed-capacity.py'))
mixed = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mixed)


def timeline(*delays):
    return {'windows': [{'intended_second': second, 'pre_send_delay': {'max_ms': delay}}
                        for second, delay in enumerate(delays)]}


class GeneratorLagTest(unittest.TestCase):
    def test_period_boundary_and_later_backlog_are_preserved(self):
        self.assertEqual(mixed.generator_lag(timeline(0, 250), 4), {
            'available': True, 'period_ms': 250, 'maximum_pre_send_ms': 250,
            'windows_over_one_period': [], 'exceeded_one_period': False})
        self.assertEqual(mixed.generator_lag(timeline(0, 250.001, 1000), 4), {
            'available': True, 'period_ms': 250, 'maximum_pre_send_ms': 1000,
            'windows_over_one_period': [1, 2], 'exceeded_one_period': True})

    def test_invalid_maximum_delay_cannot_be_reported_as_valid_evidence(self):
        for value in [float('nan'), float('inf'), -float('inf'), -1, True, '1', None]:
            with self.subTest(value=value):
                with self.assertRaises(ValueError):
                    mixed.generator_lag(timeline(value), 4)

    def test_seconds_must_be_contiguous_ordered_integers(self):
        for seconds in [[0, 0], [1, 0], [0, 2], [0, 1.0], [0, True]]:
            with self.subTest(seconds=seconds):
                source = timeline(1, 2)
                for window, second in zip(source['windows'], seconds):
                    window['intended_second'] = second
                with self.assertRaises(ValueError):
                    mixed.generator_lag(source, 4)

    def test_rate_must_be_a_positive_integer(self):
        for rate in [0, -1, True, 1.5, float('nan'), float('inf')]:
            with self.subTest(rate=rate):
                with self.assertRaises(ValueError):
                    mixed.generator_lag(timeline(0), rate)

    def test_empty_timeline_is_rejected(self):
        with self.assertRaises(ValueError):
            mixed.generator_lag(timeline(), 4)

    def test_valid_input_is_not_modified(self):
        source = timeline(1, 500)
        original = copy.deepcopy(source)
        mixed.generator_lag(source, 4)
        self.assertEqual(source, original)


if __name__ == '__main__':
    unittest.main()
