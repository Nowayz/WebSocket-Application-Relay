#!/usr/bin/env python3
"""Check switch pairing, CPU migration, boundary clipping and trace rejection."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('scheduling_analysis', Path(__file__).with_name('analyze-scheduling.py'))
analysis = importlib.util.module_from_spec(spec)
spec.loader.exec_module(analysis)


def event(comm, pid, tid, cpu, stamp, direction, preempt=False):
    whole, fraction = divmod(stamp, 1_000_000_000)
    return f'{comm} {pid}/{tid} [{cpu:03}] {whole}.{fraction:09}: PERF_RECORD_SWITCH {direction}' + (' preempt' if preempt else '') + '\n'


class SchedulingAnalysis(unittest.TestCase):
    def setUp(self):
        self.run = {'measurement_continue_monotonic_seconds':0, 'args':{'seconds':1}, 'profile_pids':{'server':1,'client':2}}

    def test_pairing_preemption_migration_and_running_time(self):
        def server(ms, direction, cpu=0, preempt=False):
            return event('war-io-0',1,11,cpu,ms*1_000_000,direction,preempt)
        def client(ms, direction):
            return event('tokio-rt-worker',2,22,8,ms*1_000_000,direction)
        result = analysis.analyze(self.run, [server(250,'IN'),server(251,'OUT',preempt=True),server(254,'IN',1),server(256,'OUT',1),server(258,'IN',1),server(260,'OUT',1),client(250,'IN'),client(255,'OUT'),client(260,'IN'),client(270,'OUT')])
        native = result['groups']['server_io']
        self.assertEqual(native['off_cpu']['count'],2)
        self.assertEqual(native['off_cpu']['max_ms'],3)
        self.assertEqual(native['preempted_off_cpu']['count'],1)
        self.assertEqual(native['preempted_off_cpu']['max_ms'],3)
        self.assertEqual(native['running_ms'],5)
        self.assertEqual(native['migrations'],1)
        self.assertEqual(result['groups']['client_io']['running_ms'],15)
        self.assertEqual(result['groups']['client_io']['off_cpu']['max_ms'],5)
        self.assertEqual(result['windows'][0]['groups']['server_io']['running_ms'],5)
        self.assertEqual(result['counts']['unpaired_in'],2)
        self.assertEqual(result['counts'].get('duplicate_directions',0),0)

    def test_measurement_end_does_not_create_an_extra_window(self):
        rows = [event('war-io-0',1,11,0,250_000_000,'IN'),event('war-io-0',1,11,0,1_250_000_000,'OUT'),event('war-io-0',1,11,0,1_250_000_000,'IN')]
        result = analysis.analyze(self.run,rows)
        self.assertEqual(len(result['windows']),1)
        self.assertEqual(result['groups']['server_io']['running_ms'],1000)
        self.assertEqual(result['groups']['server_io']['off_cpu']['count'],0)
        self.assertEqual(result['counts']['boundary_off_intervals'],1)

    def test_losses_remain_visible_and_malformed_switches_fail(self):
        result = analysis.analyze(self.run,['PERF_RECORD_LOST 3\n'])
        self.assertEqual(result['lost_records'],['PERF_RECORD_LOST 3'])
        with self.assertRaises(ValueError):
            analysis.analyze(self.run,['malformed PERF_RECORD_SWITCH IN\n'])


if __name__ == '__main__':
    unittest.main()
