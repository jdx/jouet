import contextlib
import io
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock


CHECKER = Path(__file__).resolve().parents[1] / "tasks/actual-running-balance/check.py"


class ActualCheckerTest(unittest.TestCase):
    def test_playwright_requires_a_completed_relevant_assertion(self):
        verdict = runpy.run_path(str(CHECKER))['browser_verdict']
        def report(status, message=''):
            return {'suites': [{'specs': [{'title': 'running balances refresh after repeated edits',
                    'tests': [{'results': [{'status': status, 'error': {'message': message}}]}]}]}], 'errors': []}

        self.assertEqual(verdict(report('passed'), 0), 'passed')
        self.assertEqual(verdict(report('failed', "Error: expect(locator).toHaveText(expected) failed\nLocator: getByTestId('balance')"), 1), 'failed')
        self.assertEqual(verdict(report('failed', "browserType.launch: Executable doesn't exist"), 1), 'unverified')
        self.assertEqual(verdict(report('failed', 'expect(received).toBe(expected) failed'), 1), 'unverified')
        self.assertEqual(verdict(report('skipped'), 0), 'unverified')
        self.assertEqual(verdict({'suites': []}, 0), 'unverified')
        self.assertEqual(verdict(report('passed'), 1), 'unverified')

    def test_agent_test_file_must_be_selected_and_complete_without_skips(self):
        verdict = runpy.run_path(str(CHECKER))['agent_browser_passed']
        test = {'expectedStatus': 'passed', 'results': [{'status': 'passed'}]}
        spec = {'title': 'regression', 'ok': True, 'tests': [test]}
        report = {'suites': [{'file': 'accounts.test.ts', 'specs': [spec]}], 'errors': [],
                  'stats': {'expected': 1, 'skipped': 0, 'unexpected': 0, 'flaky': 0}}
        self.assertTrue(verdict(report, 0, ['accounts.test.ts']))
        self.assertFalse(verdict(report, 1, ['accounts.test.ts']))
        self.assertFalse(verdict(report, 0, ['different.test.ts']))
        self.assertFalse(verdict({'suites': [], 'errors': []}, 0, ['accounts.test.ts']))
        self.assertFalse(verdict(report, 0, ['accounts.test.ts', 'other.test.ts']))
        report['suites'].append({'file': 'other.test.ts', 'specs': [spec]})
        self.assertTrue(verdict(report, 0, ['accounts.test.ts', 'other.test.ts']))
        report['suites'][1]['specs'] = []
        self.assertFalse(verdict(report, 0, ['accounts.test.ts', 'other.test.ts']))
        report['suites'].pop()
        report['stats']['skipped'] = 1
        self.assertFalse(verdict(report, 0, ['accounts.test.ts']))
        report['stats']['skipped'] = 0
        test['results'][0]['status'] = 'timedOut'
        self.assertFalse(verdict(report, 0, ['accounts.test.ts']))
        test['results'][0]['status'] = 'skipped'
        self.assertFalse(verdict(report, 0, ['accounts.test.ts']))

    def test_changed_browser_files_share_one_run_and_timeout(self):
        checker = runpy.run_path(str(CHECKER))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            tests = root / 'packages/desktop-client/e2e'
            tests.mkdir(parents=True)
            names = ['one.test.ts', 'two.test.ts']
            for name in names:
                (tests / name).write_text('test')
            spec = {'title': 'test', 'ok': True,
                    'tests': [{'expectedStatus': 'passed', 'results': [{'status': 'passed'}]}]}
            report = {'suites': [{'file': name, 'specs': [spec]} for name in names],
                      'errors': [], 'stats': {'expected': 2, 'skipped': 0, 'unexpected': 0, 'flaky': 0}}

            def run(_command, **kwargs):
                Path(kwargs['env']['PLAYWRIGHT_JSON_OUTPUT_FILE']).write_text(json.dumps(report))
                return subprocess.CompletedProcess([], 0, '', '')

            old_cwd = Path.cwd()
            try:
                os.chdir(root)
                with mock.patch.object(subprocess, 'run', side_effect=run) as command:
                    checker['check_agent_browser_tests']([f'packages/desktop-client/e2e/{name}' for name in names])
                self.assertEqual(command.call_count, 1)
                self.assertEqual(command.call_args.kwargs['timeout'], 600)
                self.assertEqual(checker['UNVERIFIED'], [])
            finally:
                os.chdir(old_cwd)

    def test_requires_the_working_tree_used_by_the_check_to_be_committed(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            env = {**os.environ, "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                   "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                   "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"}

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            (work / "base").write_text("base\n")
            git("add", "base")
            git("commit", "-qm", "base")
            base = git("rev-parse", "HEAD")
            (work / "packages/desktop-client/e2e").mkdir(parents=True)
            (work / "packages/desktop-client/e2e/regression.test.ts").write_text("test\n")
            (work / "upcoming-release-notes").mkdir()
            (work / "upcoming-release-notes/fix.md").write_text("note\n")
            git("add", "packages/desktop-client/e2e/regression.test.ts", "upcoming-release-notes/fix.md")
            git("commit", "-qm", "fix")

            # Stand in for Playwright and release-note validation: this test
            # exercises the checker's commit requirement, not those tools.
            bin_dir = root / "bin"
            bin_dir.mkdir()
            node = bin_dir / "node"
            node.write_text('''#!/bin/sh
mkdir -p blob-report
touch blob-report/report.zip
if [ "$1" != "--input-type=module" ] && [ -n "$PLAYWRIGHT_JSON_OUTPUT_FILE" ]; then
  if echo "$*" | grep -q regression; then
    if [ -n "$AGENT_BROWSER_FAILED" ]; then
      printf '%s' '{"suites":[{"file":"regression.test.ts","specs":[{"title":"regression","ok":false,"tests":[{"expectedStatus":"passed","results":[{"status":"timedOut"}]}]}]}],"errors":[],"stats":{"expected":0,"skipped":0,"unexpected":1,"flaky":0}}' > "$PLAYWRIGHT_JSON_OUTPUT_FILE"
      exit 1
    else
      printf '%s' '{"suites":[{"file":"regression.test.ts","specs":[{"title":"regression","ok":true,"tests":[{"expectedStatus":"passed","results":[{"status":"passed"}]}]}]}],"errors":[],"stats":{"expected":1,"skipped":0,"unexpected":0,"flaky":0}}' > "$PLAYWRIGHT_JSON_OUTPUT_FILE"
    fi
  elif [ -n "$BROWSER_MISSING" ]; then
    printf '%s' '{"suites":[{"specs":[{"title":"running balances refresh after repeated edits","tests":[{"results":[{"status":"failed","error":{"message":"browserType.launch: Executable does not exist"}}]}]}]}],"errors":[]}' > "$PLAYWRIGHT_JSON_OUTPUT_FILE"
    exit 1
  else
    printf '%s' '{"suites":[{"specs":[{"title":"running balances refresh after repeated edits","tests":[{"results":[{"status":"passed"}]}]}]}],"errors":[]}' > "$PLAYWRIGHT_JSON_OUTPUT_FILE"
  fi
fi
exit 0
''')
            node.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_BASE_SHA": base,
                        "PLAYWRIGHT_JSON_OUTPUT_FILE": str(root / 'unrelated.json')})

            def check():
                return subprocess.run(["python3", str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            (work / "uncommitted-source.js").write_text("required by the fix\n")
            self.assertNotEqual(check().returncode, 0)
            git("add", "uncommitted-source.js")
            git("commit", "-qm", "include source")
            result = check()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertFalse((root / 'unrelated.json').exists())
            self.assertTrue((work / "blob-report/report.zip").exists())
            shutil.rmtree(work / 'blob-report')

            env['AGENT_BROWSER_FAILED'] = '1'
            result = check()
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertIn('changed Playwright test files did not all complete successfully', result.stdout)
            self.assertIn('JOUET_BENCH_UNVERIFIED_V1', result.stdout)
            del env['AGENT_BROWSER_FAILED']
            shutil.rmtree(work / 'blob-report')

            checker = runpy.run_path(str(CHECKER))
            real_run = subprocess.run
            def timeout_note(command, *args, **kwargs):
                if command[0] == 'node' and '--input-type=module' in command:
                    raise subprocess.TimeoutExpired(command, 30, output=b'note started\n', stderr=b'note stalled\n')
                return real_run(command, *args, **kwargs)

            old_cwd = Path.cwd()
            try:
                os.chdir(work)
                with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(subprocess, 'run', side_effect=timeout_note):
                    output = io.StringIO()
                    with contextlib.redirect_stdout(output):
                        checker['check']()
                    self.assertIn('note started', output.getvalue())
                    self.assertIn('note stalled', output.getvalue())
                    self.assertIn('release note validation timed out', checker['UNVERIFIED'])
                    checker['UNVERIFIED'].clear()
            finally:
                os.chdir(old_cwd)
            shutil.rmtree(work / 'blob-report')

            env['BROWSER_MISSING'] = '1'
            result = check()
            self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
            self.assertTrue(result.stdout.rstrip().endswith('JOUET_BENCH_UNVERIFIED_V1'))
            self.assertFalse((work / 'packages/desktop-client/e2e/jouet-running-balance.test.ts').exists())
            shutil.rmtree(work / 'blob-report')

            git('rm', '-q', 'upcoming-release-notes/fix.md')
            git('commit', '-qm', 'remove the required release note')
            result = check()
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertNotIn('JOUET_BENCH_UNVERIFIED_V1', result.stdout)
