#!/usr/bin/env python3
"""Keep unordered cross-room validation strict about identity and duplicates."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    'observer_barrier', Path(__file__).with_name('observer-barrier.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def packet(identifier):
    return b'\x82\x1b' + (2**64 - 1).to_bytes(8, 'big') + b'\x1b' + identifier.to_bytes(8, 'big')


class NoticeValidation(unittest.TestCase):
    def test_reverse_order_consumes_exact_set(self):
        pending = {24, 65536, 2**63}
        for identifier in [2**63, 65536, 24]:
            self.assertEqual(fixture.consume_notice(packet(identifier), pending), identifier)
        self.assertEqual(pending, set())

    def test_duplicate_cannot_replace_missing_notice(self):
        pending = {24, 65536}
        fixture.consume_notice(packet(24), pending)
        with self.assertRaises(AssertionError):
            fixture.consume_notice(packet(24), pending)
        self.assertEqual(pending, {65536})

    def test_unknown_source_does_not_consume_expected_notice(self):
        pending = {24}
        with self.assertRaises(AssertionError):
            fixture.consume_notice(packet(25), pending)
        self.assertEqual(pending, {24})

    def test_non_notice_and_trailing_bytes_rejected(self):
        for body in [b'', b'\x82\x00\x18\x18', packet(24) + b'\x00']:
            with self.subTest(body=body), self.assertRaises(AssertionError):
                fixture.consume_notice(body, {24})


if __name__ == '__main__':
    unittest.main()
