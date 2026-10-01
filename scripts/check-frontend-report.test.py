#!/usr/bin/env python3
"""A success status alone is not a complete frontend test report."""
import copy
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('report', Path(__file__).with_name('check-frontend-report.py'))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class Completeness(unittest.TestCase):
    def setUp(self):
        self.files = [{'file': 'src/a.test.ts'}, {'file': 'src/b.test.ts'}]
        self.result = {'success': True, 'numFailedTests': 0, 'numFailedTestSuites': 0,
                       'numTotalTests': 2, 'numPassedTests': 2, 'numPendingTests': 0, 'numTodoTests': 0,
                       'testResults': [{'name': f['file'], 'status': 'passed', 'assertionResults': [{'status': 'passed'}]}
                                       for f in self.files]}

    def test_complete_report(self):
        self.assertEqual(report.validate(self.files, self.result), (2, 2))

    def test_missing_duplicate_failed_and_partial_results(self):
        for mutate in (
            lambda r: r['testResults'].pop(),
            lambda r: r['testResults'].append(copy.deepcopy(r['testResults'][0])),
            lambda r: r.update(success=False),
            lambda r: r.update(numFailedTests=1),
            lambda r: r.update(numTotalTests=3),
            lambda r: r['testResults'][0].update(status='pending'),
            lambda r: r['testResults'][0].update(assertionResults=[]),
        ):
            with self.subTest(mutation=mutate):
                changed = copy.deepcopy(self.result)
                mutate(changed)
                with self.assertRaises(ValueError):
                    report.validate(self.files, changed)

    def test_running_assertions_are_not_completed_skips(self):
        self.result['numPassedTests'] = 0
        self.result['numPendingTests'] = 2
        for item in self.result['testResults']:
            item['assertionResults'][0]['status'] = 'pending'
        with self.assertRaises(ValueError):
            report.validate(self.files, self.result)

    def test_intentional_skips_and_todos_are_completed(self):
        self.result.update(numPassedTests=0, numPendingTests=1, numTodoTests=1)
        self.result['testResults'][0]['assertionResults'][0]['status'] = 'skipped'
        self.result['testResults'][1]['assertionResults'][0]['status'] = 'todo'
        self.assertEqual(report.validate(self.files, self.result), (2, 2))

    def test_duplicate_json_keys_are_rejected(self):
        with self.assertRaises(ValueError):
            report.unique([('success', False), ('success', True)])

    def test_empty_discovery_is_rejected(self):
        with self.assertRaises(ValueError):
            report.validate([], self.result)


if __name__ == '__main__':
    unittest.main()
