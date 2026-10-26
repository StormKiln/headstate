#!/usr/bin/env python3
"""Default-branch maintenance: obsolete queue runs and superseded `main` Rust
caches only; never changes checks."""
import argparse
from datetime import datetime
import importlib.util
import json
import os
from pathlib import Path
import re
import urllib.error
import urllib.parse
import urllib.request

ACTIVE = {'queued', 'in_progress', 'waiting', 'pending', 'requested'}
QUEUE = re.compile(r'gh-readonly-queue/main/pr-[0-9]+-[0-9a-f]+$')


def candidate(run, repo):
    return (run.get('event') == 'merge_group'
            and run.get('path') == '.github/workflows/ci.yml'
            and run.get('status') in ACTIVE
            and (run.get('head_repository') or {}).get('full_name', '').lower() == repo.lower()
            and bool(QUEUE.fullmatch(run.get('head_branch') or ''))
            and bool(re.fullmatch(r'[0-9a-f]{40}', run.get('head_sha') or '')))


def obsolete(run, repo, current_sha):
    return candidate(run, repo) and current_sha != run['head_sha']


def cancel_obsolete(run, repo, request):
    if not candidate(run, repo):
        return False
    # Re-read immediately before mutation: a completed run needs no cancellation.
    fresh = request('GET', f"/actions/runs/{int(run['id'])}")
    if not candidate(fresh, repo):
        return False
    ref = request('GET', '/git/ref/heads/' + urllib.parse.quote(fresh['head_branch'], safe='/'))
    # None means a confirmed ref-specific 404, not an API failure or bad JSON.
    current = None if ref is None else ref['object']['sha']
    if ref is not None and (not isinstance(current, str) or not re.fullmatch(r'[0-9a-f]{40}', current)):
        raise ValueError('Malformed ref SHA; cancellation refused')
    if not obsolete(fresh, repo, current):
        return False
    request('POST', f"/actions/runs/{int(fresh['id'])}/cancel")
    return True


# Only `main` writes the Rust cache (save-if in .github/actions/setup), so a
# dependency bump leaves main holding two generations of every class until
# GitHub's seven-day idle reclaim: the state that evicts (#901). Steady state
# alone is now near the 10GB quota, so that window cannot be waited out.
CACHE_REF = 'refs/heads/main'


