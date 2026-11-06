import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

CHECKER = Path(__file__).resolve().parents[1] / 'tasks/jactionlint-dependabot-groups/check.py'
spec = importlib.util.spec_from_file_location('jactionlint_checker', CHECKER)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class JactionlintCheckerTest(unittest.TestCase):
    def test_passing_behavior_does_not_excuse_a_missing_commit(self):
        result = subprocess.CompletedProcess(['go'], 0, stdout='', stderr='')
        with mock.patch.object(checker.subprocess, 'run', return_value=result), \
                mock.patch.object(checker, 'new_commit', return_value=False), \
                mock.patch.object(checker, 'dirty', return_value=[]), \
                mock.patch.object(checker, 'check_committed_tests'), \
                mock.patch.object(checker, 'finish'), \
                mock.patch.object(checker, 'expect') as expect:
            checker.main()
            expect.assert_any_call(False, 'nothing was committed')

    def test_behavior_failure_is_distinct_from_a_runner_error(self):
        for output, failed in (('regular and group: got 1 duplicate diagnostics, want 0', True),
                               ('go: module unavailable', False)):
            with self.subTest(output=output), mock.patch.object(checker, 'UNVERIFIED', []), \
                    mock.patch.object(checker.subprocess, 'run', return_value=subprocess.CompletedProcess(
                        ['go'], 1, stdout='', stderr=output)), \
                    mock.patch.object(checker, 'new_commit', return_value=True), \
                    mock.patch.object(checker, 'dirty', return_value=[]), \
                    mock.patch.object(checker, 'check_committed_tests'), \
                    mock.patch.object(checker, 'finish'), \
                    mock.patch.object(checker, 'expect') as expect:
                checker.main()
                failures = [call for call in expect.call_args_list if call.args[0] is False]
                self.assertEqual(bool(failures), failed)
                self.assertEqual(bool(checker.UNVERIFIED), not failed)

    def check_events(self, events, exit_code=0, diff='+func TestAdded(t *testing.T) {'):
        def run(command, **kwargs):
            if command[0] == 'git':
                return subprocess.CompletedProcess(command, 0, stdout=diff, stderr='')
            self.assertEqual(command[-1], '.')
            output = '\n'.join(json.dumps(event) for event in events)
            return subprocess.CompletedProcess(command, exit_code, stdout=output, stderr='')

        with tempfile.TemporaryDirectory() as directory:
            old = Path.cwd()
            try:
                os.chdir(directory)
                Path('added_test.go').write_text('package example\n')
                with mock.patch.object(checker, 'committed_files', return_value={'added_test.go'}), \
                        mock.patch.object(checker, 'head_sha', return_value='base'), \
                        mock.patch.object(checker, 'UNVERIFIED', []), \
                        mock.patch.object(checker, 'expect') as expect, \
                        mock.patch.object(checker.subprocess, 'run', side_effect=run):
                    checker.check_committed_tests()
                    return list(checker.UNVERIFIED), expect.call_args_list
            finally:
                os.chdir(old)

    def test_requires_the_added_test_to_pass_without_skipped_subtests(self):
        passed = [{'Action': 'pass', 'Test': 'TestAdded'}]
        self.assertFalse(self.check_events(passed)[0])
        for events in ([], [{'Action': 'skip', 'Test': 'TestAdded'}],
                       passed + [{'Action': 'skip', 'Test': 'TestAdded/child'}]):
            with self.subTest(events=events):
                self.assertTrue(self.check_events(events)[0])
        self.assertTrue(self.check_events(passed, exit_code=1)[0])
        self.assertTrue(self.check_events(passed, diff='+// modified existing test')[0])

    def test_named_test_failures_are_failed_not_unverified(self):
        for name in ('TestAdded', 'TestAdded/child'):
            with self.subTest(name=name):
                unverified, calls = self.check_events([{'Action': 'fail', 'Test': name}], exit_code=1)
                self.assertFalse(unverified)
                self.assertTrue(any(call.args[0] is False and name in call.args[1] for call in calls))

    def test_existing_edited_file_runs_the_package_alongside_new_test(self):
        def run(command, **kwargs):
            if command[0] == 'git':
                diff = '+func TestAdded(t *testing.T) {' if command[-1] == 'added_test.go' else '+// edited'
                return subprocess.CompletedProcess(command, 0, stdout=diff, stderr='')
            if command[1] == 'list':
                return subprocess.CompletedProcess(command, 0, stdout=json.dumps({'TestGoFiles': ['existing_test.go']}),
                                                   stderr='')
            test = 'TestAdded' if '-run' in command else 'TestExisting'
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps({'Action': 'pass', 'Test': test}),
                                               stderr='')

        with tempfile.TemporaryDirectory() as directory:
            old = Path.cwd()
            try:
                os.chdir(directory)
                for name in ('added_test.go', 'existing_test.go'):
                    Path(name).write_text('package example\n')
                with mock.patch.object(checker, 'committed_files', return_value={'added_test.go', 'existing_test.go'}), \
                        mock.patch.object(checker, 'head_sha', return_value='base'), \
                        mock.patch.object(checker, 'UNVERIFIED', []), \
                        mock.patch.object(checker, 'expect'), \
                        mock.patch.object(checker.subprocess, 'run', side_effect=run) as process:
                    checker.check_committed_tests()
                    self.assertFalse(checker.UNVERIFIED)
                    self.assertTrue(any(call.args[0][1] == 'test' and '-run' not in call.args[0]
                                        for call in process.call_args_list))
            finally:
                os.chdir(old)

    def test_timeout_output_preserves_bytes_and_text(self):
        error = subprocess.TimeoutExpired('go', 1, output=b'partial\xff', stderr=b'error')
        self.assertEqual(checker.timeout_output(error), 'partial\ufffderror')


    def test_untracked_files_are_unverified(self):
        for output in ('', 'testdata/ignored.yaml\0', 'helper.go\0'):
            result = subprocess.CompletedProcess(['git'], 0, stdout=output, stderr='')
            with self.subTest(output=output), mock.patch.object(checker, 'UNVERIFIED', []), \
                    mock.patch.object(checker.subprocess, 'run', return_value=result) as run:
                checker.check_untracked()
                self.assertEqual(bool(checker.UNVERIFIED), bool(output))
                self.assertNotIn('--exclude-standard', run.call_args.args[0])

    def test_fixture_tests_require_each_changed_fixture_to_run(self):
        names = ['TestDependabotFixtures', 'TestDependabotFixtures/ok/dependabot_grouped']
        with tempfile.TemporaryDirectory() as directory:
            old = Path.cwd()
            try:
                os.chdir(directory)
                fixture = Path('testdata/ok/dependabot_grouped.yaml')
                fixture.parent.mkdir(parents=True)
                fixture.write_text('version: 2\n')
                Path('dependabot_test.go').write_text('package example\n')
                files = {str(fixture)}
                self.assertFalse(checker.check_dependabot_fixtures(files | {'dependabot_test.go'}))
                for outcome in ('pass', 'missing', 'skip', 'fail', 'unrelated failure'):
                    events = [{'Action': 'pass', 'Test': name} for name in names]
                    if outcome == 'missing':
                        events.pop()
                    elif outcome in ('skip', 'fail'):
                        events[-1]['Action'] = outcome
                    elif outcome == 'unrelated failure':
                        events.append({'Action': 'fail', 'Test': 'TestDependabotFixtures/ok/dependabot_other'})
                    result = subprocess.CompletedProcess(['go'], int(outcome in ('fail', 'unrelated failure')),
                                                         stdout='\n'.join(map(json.dumps, events)), stderr='')
                    with self.subTest(outcome=outcome), mock.patch.object(checker, 'UNVERIFIED', []), \
                            mock.patch.object(checker, 'expect') as expect, \
                            mock.patch.object(checker.subprocess, 'run', return_value=result):
                        self.assertTrue(checker.check_dependabot_fixtures(files))
                        self.assertEqual(bool(checker.UNVERIFIED), outcome in ('missing', 'skip', 'unrelated failure'))
                        self.assertEqual(expect.called, outcome == 'fail')
            finally:
                os.chdir(old)


if __name__ == '__main__':
    unittest.main()
