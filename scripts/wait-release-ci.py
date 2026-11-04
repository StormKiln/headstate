#!/usr/bin/env python3
"""#1779: wait for all CI runs/attempts, including queued runs without checks.

A green main run cannot stand in for a tag's pending workflow, and a rerun
never washes out a failed attempt -- with ONE exception, below.

Runner capacity. v10.12.2 and v10.12.3 were both refused because macOS jobs
in the tag's own CI run were cancelled with "The job was not acquired by
Runner of type hosted even after multiple attempts". Those jobs never ran a
step, so their attempt says nothing about the code. An attempt is excused
only when every non-success job in it is either:

- capacity-cancelled: `cancelled`, no runner, no steps, AND carrying that
  annotation (all four, so a timeout or a superseded run never qualifies); or
- downstream of one: `skipped` because a `needs:` job was capacity-cancelled,
  or the `lint` aggregator, whose only step reports its needs (ci.yml).

An excused attempt is replaced, not ignored: with `--rerun-capacity` the gate
requests a rerun of the failed jobs (at most MAX_RERUNS per run), and the new
attempt must pass on its own. Without it, an excused latest attempt refuses.

`--check-runs` also requires every check run on the commit (all attempts,
every suite, minus `--ignore-checks`) to be `success` or one of the excused
jobs above. That replaces lewagon/wait-on-check-action, which could not tell
a capacity cancellation from a real one.
"""
import argparse
import re
import sys
import time
from pathlib import Path
from urllib.parse import quote
from release_delivery import GitHub, DeliveryError, ApiError

CAPACITY = 'The job was not acquired by Runner'
MAX_RERUNS = 3
# Jobs whose only work is to report their `needs:` (ci.yml `lint` runs
# `ci-maintenance.py --gate`). A failure there cannot be a code failure of its own.
AGGREGATORS = {'lint'}
CI_YML = Path(__file__).resolve().parent.parent / '.github/workflows/ci.yml'
MAX_TIMEOUT = 14100  # release.yml gate job: 240m, less margin


class AdmissionError(DeliveryError):
    pass


def ci_needs(text=None):
    """{job id: [needs]} from ci.yml's top-level jobs."""
    text = CI_YML.read_text() if text is None else text
    needs, job, inside = {}, None, False
    for line in text.splitlines():
        if line.rstrip() == 'jobs:':
            inside = True
            continue
        if not inside:
            continue
        top = re.fullmatch(r'  ([A-Za-z0-9_-]+):\s*', line)
        if top:
            job = top.group(1)
            needs.setdefault(job, [])
            continue
        found = re.fullmatch(r'    needs:\s*(\[[^\]]*\]|[A-Za-z0-9_-]+)\s*', line)
        if job and found:
            value = found.group(1).strip('[]')
            needs[job] = [item.strip() for item in value.split(',') if item.strip()]
    return needs


def base(name):
    # A matrix job reports as `platform (windows-latest)`; ci.yml names `platform`.
    return (name or '').split(' (', 1)[0]


def capacity_cancelled(api, job):
    if job.get('conclusion') != 'cancelled' or job.get('runner_id') or job.get('runner_name') \
            or job.get('steps'):
        return False
    notes = api.pages(f'repos/{api.repo}/check-runs/{int(job["id"])}/annotations')
    return any(CAPACITY in (note.get('message') or '') for note in notes)


def excused_jobs(api, jobs, needs):
    """Ids of every non-success job, if the attempt failed ONLY for runner capacity; else None."""
    bad = [job for job in jobs if job.get('conclusion') != 'success']
    capacity = {job['id'] for job in bad if capacity_cancelled(api, job)}
    if not capacity:
        return None
    lost = {base(job.get('name')) for job in bad if job['id'] in capacity}
    for job in bad:
        if job['id'] in capacity:
            continue
        if not set(needs.get(base(job.get('name')), [])) & lost:
            return None
        if job.get('conclusion') == 'skipped' or base(job.get('name')) in AGGREGATORS:
            continue
        return None
    return {job['id'] for job in bad}


