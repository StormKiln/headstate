#!/usr/bin/env python3
"""#1779: wait for all CI runs/attempts, including queued runs without checks.

This supplements, not replaces, the existing filter=all success-only check
policy. A green main run cannot stand in for a tag's pending workflow.
"""
import argparse
import sys
import time
from urllib.parse import quote
from release_delivery import GitHub, DeliveryError

class AdmissionError(DeliveryError):
    pass


def ready(api, sha, tag, observed=None):
    runs = api.pages(f'repos/{api.repo}/actions/workflows/ci.yml/runs?head_sha={quote(sha)}', key='workflow_runs')
    runs = [run for run in runs if run.get('head_sha') == sha]
    if observed is None:
        observed = {}
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
    for run in runs:
        attempts = run.get('run_attempt')
        if type(attempts) is not int or not 1 <= attempts <= 100:
            raise AdmissionError('missing or excessive CI attempt count')
        for number in range(1, attempts + 1):
            attempt = run if number == attempts else api.request(f'repos/{api.repo}/actions/runs/{run["id"]}/attempts/{number}')
            if attempt.get('status') != 'completed':
                pending = True
                continue
            if attempt.get('conclusion') != 'success':
                raise AdmissionError(f'CI run {run["id"]} attempt {number} concluded {attempt.get("conclusion")}; refusing release')
            jobs = api.pages(f'repos/{api.repo}/actions/runs/{run["id"]}/attempts/{number}/jobs', key='jobs')
            if not jobs:
                raise AdmissionError('completed CI attempt has no observable jobs')
            for job in jobs:
                if job.get('status') != 'completed':
                    pending = True
                elif job.get('conclusion') != 'success':
                    raise AdmissionError(f'CI run {run["id"]} attempt {number}: {job.get("name")} was not successful')
    return not pending


def wait(api, sha, tag, timeout, interval=15):
    deadline = time.monotonic() + timeout
    # Two fresh successful polls avoid relying on a single eventually-consistent
    # snapshot. This cannot promise that no future event will ever be delivered.
    consecutive = 0
    observed = {}
    while True:
        consecutive = consecutive + 1 if ready(api, sha, tag, observed) else 0
        if consecutive >= 2:
            print('All observed CI runs and attempts succeeded, including tag CI.')
            return
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise AdmissionError('deadline exceeded waiting for all CI attempts')
        print('Waiting for complete CI admission evidence...', flush=True)
        time.sleep(min(interval, remaining))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', required=True)
    parser.add_argument('--sha', required=True)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--timeout-seconds', type=int, default=3300)
    args = parser.parse_args()
    if not 1 <= args.timeout_seconds <= 3300:
        parser.error('timeout must be between 1 and 3300 seconds')
    wait(GitHub(args.repo), args.sha, args.tag, args.timeout_seconds)

if __name__ == '__main__':
    try: main()
    except (DeliveryError, TimeoutError) as error: sys.exit(str(error))
