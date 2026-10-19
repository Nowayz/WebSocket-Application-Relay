#!/usr/bin/env python3
"""The live fanout gate distinguishes membership retirement from TCP closing."""
import unittest
import stability


class LivePressureTest(unittest.TestCase):
    def test_retired_transports_may_finish_close_after_membership_leaves(self):
        for connections in [62, 64, 66]:
            metrics = {'war_slow_consumers_total': '4', 'war_sessions': '62',
                       'war_connections': str(connections)}
            self.assertTrue(stability.live_fanout_pressure_valid(metrics, 66, 4))

    def test_missing_pressure_extra_overload_and_missing_healthy_peers_fail(self):
        for change in [{'war_slow_consumers_total': '3'}, {'war_slow_consumers_total': '5'},
                       {'war_sessions': '61'}, {'war_sessions': '63'},
                       {'war_connections': '61'}, {'war_connections': '67'}]:
            with self.subTest(change=change):
                metrics = {'war_slow_consumers_total': '4', 'war_sessions': '62',
                           'war_connections': '64'} | change
                self.assertFalse(stability.live_fanout_pressure_valid(metrics, 66, 4))

    def test_gate_rejects_another_mode_before_starting_children(self):
        with self.assertRaisesRegex(ValueError, "requires fanout"):
            stability.run_case('traffic', 1, 8, fanout_teardown_gate=True)

    def test_no_pressure_control_requires_every_session_and_no_overload(self):
        metrics = {'war_slow_consumers_total': '0', 'war_sessions': '66', 'war_connections': '66'}
        self.assertTrue(stability.live_fanout_pressure_valid(metrics, 66, 0))
        self.assertFalse(stability.live_fanout_pressure_valid(metrics | {'war_slow_consumers_total': '1'}, 66, 0))


if __name__ == '__main__':
    unittest.main()
