#!/usr/bin/env python3
"""#1779: pending duplicate workflows must not disappear behind green checks."""
import importlib.util
import re
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
        def poll(api, sha, tag, observed, *rest):
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

# Runner capacity (v10.12.2, v10.12.3): a job GitHub never gave a runner.
CAPACITY_NOTE = [{'message': 'The job was not acquired by Runner of type hosted even after multiple attempts'}]

def job(id, name, conclusion='success', runner=True, steps=True):
    return dict(id=id, name=name, status='completed', conclusion=conclusion,
                runner_id=7 if runner else 0, runner_name='GitHub Actions 7' if runner else '',
                steps=[{'name': 'Set up job'}] if steps else [])

def lost(id, name):
    return job(id, name, 'cancelled', runner=False, steps=False)

NEEDS = {'lint': ['lint-static', 'lint-native'], 'lint-static': [], 'lint-native': [],
         'test-rust': [], 'mobile-changes': [], 'mobile-ios': ['mobile-changes']}

class Capacity:
    """Run 1 (the tag's push run); attempt -> jobs; annotations by job id; check runs."""
    repo = 'octocat/hello-world'
    def __init__(self, attempts, notes=None, checks=None):
        self.attempts = attempts  # {number: (conclusion, jobs)}
        self.notes = notes if notes is not None else {}
        self.checks = checks
        self.posts = []
    def latest(self):
        number = max(self.attempts)
        return run(conclusion=self.attempts[number][0], attempt=number)
    def pages(self, path, key=None):
        if key == 'workflow_runs':
            return [self.latest()]
        if key == 'jobs':
            return self.attempts[int(path.split('/attempts/')[1].split('/')[0])][1]
        if key == 'check_runs':
            return self.checks
        return self.notes.get(int(path.split('/check-runs/')[1].split('/')[0]), [])
    def request(self, path, method='GET', **kwargs):
        if method == 'POST':
            self.posts.append(path)
            return None
        number = int(path.rsplit('/', 1)[1])
        return run(conclusion=self.attempts[number][0], attempt=number)

def ready(api, rerun=None, ignore=None):
    return gate.ready(api, 'a'*40, 'v1.2.3', {}, rerun, ignore, NEEDS)

LINT_LOST = [job(10, 'lint-static'), lost(11, 'lint-native'), job(12, 'lint', 'failure'),
             lost(13, 'test-rust')]
GREEN = [job(20, 'lint-static'), job(21, 'lint-native'), job(22, 'lint'), job(23, 'test-rust')]

