import contextlib
import io
import json
import os
import runpy
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock


TASKS = Path(__file__).resolve().parents[1] / 'tasks'
CHECKER = TASKS / 'django-13344-container/check.py'
SPEC = TASKS / 'swebench-django-13344/swebench.json'
WITNESSES = [
    'test_add (cache.tests.MemcachedCacheTests)',
    'test_add (cache.tests.PyLibMCCacheTests)',
]


class DjangoContainerCheckerTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.work = self.root / 'work'
        self.work.mkdir()
        self.env = {**os.environ, 'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1',
                    'GIT_AUTHOR_NAME': 'Bench', 'GIT_AUTHOR_EMAIL': 'bench@example.invalid',
                    'GIT_COMMITTER_NAME': 'Bench', 'GIT_COMMITTER_EMAIL': 'bench@example.invalid',
                    'TMPDIR': str(self.root)}
        self.env.pop('BENCH_DOCKER', None)
        self.git('init', '-q')
        self.original = self.work / 'tests/cache/tests.py'
        self.original.parent.mkdir(parents=True)
        self.original.write_text('agent test content\n')
        self.original.chmod(0o640)
        self.git('add', 'tests/cache/tests.py')
        self.git('commit', '-qm', 'base')
        self.env['BENCH_HEAD_SHA'] = self.git('rev-parse', 'HEAD')
        (self.work / 'fix').write_text('fix\n')
        self.git('add', 'fix')
        self.git('commit', '-qm', 'fix')
        docker = self.root / 'docker'
        docker.write_text('#!/usr/bin/env python3\n'
                          'import os, sys\n'
                          'if sys.argv[1:3] == ["image", "inspect"]:\n'
                          '    print("sha256:" + "a" * 64 + " " + os.environ.get("FAKE_LABEL", "1"))\n'
                          'elif sys.argv[1] == "run":\n'
                          '    print(open(os.environ["FAKE_LOG"]).read())\n'
                          '    sys.exit(int(os.environ.get("FAKE_STATUS", "0")))\n')
        docker.chmod(0o755)
        self.env['BENCH_DOCKER'] = str(docker)
        self.log = self.root / 'output.log'
        self.env['FAKE_LOG'] = str(self.log)

    def git(self, *args):
        return subprocess.run(['git', *args], cwd=self.work, env=self.env, check=True,
                              capture_output=True, text=True).stdout.strip()

    def check(self):
        return subprocess.run([sys.executable, str(CHECKER)], cwd=self.work, env=self.env,
                              capture_output=True, text=True)

    def full_log(self, include_witnesses=True):
        spec = json.loads(SPEC.read_text())
        names = spec['fail_to_pass'] + spec['pass_to_pass']
        if include_witnesses:
            names += WITNESSES
        return ''.join(f'{name} ... ok\n' for name in names)

    def test_missing_docker_is_unverified_but_no_commit_is_a_failure(self):
        self.env.pop('BENCH_DOCKER')
        result = self.check()
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('JOUET_BENCH_UNVERIFIED_V1', result.stdout)
        self.env['BENCH_DOCKER'] = str(self.root / 'docker')
        self.env['FAKE_LABEL'] = 'wrong'
        self.assertEqual(self.check().returncode, 3)
        self.env['BENCH_HEAD_SHA'] = self.git('rev-parse', 'HEAD')
        self.assertEqual(self.check().returncode, 1)

    def test_complete_check_restores_original_files_and_requires_both_backends(self):
        self.log.write_text(self.full_log())
        result = self.check()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.original.read_text(), 'agent test content\n')
        self.assertEqual(self.original.stat().st_mode & 0o777, 0o640)
        self.assertFalse((self.work / 'tests/deprecation/test_middleware_mixin.py').exists())
        self.log.write_text(self.full_log(include_witnesses=False))
        result = self.check()
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('cache backend was not verified', result.stdout)

    def test_named_failure_wins_over_incomplete_results(self):
        self.log.write_text('test_coroutine (deprecation.test_middleware_mixin.MiddlewareMixinTests) ... FAIL\nAssertionError: expected a fresh tier\n')
        self.env['FAKE_STATUS'] = '1'
        result = self.check()
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn('test_coroutine', result.stdout)
        self.assertIn('AssertionError: expected a fresh tier', result.stdout)
        self.assertEqual(self.original.read_text(), 'agent test content\n')

    def test_unnamed_failure_preserves_diagnostics(self):
        self.log.write_text('container could not start the test runner\n')
        self.env['FAKE_STATUS'] = '1'
        result = self.check()
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('container could not start the test runner', result.stdout)
        self.assertEqual(result.stdout.splitlines()[-1], 'JOUET_BENCH_UNVERIFIED_V1')

    def test_timeout_preserves_partial_output(self):
        self.log.write_text('')
        real_run = subprocess.run

        def run(command, *args, **kwargs):
            if command[:2] == [self.env['BENCH_DOCKER'], 'run']:
                raise subprocess.TimeoutExpired(command, 600, output=b'partial test output\n')
            return real_run(command, *args, **kwargs)

        old_cwd = Path.cwd()
        try:
            os.chdir(self.work)
            with mock.patch.dict(os.environ, self.env, clear=True), mock.patch.object(subprocess, 'run', side_effect=run):
                checker = runpy.run_path(str(CHECKER))
                output = io.StringIO()
                with contextlib.redirect_stdout(output):
                    checker['run_check']()
                self.assertIn('partial test output', output.getvalue())
                self.assertIn('containerized tests timed out', checker['UNVERIFIED'])
                checker['UNVERIFIED'].clear()
        finally:
            os.chdir(old_cwd)

    def test_symlink_in_hidden_path_is_not_followed(self):
        self.original.unlink()
        self.original.parent.rmdir()
        outside = self.root / 'outside'
        outside.mkdir()
        (outside / 'tests.py').write_text('outside\n')
        (self.work / 'tests/cache').symlink_to(outside, target_is_directory=True)
        self.log.write_text(self.full_log())
        result = self.check()
        self.assertEqual(result.returncode, 3, result.stdout + result.stderr)
        self.assertIn('symlink in checker path', result.stdout)
        self.assertEqual((outside / 'tests.py').read_text(), 'outside\n')
