"""Vitest evidence must distinguish a change from its original code."""
import importlib.util
import tempfile
import unittest
from pathlib import Path

MODULE = Path(__file__).resolve().parents[1] / 'tasks/memos-sidebar-position/evidence.py'
spec = importlib.util.spec_from_file_location('memos_evidence', MODULE)
evidence = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evidence)


class MemosEvidenceTest(unittest.TestCase):
    def test_report_requires_exact_files_unique_cases_and_no_skips(self):
        with tempfile.TemporaryDirectory() as directory:
            web = Path(directory)
            case = {'fullName': 'group test', 'status': 'passed'}
            suite = {'name': str(web / 'tests/example.test.tsx'), 'assertionResults': [case]}
            report = {'testResults': [suite], 'numTotalTests': 1, 'numPassedTests': 1,
                      'numFailedTests': 0, 'numPendingTests': 0}
            expected = {'tests/example.test.tsx'}
            self.assertEqual(evidence.outcomes(report, web, expected),
                             {('tests/example.test.tsx', 'group test'): 'passed'})
            for invalid in (
                {**report, 'testResults': [suite, suite]},
                {**report, 'testResults': [{**suite, 'name': str(web.parent / 'outside.tsx')}]},
                {**report, 'testResults': [{**suite, 'assertionResults': [case, case]}]},
                {**report, 'testResults': [{**suite, 'assertionResults': [{**case, 'status': 'pending'}]}]},
                {**report, 'numTotalTests': 2},
                {**report, 'testResults': []},
            ):
                with self.subTest(report=invalid), self.assertRaises(evidence.IncompleteEvidence):
                    evidence.outcomes(invalid, web, expected)

    def test_new_and_modified_cases_must_fail_original_and_pass_fix(self):
        one = ('tests/example.test.tsx', 'group changed')
        two = ('tests/example.test.tsx', 'group added')
        fixed = {one: 'passed', two: 'passed'}
        self.assertTrue(evidence.distinguishes(fixed, {one: 'failed', two: 'passed'}, {one: 'passed'}))
        self.assertTrue(evidence.distinguishes(fixed, {one: 'passed', two: 'failed'}, {one: 'passed'}))
        self.assertFalse(evidence.distinguishes(fixed, fixed, {one: 'passed'}))
        for args in (
            (fixed, {one: 'failed'}, {one: 'passed'}),
            ({one: 'failed'}, {one: 'failed'}, {one: 'passed'}),
            (fixed, {one: 'failed', two: 'passed'}, {one: 'failed'}),
            (fixed, {one: 'failed', two: 'passed'}, {('other', 'removed'): 'passed'}),
        ):
            with self.subTest(args=args), self.assertRaises(evidence.IncompleteEvidence):
                evidence.distinguishes(*args)


if __name__ == '__main__':
    unittest.main()