def ready(api, sha, tag, observed=None, rerun=None, ignore=None, needs=None):
    """True when admitted. A `rerun` set enables capacity reruns, recording
    (run id, attempt) pairs already requested; an `ignore` set of check names
    enables the check-run requirement."""
    runs = api.pages(f'repos/{api.repo}/actions/workflows/ci.yml/runs?head_sha={quote(sha)}', key='workflow_runs')
    runs = [run for run in runs if run.get('head_sha') == sha]
    if observed is None:
        observed = {}
    if needs is None:
        needs = ci_needs()
    listed = {run['id'] for run in runs}
    for run_id in set(observed) - listed:
        run = api.request(f'repos/{api.repo}/actions/runs/{run_id}')
        if run.get('head_sha') != sha or run.get('id') != run_id:
            raise AdmissionError('previously observed CI run cannot be reconciled')
        runs.append(run)
    for run in runs:
        previous = observed.get(run['id'])
        if previous and run.get('run_attempt', 0) < previous.get('run_attempt', 0):
            # An older API snapshot cannot erase an already-observed rerun.
            continue
        observed[run['id']] = run
    runs = list(observed.values())
    tag_seen = any(run.get('event') == 'push' and run.get('head_branch') == tag for run in runs)
    pending = not tag_seen
    excused = set()
    for run in runs:
        attempts = run.get('run_attempt')
        if type(attempts) is not int or not 1 <= attempts <= 100:
            raise AdmissionError('missing or excessive CI attempt count')
        for number in range(1, attempts + 1):
            attempt = run if number == attempts else api.request(f'repos/{api.repo}/actions/runs/{run["id"]}/attempts/{number}')
            if attempt.get('status') != 'completed':
                pending = True
                continue
            jobs = api.pages(f'repos/{api.repo}/actions/runs/{run["id"]}/attempts/{number}/jobs', key='jobs')
            if not jobs:
                raise AdmissionError('completed CI attempt has no observable jobs')
            if attempt.get('conclusion') != 'success':
                lost = None
                if attempt.get('conclusion') in {'failure', 'cancelled'}:
                    lost = excused_jobs(api, jobs, needs)
                if lost is None:
                    raise AdmissionError(f'CI run {run["id"]} attempt {number} concluded {attempt.get("conclusion")}; refusing release')
                excused |= lost
                if number < attempts:
                    continue  # replaced by a later attempt, which is judged on its own
                if rerun is None:
                    raise AdmissionError(f'CI run {run["id"]} attempt {number} lost jobs to runner capacity and was not rerun; refusing release')
                if (run['id'], number) not in rerun:
                    if sum(1 for run_id, _ in rerun if run_id == run['id']) >= MAX_RERUNS:
                        raise AdmissionError(f'CI run {run["id"]} lost jobs to runner capacity {MAX_RERUNS + 1} times; refusing release')
                    api.request(f'repos/{api.repo}/actions/runs/{run["id"]}/rerun-failed-jobs', method='POST')
                    rerun.add((run['id'], number))
                    print(f'CI run {run["id"]} attempt {number} lost jobs to runner capacity; requested a rerun.', flush=True)
                pending = True
                continue
            for job in jobs:
                if job.get('status') != 'completed':
                    pending = True
                elif job.get('conclusion') != 'success':
                    raise AdmissionError(f'CI run {run["id"]} attempt {number}: {job.get("name")} was not successful')
    if ignore is not None:
        checks = api.pages(f'repos/{api.repo}/commits/{quote(sha)}/check-runs?filter=all', key='check_runs')
        for check in checks:
            if check.get('name') in ignore:
                continue
            if check.get('status') != 'completed':
                pending = True
            elif check.get('conclusion') != 'success' and check.get('id') not in excused:
                raise AdmissionError(f'check run {check.get("name")} ({check.get("id")}) concluded {check.get("conclusion")}; refusing release')
    return not pending


def wait(api, sha, tag, timeout, interval=15, rerun=False, ignore=None):
    deadline = time.monotonic() + timeout
    # Two fresh successful polls avoid relying on a single eventually-consistent
    # snapshot. This cannot promise that no future event will ever be delivered.
    consecutive = 0
    transient_failures = 0
    observed = {}
    requested = set() if rerun else None
    while True:
        delay = interval
        try:
            consecutive = consecutive + 1 if ready(api, sha, tag, observed, requested, ignore) else 0
            transient_failures = 0
        except ApiError as error:
            # v10.1.0 was stranded by one GitHub 502 during read-only polling.
            # Unavailable evidence is never success; retain observed runs but
            # require two fresh successful polls again after recovery.
            if error.status not in {500, 502, 503, 504}:
                raise
            consecutive = 0
            transient_failures += 1
            if transient_failures > 5:
                raise
            delay = min(interval * 2 ** (transient_failures - 1), 60)
            print(f'Transient GitHub HTTP {error.status}; retry {transient_failures}/5.', flush=True)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AdmissionError('deadline exceeded waiting for all CI attempts')
        if consecutive >= 2:
            print('All observed CI runs and attempts succeeded, including tag CI.')
            return
        print('Waiting for complete CI admission evidence...', flush=True)
        time.sleep(min(delay, remaining))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', required=True)
    parser.add_argument('--sha', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--timeout-seconds', type=int, default=3300)
    parser.add_argument('--rerun-capacity', action='store_true',
                        help='rerun failed jobs of an attempt that lost jobs only to runner capacity')
    parser.add_argument('--check-runs', action='store_true',
                        help='also require every check run on the commit to succeed or be excused')
    parser.add_argument('--ignore-checks', default='',
                        help='comma-separated check names --check-runs does not wait on')
    args = parser.parse_args()
    if not 1 <= args.timeout_seconds <= MAX_TIMEOUT:
        parser.error(f'timeout must be between 1 and {MAX_TIMEOUT} seconds')
    ignore = {name.strip() for name in args.ignore_checks.split(',') if name.strip()} if args.check_runs else None
    wait(GitHub(args.repo), args.sha, args.tag, args.timeout_seconds, rerun=args.rerun_capacity, ignore=ignore)

if __name__ == '__main__':
    try: main()
    except (DeliveryError, TimeoutError) as error: sys.exit(str(error))
