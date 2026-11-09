import contextlib
import importlib.util
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from jbench import tasks

TASKS = Path(__file__).resolve().parents[1] / "tasks"
sys.path.insert(0, str(TASKS))
spec = importlib.util.spec_from_file_location(
    "fnox_bats_check", TASKS / "fnox-editor-args" / "bats_check.py"
)
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class FnoxBatsTest(unittest.TestCase):
    def test_age_identity_task_exposes_both_age_tools(self):
        task = tasks.load("fnox-age-identity-pr")
        self.assertEqual(task.tools.get("age"), "age")
        self.assertEqual(task.tools.get("age-keygen"), "age-keygen")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            binaries = {}
            for name in ("mise", "age", "age-keygen"):
                binary = root / name
                binary.write_text(f"#!/bin/sh\nprintf '{name} available\\n'\n")
                binary.chmod(0o755)
                binaries[name] = binary
            env = {"PATH": str(bin_dir), "HOME": str(root)}
            with mock.patch("jbench.arms.resolve", side_effect=binaries.get):
                task.agent_env(env, bin_dir, root)
            for name in ("age", "age-keygen"):
                result = subprocess.run([name], env=env, text=True, capture_output=True, check=True)
                self.assertEqual(result.stdout, f"{name} available\n")

    def test_editor_task_exposes_age_keygen_to_the_isolated_environment(self):
        task = tasks.load("fnox-editor-args")
        self.assertEqual(task.tools.get("age-keygen"), "age-keygen")
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "age-keygen"
            binary.write_text("#!/bin/sh\nprintf 'available\\n'\n")
            binary.chmod(0o755)
            bin_dir = root / "bin"
            bin_dir.mkdir()
            env = {"PATH": str(bin_dir), "HOME": str(root)}
            with mock.patch("jbench.arms.resolve", return_value=binary):
                task.agent_env(env, bin_dir, root)
            result = subprocess.run(["age-keygen"], env=env, text=True, capture_output=True, check=True)
            self.assertEqual(result.stdout, "available\n")

    def test_checker_imports_its_helper_in_isolated_python(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            subprocess.run(
                ["git", "-C", str(root), "-c", "user.name=Bench", "-c", "user.email=bench@example.invalid",
                 "commit", "--allow-empty", "-qm", "base"],
                check=True,
            )
            base = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
            (root / "cargo").write_text("#!/bin/sh\nexit 1\n")
            (root / "cargo").chmod(0o755)
            env = {**os.environ, "PATH": f"{root}{os.pathsep}{os.environ['PATH']}", "BENCH_HEAD_SHA": base}
            result = subprocess.run(
                [sys.executable, "-I", str(TASKS / "fnox-editor-args" / "check.py")],
                cwd=root, env=env, capture_output=True, text=True, check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("ModuleNotFoundError", result.stderr)
            self.assertIn("build fails", result.stdout)

    def test_realistic_tap_requires_complete_success(self):
        self.assertIsNone(checker.inspect_tap("1..2\nok 1 first\nok 2 second\n", 0))
        cases = [
            ("", 0),
            ("1..0\n", 0),
            ("1..2\nok 1 first\n", 0),
            ("1..2\nok 1 first\nok 1 duplicate\n", 0),
            ("1..1\nok 1 first # skip unavailable\n", 0),
            ("1..1\nok 1 first # TODO later\n", 0),
            ("1..1\nnot ok 1 first\n", 1),
            ("1..1\nok 1 first\n", 1),
            ("1..1\n1..1\nok 1 first\n", 0),
        ]
        for output, status in cases:
            with self.subTest(output=output, status=status):
                self.assertIsNotNone(checker.inspect_tap(output, status))
        self.assertIsNone(checker.inspect_tap("1..2\nok 1 first\nok 2 second\n", 0, ["first", "second"]))
        self.assertIsNotNone(checker.inspect_tap("1..2\nok 1 first\nok 2 other\n", 0, ["first", "second"]))
        self.assertEqual(
            checker.inspect_tap("1..2\nnot ok 1 first\nok 2 second # skip unavailable\n", 1, ["first", "second"]),
            "a committed regression test failed",
        )

    def test_complete_run_executes_both_committed_tests(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        original = subprocess.CompletedProcess([], 0, '@test "old" {\n', "")
        current = subprocess.CompletedProcess([], 0, '@test "old" {\n@test "first" {\n@test "second" {\n', "")
        clean = subprocess.CompletedProcess([], 0, "", "")
        report = subprocess.CompletedProcess([], 0, "1..3\nok 1 old\nok 2 first\nok 3 second\n", "")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, original, current, clean, report]), \
                contextlib.redirect_stdout(io.StringIO()):
            checker.check_bats()
            self.assertEqual(checker.UNVERIFIED, [])

    def test_bats_failure_and_skip_are_unverified(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        original = subprocess.CompletedProcess([], 0, '@test "old" {\n', "")
        current = subprocess.CompletedProcess([], 0, '@test "old" {\n@test "first" {\n@test "second" {\n', "")
        clean = subprocess.CompletedProcess([], 0, "", "")
        report = subprocess.CompletedProcess(
            [], 1, "1..2\nok 1 editor # skip unavailable\nnot ok 2 keychain\n", ""
        )
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, original, current, clean, report]), \
                contextlib.redirect_stdout(io.StringIO()):
            checker.check_bats()
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("skipped", checker.UNVERIFIED[0])

    def test_missing_tests_are_unverified(self):
        unchanged = subprocess.CompletedProcess([], 0, "", "")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", return_value=unchanged):
            checker.check_bats()
            self.assertIn("no committed editor regression", checker.UNVERIFIED[0])

    def test_renamed_existing_tests_are_unverified(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        original = subprocess.CompletedProcess([], 0, '@test "old one" {\n@test "old two" {\n', "")
        renamed = subprocess.CompletedProcess([], 0, '@test "new one" {\n@test "new two" {\n', "")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, original, renamed]) as run:
            checker.check_bats()
            self.assertIn("missing, renamed, or ambiguous", checker.UNVERIFIED[0])
            self.assertEqual(run.call_count, 3)

    def test_timeout_preserves_a_failed_new_regression(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        original = subprocess.CompletedProcess([], 0, '@test "old" {\n', "")
        current = subprocess.CompletedProcess([], 0, '@test "old" {\n@test "first" {\n@test "second" {\n', "")
        clean = subprocess.CompletedProcess([], 0, "", "")
        timeout = subprocess.TimeoutExpired("bats", 180, output=b"1..3\nok 1 old\nnot ok 2 first\n")
        output = io.StringIO()
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker, "expect") as expect, \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, original, current, clean, timeout]), \
                contextlib.redirect_stdout(output):
            checker.check_bats()
            expect.assert_called_once_with(False, "a committed regression test failed before the Bats suite timed out")
            self.assertEqual(checker.UNVERIFIED, [])
            self.assertIn("not ok 2 first", output.getvalue())

    def test_timeout_without_a_failed_regression_is_unverified(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        original = subprocess.CompletedProcess([], 0, '@test "old" {\n', "")
        current = subprocess.CompletedProcess([], 0, '@test "old" {\n@test "first" {\n@test "second" {\n', "")
        clean = subprocess.CompletedProcess([], 0, "", "")
        timeout = subprocess.TimeoutExpired("bats", 180, output=b"1..3\nok 1 old\n")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, original, current, clean, timeout]), \
                contextlib.redirect_stdout(io.StringIO()):
            checker.check_bats()
            self.assertIn("timed out", checker.UNVERIFIED[0])

    def test_untracked_files_cannot_make_tests_pass(self):
        changed = subprocess.CompletedProcess([], 0, "test/edit.bats\n", "")
        original = subprocess.CompletedProcess([], 0, '@test "old" {\n', "")
        current = subprocess.CompletedProcess([], 0, '@test "old" {\n@test "first" {\n@test "second" {\n', "")
        untracked = subprocess.CompletedProcess([], 0, "test/helper.bash\n", "")
        with mock.patch.dict(os.environ, {"BENCH_HEAD_SHA": "base"}), \
                mock.patch.object(checker, "UNVERIFIED", []), \
                mock.patch.object(checker.subprocess, "run", side_effect=[changed, original, current, untracked]) as run:
            checker.check_bats()
            self.assertEqual(len(checker.UNVERIFIED), 1)
            self.assertIn("untracked", checker.UNVERIFIED[0])
            self.assertEqual(run.call_count, 4)


if __name__ == "__main__":
    unittest.main()
