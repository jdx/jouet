"""Exercise the Memos checker's verdicts without requiring browser tools in CI."""
import contextlib
import io
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

TASKS = Path(__file__).resolve().parents[1] / 'tasks'
CHECK = TASKS / 'memos-sidebar-position/check.py'


class MemosCheckerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        git = self.bin / 'git'
        git.write_text('''#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
if args[:2] == ['rev-parse', 'HEAD']:
    print('changed-head' if (pathlib.Path.cwd() / '.changed-head').exists() else 'new-commit')
elif args[:2] == ['diff', '--name-only']:
    sys.stdout.write('web/tests/new.test.tsx\\0')
    if os.environ.get('OTHER_TEST'):
        sys.stdout.write('web/src/other.test.tsx\\0')
elif args[:2] == ['diff', '--quiet'] and (os.environ.get('CHANGED_DEPS')
        or (os.environ.get('CHANGED_CONFIG') and 'web/vitest.config.mts' in args)):
    sys.exit(1)
elif args and args[0] == 'clone':
    checkout = pathlib.Path(args[-1])
    (checkout / 'web' / 'tests').mkdir(parents=True)
    if checkout.name == 'work':
        (checkout / 'web' / 'tests' / 'new.test.tsx').write_text('test')
    elif os.environ.get('ORIGINAL_TEST'):
        (checkout / 'web' / 'tests' / 'new.test.tsx').write_text('pristine')
''')
        pnpm = self.bin / 'pnpm'
        pnpm.write_text('#!/bin/sh\nexit 0\n')
        node = self.bin / 'node'
        node.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
if '--no-allow-only' in args:
    if os.environ.get('AUTHORED_NO_REPORT'):
        print('runner could not start', file=sys.stderr)
        sys.exit(1)
    base = 'authored-base' in pathlib.Path.cwd().parts
    pristine = base and pathlib.Path('tests/new.test.tsx').read_text() == 'pristine'
    failed = ((pristine and os.environ.get('ORIGINAL_FAIL'))
              or (base and not pristine and not os.environ.get('AUTHORED_SAME'))
              or (not base and os.environ.get('AUTHORED_FAIL')))
    status = 'pending' if os.environ.get('AUTHORED_SKIPPED') else ('failed' if failed else 'passed')
    report = {'testResults': [{'name': str(pathlib.Path.cwd() / 'tests/new.test.tsx'),
                               'assertionResults': [{'fullName': 'new regression', 'status': status,
                                                     'failureMessages': ['report assertion details'] if failed else []}]}],
              'numTotalTests': 1, 'numPassedTests': int(status == 'passed'),
              'numFailedTests': int(status == 'failed'), 'numPendingTests': int(status == 'pending')}
    pathlib.Path(args[args.index('--outputFile') + 1]).write_text(json.dumps(report))
    if failed:
        print('assertion details', file=sys.stderr)
    if (not base and os.environ.get('MUTATE_FIXED')) or (base and not pristine and os.environ.get('MUTATE_BASE')):
        pathlib.Path('tests/new.test.tsx').write_text('changed during test')
    if not base and os.environ.get('MUTATE_HEAD'):
        (pathlib.Path.cwd().parent / '.changed-head').write_text('commit made')
    sys.exit(1 if failed else 0)
browser = '--config' in args
if browser and os.environ.get('MISSING_BROWSER'):
    print('browser missing', file=sys.stderr)
    sys.exit(1)
names = ([
    'keeps a fixed sidebar popup in place when the page scrolls',
    'preserves the default positioning for other popovers',
    'preserves the default positioning for other dropdown menus',
    'preserves the default positioning for other selects',
    'keeps sidebar Timeline settings fixed while scrolling',
    'keeps sidebar Timeline memo.order-by selector fixed while scrolling',
    'keeps sidebar Timeline memo.direction selector fixed while scrolling',
    'keeps sidebar memo Share fixed while scrolling',
    'keeps sidebar Tags fixed while scrolling',
    'keeps sidebar Views fixed while scrolling',
] if browser else [
    'keeps the sidebar scope popup and its Space selector fixed after positioning',
    'keeps the /setting.preference.theme/ submenu fixed after positioning',
    'keeps the /common.language/ submenu fixed after positioning',
])
rows = [{'fullName': name, 'status': 'passed'} for name in names]
if browser and os.environ.get('MISSING_TIMELINE'):
    rows = [row for row in rows if 'Timeline settings' not in row['fullName']]
if not browser and os.environ.get('FAIL_DOM'):
    rows[0].update(status='failed', failureMessages=['specific assertion'])
