#!/usr/bin/env python3
"""#1779: pending duplicate workflows must not disappear behind green checks."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
from release_delivery import ApiError

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
    def test_transient_error_resets_success_evidence_and_preserves_observed_runs(self):
        observations = []
        def poll(api, sha, tag, observed):
            observations.append(observed)
            observed[42] = run(42)
            if len(observations) == 2:
                raise ApiError('temporary', 502)
            return True
        with patch.object(gate, 'ready', side_effect=poll) as ready, patch.object(gate.time, 'sleep'):
            gate.wait(API([]), 'a'*40, 'v1.2.3', 100)
        self.assertEqual(ready.call_count, 4)
        self.assertTrue(all(item is observations[0] for item in observations))
        self.assertIn(42, observations[-1])

    def test_transient_errors_have_bounded_backoff(self):
        with patch.object(gate, 'ready', side_effect=ApiError('temporary', 503)) as ready, \
                patch.object(gate.time, 'sleep') as sleep, self.assertRaises(ApiError):
            gate.wait(API([]), 'a'*40, 'v1.2.3', 1000)
        self.assertEqual(ready.call_count, 6)
        self.assertEqual([call.args[0] for call in sleep.call_args_list], [15, 30, 60, 60, 60])

    def test_permanent_and_admission_errors_are_not_retried(self):
        for error in [ApiError('unauthorized', 401), ApiError('forbidden', 403),
                      ApiError('missing', 404), ApiError('unknown', None),
                      gate.AdmissionError('failed CI')]:
            with self.subTest(error=error), patch.object(gate, 'ready', side_effect=error) as ready, \
                    patch.object(gate.time, 'sleep') as sleep, self.assertRaises(type(error)):
                gate.wait(API([]), 'a'*40, 'v1.2.3', 100)
            self.assertEqual(ready.call_count, 1)
            sleep.assert_not_called()

    def test_transient_wait_respects_deadline(self):
        with patch.object(gate, 'ready', side_effect=ApiError('temporary', 504)), \
                patch.object(gate.time, 'monotonic', side_effect=[0, 0, 10]), \
                patch.object(gate.time, 'sleep') as sleep, self.assertRaises(gate.AdmissionError):
            gate.wait(API([]), 'a'*40, 'v1.2.3', 10)
        sleep.assert_called_once_with(10)

    def test_recovery_still_rejects_failed_ci(self):
        with patch.object(gate, 'ready', side_effect=[ApiError('temporary', 502),
                gate.AdmissionError('CI failed')]) as ready, patch.object(gate.time, 'sleep'), \
                self.assertRaisesRegex(gate.AdmissionError, 'CI failed'):
            gate.wait(API([]), 'a'*40, 'v1.2.3', 100)
        self.assertEqual(ready.call_count, 2)

    def test_success_after_deadline_does_not_admit_release(self):
        with patch.object(gate, 'ready', return_value=True), \
                patch.object(gate.time, 'monotonic', side_effect=[0, 0, 10]), \
                patch.object(gate.time, 'sleep'), self.assertRaises(gate.AdmissionError):
            gate.wait(API([]), 'a'*40, 'v1.2.3', 10)

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
