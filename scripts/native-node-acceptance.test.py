#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import signal
import subprocess
import unittest
from unittest.mock import patch, Mock

spec = importlib.util.spec_from_file_location('acceptance', Path(__file__).with_name('native-node-acceptance.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)

class Acceptance(unittest.TestCase):
    def test_requires_exact_signal_pid_matching_symbolized_capture(self):
        evidence = {'status':'captured','pid':123,'exception':{'signal':'SIGSEGV'},'symbolizedFrames':3}
        self.assertTrue(fixture.accepted(-signal.SIGSEGV, evidence, 123))
        for change in [{'status':'unavailable'}, {'pid':124}, {'exception':{'signal':'SIGABRT'}}, {'symbolizedFrames':0}]:
            self.assertFalse(fixture.accepted(-signal.SIGSEGV, {**evidence, **change}, 123))
        for status in [0, 1, -signal.SIGABRT, 139]:
            self.assertFalse(fixture.accepted(status, evidence, 123))

    def test_timeout_kills_owned_group_and_has_bounded_join(self):
        child = Mock(pid=123)
        child.wait.side_effect = [subprocess.TimeoutExpired('node', 10), -signal.SIGKILL]
        with patch.object(fixture.subprocess, 'Popen', return_value=child) as spawn, patch.object(fixture.os, 'killpg') as kill:
            result = fixture.bounded_child(['node', 'synthetic.cjs'], Path('/tmp'))
        self.assertTrue(result[-1])
        kill.assert_called_once_with(123, signal.SIGKILL)
        self.assertEqual(child.wait.call_args_list[-1].kwargs, {'timeout':5})
        self.assertEqual(spawn.call_args.kwargs['env'], {})
        self.assertTrue(spawn.call_args.kwargs['start_new_session'])
        self.assertIs(spawn.call_args.kwargs['preexec_fn'], fixture.no_core)

    def test_report_flags_match_frontend_and_exclude_private_context(self):
        wrapper = (fixture.ROOT / 'scripts/test-frontend-ci.sh').read_text()
        for flag in fixture.FLAGS:
            self.assertIn(flag, wrapper)
        self.assertIn('--report-exclude-env', fixture.FLAGS)
        self.assertIn('--report-exclude-network', fixture.FLAGS)

if __name__ == '__main__':
    unittest.main()
