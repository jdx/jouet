import os
from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest


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
            (work / "regression.test.js").write_text("test\n")
            (work / "upcoming-release-notes").mkdir()
            (work / "upcoming-release-notes/fix.md").write_text("note\n")
            git("add", "regression.test.js", "upcoming-release-notes/fix.md")
            git("commit", "-qm", "fix")

            # Stand in for Playwright and release-note validation: this test
            # exercises the checker's commit requirement, not those tools.
            bin_dir = root / "bin"
            bin_dir.mkdir()
            node = bin_dir / "node"
            node.write_text('''#!/bin/sh
mkdir -p blob-report
touch blob-report/report.zip
if [ -n "$PLAYWRIGHT_JSON_OUTPUT_NAME" ]; then
  if [ -n "$BROWSER_MISSING" ]; then
    printf '%s' '{"suites":[{"specs":[{"title":"running balances refresh after repeated edits","tests":[{"results":[{"status":"failed","error":{"message":"browserType.launch: Executable does not exist"}}]}]}]}],"errors":[]}' > "$PLAYWRIGHT_JSON_OUTPUT_NAME"
    exit 1
  else
    printf '%s' '{"suites":[{"specs":[{"title":"running balances refresh after repeated edits","tests":[{"results":[{"status":"passed"}]}]}]}],"errors":[]}' > "$PLAYWRIGHT_JSON_OUTPUT_NAME"
  fi
fi
exit 0
''')
            node.chmod(0o755)
            env.update({"PATH": f"{bin_dir}{os.pathsep}{env['PATH']}", "BENCH_BASE_SHA": base})

            def check():
                return subprocess.run(["python3", str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True)

            (work / "uncommitted-source.js").write_text("required by the fix\n")
            self.assertNotEqual(check().returncode, 0)
            git("add", "uncommitted-source.js")
            git("commit", "-qm", "include source")
            result = check()
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((work / "blob-report/report.zip").exists())
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
