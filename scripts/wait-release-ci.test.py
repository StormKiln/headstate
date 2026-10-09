#!/usr/bin/env python3
"""#1779: pending duplicate workflows must not disappear behind green checks."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location('gate', Path(__file__).with_name('wait-release-ci.py'))
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)

class API:
    repo = 'octocat/hello-world'
    def __init__(self, runs, attempts=None, jobs=None):
        self.runs = runs
        self.attempts = attempts or {}
        self.jobs = jobs or [{'name': 'new-ci-job', 'status': 'completed', 'conclusion': 'success'}]
    def pages(self, path, key=None):
        return self.runs if key == 'workflow_runs' else self.jobs
    def request(self, path, **kwargs):
        return self.attempts[int(path.rsplit('/', 1)[1])]


def run(id=1, status='completed', conclusion='success', attempt=1, branch='v1.2.3'):
    return dict(id=id, status=status, conclusion=conclusion, run_attempt=attempt,
                head_branch=branch, head_sha='a'*40, event='push')

class GateTests(unittest.TestCase):
    def test_observed_pending_run_cannot_disappear_from_later_list_snapshots(self):
        class EventuallyConsistent(API):
            def request(self, path, **kwargs):
                return self.direct
        api = EventuallyConsistent([run(), run(2, 'pending', None)])
        observed = {}
        self.assertFalse(gate.ready(api, 'a'*40, 'v1.2.3', observed))
        api.runs = [run()]; api.direct = run(2, 'pending', None)
        self.assertFalse(gate.ready(api, 'a'*40, 'v1.2.3', observed))
        self.assertFalse(gate.ready(api, 'a'*40, 'v1.2.3', observed))
        api.direct = run(2)
        self.assertTrue(gate.ready(api, 'a'*40, 'v1.2.3', observed))

    def test_pending_duplicate_without_jobs_keeps_gate_closed(self):
        self.assertFalse(gate.ready(API([run(), run(2, 'pending', None)]), 'a'*40, 'v1.2.3'))
    def test_main_success_cannot_substitute_for_missing_tag_ci(self):
        self.assertFalse(gate.ready(API([run(branch='main')]), 'a'*40, 'v1.2.3'))
    def test_all_successful_duplicates_pass(self):
        self.assertTrue(gate.ready(API([run(), run(2)]), 'a'*40, 'v1.2.3'))
    def test_failure_cancellation_and_skip_never_become_success(self):
        for conclusion in ['failure', 'cancelled', 'skipped', 'timed_out', None]:
            with self.subTest(conclusion=conclusion), self.assertRaises(gate.AdmissionError):
                gate.ready(API([run(conclusion=conclusion)]), 'a'*40, 'v1.2.3')
    def test_failed_previous_attempt_stays_failed_after_successful_rerun(self):
        with self.assertRaises(gate.AdmissionError):
            gate.ready(API([run(attempt=2)], {1: run(conclusion='failure')}), 'a'*40, 'v1.2.3')
    def test_skipped_new_job_blocks_even_if_workflow_conclusion_is_success(self):
        with self.assertRaises(gate.AdmissionError):
            gate.ready(API([run()], jobs=[dict(name='new-ci-job', status='completed', conclusion='skipped')]), 'a'*40, 'v1.2.3')
    def test_empty_jobs_on_completed_attempt_fail_closed(self):
        api = API([run()]); api.jobs = []
        with self.assertRaises(gate.AdmissionError):
            gate.ready(api, 'a'*40, 'v1.2.3')

if __name__ == '__main__': unittest.main()
