"""Exercise the Memos checker's verdicts without requiring browser tools in CI."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

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
    print('new-commit')
elif args[:2] == ['diff', '--name-only']:
    print('web/tests/new.test.tsx')
elif args[:2] == ['diff', '--quiet'] and os.environ.get('CHANGED_DEPS'):
    sys.exit(1)
elif args and args[0] == 'clone':
    (pathlib.Path(args[-1]) / 'web' / 'tests').mkdir(parents=True)
''')
        pnpm = self.bin / 'pnpm'
        pnpm.write_text('#!/bin/sh\nexit 0\n')
        node = self.bin / 'node'
        node.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
args = sys.argv[1:]
browser = '--config' in args
if browser and os.environ.get('MISSING_BROWSER'):
    print('browser missing', file=sys.stderr)
    sys.exit(1)
names = ([
    'keeps a fixed sidebar popup in place when the page scrolls',
    'preserves the default positioning for other popovers',
    'preserves the default positioning for other dropdown menus',
    'preserves the default positioning for other selects',
] if browser else [
    'keeps the md scope popup and its Space selector fixed after positioning',
    'keeps the header scope popup and its Space selector fixed after positioning',
    'keeps the /setting.preference.theme/ submenu fixed after positioning',
    'keeps the /common.language/ submenu fixed after positioning',
])
rows = [{'fullName': name, 'status': 'passed'} for name in names]
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

    def test_changed_dependencies_are_unverified(self):
        result = self.check(CHANGED_DEPS='1')
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('frontend dependency files changed', result.stdout)

    def test_known_failure_wins_over_missing_browser(self):
        result = self.check(FAIL_DOM='1', MISSING_BROWSER='1')
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('specific assertion', result.stdout)
        self.assertNotIn('JOUET_BENCH_UNVERIFIED_V1', result.stdout)
