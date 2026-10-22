#!/usr/bin/env python3
"""Cancellation must never turn an active quality gate into missing work."""
import importlib.util
import io
from unittest.mock import patch
from pathlib import Path
import unittest

SOURCE = Path(__file__).with_name('ci-maintenance.py')

class Maintenance(unittest.TestCase):
    def setUp(self):
        self.assertTrue(SOURCE.exists(), 'CI maintenance is not implemented')
        spec = importlib.util.spec_from_file_location('maintenance', SOURCE)
        self.m = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.m)
        self.run = dict(id=7, event='merge_group', path='.github/workflows/ci.yml',
                        head_branch='gh-readonly-queue/main/pr-1983-abc', head_sha='a'*40,
                        status='queued', head_repository={'full_name': 'StormKiln/headstate'})

    def test_active_exact_ref_is_never_cancelled(self):
        self.assertFalse(self.m.obsolete(self.run, 'StormKiln/headstate', 'a'*40))

    def test_missing_or_replaced_queue_ref_is_obsolete(self):
        self.assertTrue(self.m.obsolete(self.run, 'StormKiln/headstate', None))
        self.assertTrue(self.m.obsolete(self.run, 'StormKiln/headstate', 'b'*40))

    def test_non_queue_and_release_work_is_never_cancelled(self):
        for fields in [dict(event='pull_request'), dict(event='push'),
                       dict(head_branch='main'), dict(head_branch='v10.9.4'),
                       dict(path='.github/workflows/release.yml'),
                       dict(status='completed'), dict(head_repository={'full_name':'other/repo'}),
                       dict(head_sha=''), dict(head_branch='gh-readonly-queue/other/pr-1-abc')]:
            with self.subTest(fields=fields):
                self.assertFalse(self.m.obsolete({**self.run, **fields}, 'StormKiln/headstate', None))

    def test_cancellation_rechecks_ref_and_run(self):
        calls = []
        def request(method, path):
            calls.append((method, path))
            if path == '/actions/runs/7': return self.run
            if path.startswith('/git/ref/'): return {'object': {'sha': 'a'*40}}
            self.fail('must not cancel a group whose current ref still matches')
        self.assertFalse(self.m.cancel_obsolete(self.run, 'StormKiln/headstate', request))
        self.assertEqual(len(calls), 2)

    def test_only_confirmed_missing_ref_allows_cancellation(self):
        calls = []
        def request(method, path):
            calls.append((method, path))
            if path == '/actions/runs/7': return self.run
            if path.startswith('/git/ref/'): return None
            if path == '/actions/runs/7/cancel': return {}
            self.fail(path)
        self.assertTrue(self.m.cancel_obsolete(self.run, 'StormKiln/headstate', request))
        self.assertEqual(calls[-1], ('POST', '/actions/runs/7/cancel'))

    def test_malformed_ref_is_not_evidence_for_cancellation(self):
        for value in [None, '', 'not-a-sha']:
            def request(method, path):
                if path == '/actions/runs/7': return self.run
                if path.startswith('/git/ref/'): return {'object': {'sha': value}}
                self.fail('malformed ref must not cause cancellation')
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.m.cancel_obsolete(self.run, 'StormKiln/headstate', request)

    def test_api_error_never_cancels(self):
        def request(method, path):
            raise RuntimeError('API unavailable')
        with self.assertRaises(RuntimeError):
            self.m.cancel_obsolete(self.run, 'StormKiln/headstate', request)

    def test_only_ref_404_is_missing_other_errors_propagate(self):
        api = self.m.GitHub('StormKiln/headstate', 'synthetic-token')
        ref = '/git/ref/heads/gh-readonly-queue/main/pr-1-abc'
        for code in [401,403,429,500]:
            error = self.m.urllib.error.HTTPError('https://api.github.com', code, 'synthetic', {}, None)
            with self.subTest(code=code), patch.object(self.m.urllib.request,'urlopen',side_effect=error):
                with self.assertRaises(RuntimeError): api.request('GET', ref)
        error = self.m.urllib.error.HTTPError('https://api.github.com',404,'synthetic',{},None)
        with patch.object(self.m.urllib.request,'urlopen',side_effect=error):
            self.assertIsNone(api.request('GET', ref))
            with self.assertRaises(RuntimeError): api.request('GET', '/actions/runs/7')

    def test_successful_null_response_is_not_a_missing_ref(self):
        api = self.m.GitHub('StormKiln/headstate', 'synthetic-token')
        for body in [b'null', b'[]', b'"bad"']:
            with self.subTest(body=body), patch.object(self.m.urllib.request, 'urlopen', return_value=io.BytesIO(body)):
                with self.assertRaises(ValueError):
                    api.request('GET', '/git/ref/heads/gh-readonly-queue/main/pr-1-abc')

    def test_gate_requires_both_successes(self):
        good = {'lint-static': {'result':'success'}, 'lint-native': {'result':'success'}}
        self.assertTrue(self.m.gate_passed(good))
        for state in ['failure','cancelled','skipped','pending',None]:
            self.assertFalse(self.m.gate_passed({**good, 'lint-native':{'result':state}}))
        self.assertFalse(self.m.gate_passed({}))
        self.assertFalse(self.m.gate_passed({'lint-static':{'result':'success'}}))

    def test_inconsistent_rerun_order_is_unknown_not_zero(self):
        self.assertIsNone(self.m.seconds('2026-10-21T22:00:00Z', '2026-10-21T21:00:00Z'))

    def test_timing_uses_first_step_not_queued_job_started_at(self):
        job = {'name':'test-rust', 'started_at':'2026-10-21T20:00:00Z',
               'completed_at':'2026-10-21T21:04:00Z', 'status':'completed',
               'steps':[{'name':'Set up job','started_at':'2026-10-21T21:00:00Z'},
                        {'name':'Run ./.github/actions/setup','started_at':'2026-10-21T21:00:10Z',
                         'completed_at':'2026-10-21T21:01:10Z'}]}
        self.assertEqual(self.m.timing(job, '2026-10-21T20:00:00Z'), (3600,240,60))
        self.assertEqual(self.m.timing({'status':'queued','started_at':job['started_at'],'steps':[]},
                                       '2026-10-21T20:00:00Z'), (None,None,None))

if __name__ == '__main__': unittest.main()