pathlib.Path(args[args.index('--outputFile') + 1]).write_text(
    json.dumps({'testResults': [{'assertionResults': rows}]}))
sys.exit(1 if not browser and os.environ.get('FAIL_DOM') else 0)
''')
        for path in (git, pnpm, node):
            path.chmod(0o755)
        self.env = {**os.environ, 'PATH': str(self.bin) + os.pathsep + os.defpath,
                    'BENCH_CHECK_HELPERS': str(TASKS), 'BENCH_HEAD_SHA': 'original-commit',
                    'PNPM_CONFIG_STORE_DIR': str(self.root / 'store')}

    def check(self, **env):
        return subprocess.run([sys.executable, '-I', str(CHECK)], cwd=self.root,
                              env={**self.env, **env}, capture_output=True, text=True)

    def test_complete_checks_pass(self):
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_browser_is_unverified(self):
        result = self.check(MISSING_BROWSER='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertEqual(result.stdout.splitlines()[-1], 'JOUET_BENCH_UNVERIFIED_V1')
        self.assertIn('browser missing', result.stdout)

    def test_authored_test_must_distinguish_the_original(self):
        result = self.check(AUTHORED_SAME='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('did not distinguish', result.stdout)

    def test_skipped_authored_test_is_unverified(self):
        result = self.check(AUTHORED_SKIPPED='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('skipped or unknown', result.stdout)

    def test_failing_authored_test_fails_the_check(self):
        result = self.check(AUTHORED_FAIL='1')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('authored tests fail on the fix', result.stdout)
        self.assertIn('assertion details', result.stdout)
        self.assertIn('report assertion details', result.stdout)

    def test_missing_authored_report_includes_runner_output(self):
        result = self.check(AUTHORED_NO_REPORT='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('runner could not start', result.stdout)

    def test_a_failing_original_suite_cannot_prove_a_regression(self):
        result = self.check(ORIGINAL_TEST='1', ORIGINAL_FAIL='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('original test suite did not pass', result.stdout)

    def test_changed_runner_configuration_is_unverified(self):
        result = self.check(CHANGED_CONFIG='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('runner configuration changed', result.stdout)

    def test_tests_outside_the_supported_directory_are_unverified(self):
        result = self.check(OTHER_TEST='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('outside web/tests', result.stdout)

    def test_authored_tests_cannot_change_the_files_being_compared(self):
        for key in ('MUTATE_FIXED', 'MUTATE_BASE', 'MUTATE_HEAD'):
            with self.subTest(key=key):
                result = self.check(**{key: '1'})
                self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
                self.assertIn('checkout changed during test execution', result.stdout)

    def test_authored_test_paths_cannot_follow_symlinks(self):
        web = self.root / 'web'
        (web / 'tests').mkdir(parents=True)
        outside = self.root / 'outside'
        outside.mkdir()
        (web / 'tests' / 'linked').symlink_to(outside, target_is_directory=True)
        with mock.patch.dict(os.environ, self.env, clear=True):
            checker = runpy.run_path(str(CHECK))
        with self.assertRaises(checker['IncompleteEvidence']):
            checker['safe_test_path'](web, 'tests/linked/new.test.tsx', required=False)

    def test_missing_sidebar_browser_case_is_unverified(self):
        result = self.check(MISSING_TIMELINE='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('Timeline settings fixed', result.stdout)

    def test_timeout_keeps_partial_runner_output(self):
        real_run = subprocess.run

        def run(command, *args, **kwargs):
            if command[0] == 'node' and '--config' in command:
                raise subprocess.TimeoutExpired(command, 90, output=b'browser started\n', stderr=b'browser stalled\n')
            return real_run(command, *args, **kwargs)

        old_cwd = Path.cwd()
        try:
            os.chdir(self.root)
            with mock.patch.dict(os.environ, self.env, clear=True), mock.patch.object(subprocess, 'run', side_effect=run):
                checker = runpy.run_path(str(CHECK))
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    checker['check']()
                self.assertIn('browser started', output.getvalue())
                self.assertIn('browser stalled', output.getvalue())
                self.assertIn('Chromium check timed out', checker['UNVERIFIED'])
                checker['UNVERIFIED'].clear()
        finally:
            os.chdir(old_cwd)

    def test_changed_dependencies_are_unverified(self):
        result = self.check(CHANGED_DEPS='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('frontend dependency files changed', result.stdout)

    def test_known_failure_wins_over_missing_browser(self):
        result = self.check(FAIL_DOM='1', MISSING_BROWSER='1')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('specific assertion', result.stdout)
        self.assertNotIn('JOUET_BENCH_UNVERIFIED_V1', result.stdout)
