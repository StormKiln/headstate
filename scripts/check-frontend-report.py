#!/usr/bin/env python3
"""Reject incomplete zero-exit Vitest runs (#1611), using its own file discovery."""
import json
from collections import Counter
from pathlib import Path
import sys


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def validate(discovered, report):
    expected = [str(Path(item['file']).resolve()) for item in discovered]
    results = report['testResults']
    actual = [str(Path(item['name']).resolve()) for item in results]
    if not expected or len(set(expected)) != len(expected):
        raise ValueError('discovery must contain distinct test files')
    if len(set(actual)) != len(actual) or set(expected) != set(actual):
        raise ValueError('reported test files do not match discovery')
    if report['success'] is not True or any(item['status'] != 'passed' for item in results):
        raise ValueError('report contains unsuccessful test files')
    if report['numFailedTests'] != 0 or report['numFailedTestSuites'] != 0:
        raise ValueError('report contains failures')
    total = report['numTotalTests']
    counts = [report[k] for k in ('numPassedTests', 'numPendingTests', 'numTodoTests')]
    if not isinstance(total, int) or total <= 0 or any(type(n) is not int or n < 0 for n in counts):
        raise ValueError('invalid test counts')
    if sum(counts) != total or sum(len(item['assertionResults']) for item in results) != total:
        raise ValueError('test counts do not match completed results')
    statuses = Counter(assertion['status'] for item in results for assertion in item['assertionResults'])
    if set(statuses) - {'passed', 'skipped', 'todo'}:
        raise ValueError('report contains unfinished or failed assertions')
    # Vitest includes genuinely running/queued tests AND intentional skips in
    # numPendingTests. Only assertion status distinguishes those cases.
    if [statuses['passed'], statuses['skipped'], statuses['todo']] != counts:
        raise ValueError('assertion statuses do not match report counts')
    return len(actual), total


def main():
    try:
        discovered, report = [json.loads(Path(p).read_text(), object_pairs_hook=unique) for p in sys.argv[1:]]
        files, tests = validate(discovered, report)
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f'::error::Incomplete frontend run: {error}', file=sys.stderr)
        return 1
    print(f'Frontend report verified: {tests} tests across all {files} discovered files')
    return 0


if __name__ == '__main__':
    sys.exit(main())