class CapacityTests(unittest.TestCase):
    def notes(self):
        return {11: CAPACITY_NOTE, 13: CAPACITY_NOTE}

    def test_capacity_attempt_requests_one_rerun_and_stays_pending(self):
        api = Capacity({1: ('failure', LINT_LOST)}, self.notes()); requested = set()
        self.assertFalse(ready(api, requested))
        self.assertFalse(ready(api, requested))
        self.assertEqual(api.posts, ['repos/octocat/hello-world/actions/runs/1/rerun-failed-jobs'])

    def test_capacity_attempt_replaced_by_green_rerun_admits(self):
        api = Capacity({1: ('failure', LINT_LOST), 2: ('success', GREEN)}, self.notes())
        self.assertTrue(ready(api, set()))
        self.assertEqual(api.posts, [])

    def test_without_rerun_a_capacity_latest_attempt_refuses(self):
        api = Capacity({1: ('failure', LINT_LOST)}, self.notes())
        with self.assertRaisesRegex(gate.AdmissionError, 'runner capacity'):
            ready(api)

    def test_rerun_budget_is_bounded(self):
        attempts = {n: ('failure', LINT_LOST) for n in range(1, gate.MAX_RERUNS + 2)}
        api = Capacity(attempts, self.notes())
        requested = {(1, n) for n in range(1, gate.MAX_RERUNS + 1)}
        with self.assertRaisesRegex(gate.AdmissionError, 'runner capacity'):
            ready(api, requested)

    def test_a_real_failure_beside_a_capacity_loss_is_never_excused(self):
        jobs = LINT_LOST[:3] + [job(13, 'test-rust', 'failure')]
        for attempts in [{1: ('failure', jobs)}, {1: ('failure', jobs), 2: ('success', GREEN)}]:
            with self.subTest(attempts=len(attempts)), self.assertRaises(gate.AdmissionError):
                ready(Capacity(attempts, {11: CAPACITY_NOTE}), set())

    def test_cancellation_needs_every_capacity_signal(self):
        # Timeout or superseded run: cancelled, but it had a runner, ran steps, or lacks the note.
        for bad in [job(13, 'test-rust', 'cancelled'), job(13, 'test-rust', 'cancelled', runner=False),
                    job(13, 'test-rust', 'cancelled', steps=False)]:
            with self.subTest(bad=bad), self.assertRaises(gate.AdmissionError):
                ready(Capacity({1: ('cancelled', [bad])}, {13: CAPACITY_NOTE}), set())
        with self.assertRaises(gate.AdmissionError):
            ready(Capacity({1: ('cancelled', [lost(13, 'test-rust')])}, {13: [{'message': 'timed out'}]}), set())

    def test_lint_is_excused_only_when_its_own_needs_lost_a_runner(self):
        jobs = [job(10, 'lint-static'), job(11, 'lint-native'), job(12, 'lint', 'failure'), lost(13, 'test-rust')]
        with self.assertRaises(gate.AdmissionError):
            ready(Capacity({1: ('failure', jobs)}, {13: CAPACITY_NOTE}), set())

    def test_skipped_dependents_of_a_lost_job_are_excused(self):
        jobs = [lost(30, 'mobile-changes'), job(31, 'mobile-ios', 'skipped')]
        api = Capacity({1: ('failure', jobs), 2: ('success', GREEN)}, {30: CAPACITY_NOTE})
        self.assertTrue(ready(api, set()))

    def test_a_dependent_that_ran_and_failed_is_not_excused(self):
        jobs = [lost(30, 'mobile-changes'), job(31, 'mobile-ios', 'failure')]
        with self.assertRaises(gate.AdmissionError):
            ready(Capacity({1: ('failure', jobs), 2: ('success', GREEN)}, {30: CAPACITY_NOTE}), set())

    def test_check_runs_excuse_exactly_the_lost_jobs(self):
        checks = [dict(id=j['id'], name=j['name'], status='completed', conclusion=j['conclusion'])
                  for j in LINT_LOST + GREEN]
        api = Capacity({1: ('failure', LINT_LOST), 2: ('success', GREEN)}, self.notes(), checks)
        self.assertTrue(ready(api, set(), set()))
        api.checks = checks + [dict(id=99, name='other', status='completed', conclusion='cancelled')]
        with self.assertRaisesRegex(gate.AdmissionError, 'other'):
            ready(api, set(), set())
        api.checks = checks + [dict(id=98, name='cache-budget', status='completed', conclusion='failure')]
        self.assertTrue(ready(api, set(), {'cache-budget'}))
        api.checks = checks + [dict(id=97, name='other', status='in_progress', conclusion=None)]
        self.assertFalse(ready(api, set(), set()))

    def test_ci_needs_reads_the_real_workflow(self):
        needs = gate.ci_needs()
        self.assertEqual(needs['lint'], ['lint-static', 'lint-native'])
        self.assertIn('test-rust', needs)

    def test_aggregators_only_report_their_needs(self):
        # AGGREGATORS may only name jobs whose one run step is ci-maintenance --gate;
        # anything with real work could hide a real failure behind a capacity loss.
        text = gate.CI_YML.read_text()
        for name in gate.AGGREGATORS:
            block = re.split(r'\n  [A-Za-z0-9_-]+:\n', text.split(f'\n  {name}:\n', 1)[1], maxsplit=1)[0]
            self.assertEqual(re.findall(r'\n\s+run: (.*)', block), ['python3 scripts/ci-maintenance.py --gate'])

if __name__ == '__main__': unittest.main()