def _job_class():
    # One definition of a cache class, shared with the budget guard.
    path = Path(__file__).with_name('check-cache-budget.py')
    spec = importlib.util.spec_from_file_location('cache_budget', path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.job_class


def superseded(caches, job_class):
    """Older `main` generations of each rust-cache class, never the newest.

    Newest by CREATED, not last accessed: after a bump, open pull requests
    still on the old lockfile keep restoring the old generation, so
    "last accessed" would delete the one `main` now needs. The cost of
    created-order is a lockfile REVERT, where main hits the old key and
    writes nothing: the old key is then deleted and rebuilt once.
    """
    classes = {}
    for cache in caches:
        if cache.get('ref') != CACHE_REF:
            continue
        cls = job_class(cache.get('key') or '')
        if cls is None:
            continue
        if not isinstance(cache.get('id'), int) or not cache.get('created_at'):
            raise ValueError('Malformed cache entry; pruning refused')
        classes.setdefault(cls, []).append(cache)
    stale = []
    for entries in classes.values():
        entries.sort(key=lambda c: (datetime.fromisoformat(c['created_at'].replace('Z', '+00:00')), c['id']))
        stale += entries[:-1]
    return stale


def prune_caches(api, apply):
    caches = list(api.pages('/actions/caches?ref=' + urllib.parse.quote(CACHE_REF, safe=''), 'actions_caches'))
    stale = superseded(caches, _job_class())
    for cache in stale:
        if apply:
            api.request('DELETE', f"/actions/caches/{int(cache['id'])}")
    return [f"{c['key']} ({c['size_in_bytes'] / 1024**3:.2f}GB, {c['created_at']})" for c in stale]


def gate_passed(needs):
    expected = {'lint-static', 'lint-native'}
    return set(needs) == expected and all(needs[k].get('result') == 'success' for k in expected)


def seconds(start, end):
    if not start or not end:
        return None
    elapsed = int((datetime.fromisoformat(end.replace('Z', '+00:00'))
                   - datetime.fromisoformat(start.replace('Z', '+00:00'))).total_seconds())
    return elapsed if elapsed >= 0 else None


def timing(job, created):
    # The API may set job.started_at to enqueue time while no runner exists.
    steps = job.get('steps') or []
    starts = [s['started_at'] for s in steps if s.get('started_at')]
    started = min(starts) if starts else None
    setup = [s for s in steps if s.get('name') == 'Run ./.github/actions/setup']
    setup_seconds = seconds(setup[0].get('started_at'), setup[0].get('completed_at')) if setup else None
    return seconds(created, started), seconds(started, job.get('completed_at')), setup_seconds


class GitHub:
    def __init__(self, repo, token):
        if not re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', repo):
            raise ValueError('Invalid repository')
        self.base = 'https://api.github.com/repos/' + repo
        self.token = token

    def request(self, method, path):
        req = urllib.request.Request(self.base + path, method=method, headers={
            'Authorization': 'Bearer ' + self.token, 'Accept': 'application/vnd.github+json',
            'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'headstate-ci-maintenance'})
        try:
            with urllib.request.urlopen(req, timeout=30) as response:
                body = response.read()
                value = json.loads(body) if body else {}
                if not isinstance(value, dict):
                    raise ValueError('Malformed API response; cancellation refused')
                return value
        except urllib.error.HTTPError as error:
            if error.code == 404 and method == 'GET' and path.startswith('/git/ref/heads/gh-readonly-queue/main/'):
                return None
            raise RuntimeError(f'GitHub {method} failed with HTTP {error.code}') from None

    def pages(self, path, key):
        for page in range(1, 101):
            separator = '&' if '?' in path else '?'
            items = self.request('GET', f'{path}{separator}per_page=100&page={page}')[key]
            yield from items
            if len(items) < 100:
                return
        raise RuntimeError('Pagination bound reached; refusing an incomplete inventory')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--gate', action='store_true')
    parser.add_argument('--apply', action='store_true',
                        help='cancel confirmed obsolete CI merge groups and delete superseded main Rust caches')
    args = parser.parse_args()
    if args.gate:
        if not gate_passed(json.loads(os.environ['NEEDS_JSON'])):
            raise SystemExit('Both static and native lint must succeed; skipped/cancelled is not success')
        print('Static and native lint passed')
        return
    repo = os.environ['GITHUB_REPOSITORY']
    api = GitHub(repo, os.environ['GH_TOKEN'])
    # A failed repository read must never masquerade as a collection of missing refs.
    api.request('GET', '')
    cancelled = []
    for status in sorted(ACTIVE):
        runs = list(api.pages('/actions/workflows/ci.yml/runs?event=merge_group&status=' + status, 'workflow_runs'))
        for run in runs:
            if not candidate(run, repo):
                continue
            if args.apply and cancel_obsolete(run, repo, api.request):
                cancelled.append(run['id'])
                print(f"Cancelled obsolete merge-group run {run['id']}")
    event = json.loads(Path(os.environ['GITHUB_EVENT_PATH']).read_text())
    trigger = event.get('workflow_run')
    if trigger:
        runs = [api.request('GET', f"/actions/runs/{int(trigger['id'])}")]
    else:
        runs = api.request('GET', '/actions/workflows/ci.yml/runs?per_page=10')['workflow_runs']
    lines = ['## CI scheduling and execution', '',
             'Queue values include dependency waits. Missing observations are shown as unknown.', '',
             '| Run | Job | Status | Queue seconds | Execution seconds | Setup seconds |',
             '|---|---|---|---:|---:|---:|']
    for run in runs:
        for job in api.pages(f"/actions/runs/{int(run['id'])}/attempts/{int(run['run_attempt'])}/jobs", 'jobs'):
            values = timing(job, run.get('run_started_at') or run['created_at'])
            name = re.sub(r'[^A-Za-z0-9 _().-]', '_', job.get('name', 'unknown'))
            cells = ' | '.join('unknown' if v is None else str(v) for v in values)
            lines.append(f"| {run['id']} | {name} | {job['status']} | {cells} |")
    pruned = prune_caches(api, args.apply)
    verb = 'deleted' if args.apply else 'would delete'
    lines += ['', f'Obsolete runs cancelled: {cancelled}',
              f'Superseded main Rust caches {verb}: {pruned}', '']
    summary = '\n'.join(lines)
    print(summary)
    if os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(os.environ['GITHUB_STEP_SUMMARY'], 'a') as output:
            output.write(summary)


if __name__ == '__main__':
    main()
