import json
import os
import shutil
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path

import import_swebench as imp
from jbench import tasks

sys.path.insert(0, str(tasks.TASKS_DIR))
import _swebench  # noqa: E402

GIT_ENV = {**os.environ, **tasks.GIT_ENV}


def git(cwd, *args):
    return subprocess.run(["git", *args], cwd=cwd, env=GIT_ENV, check=True, capture_output=True, text=True).stdout.strip()


FIX = """\
diff --git a/calc.py b/calc.py
--- a/calc.py
+++ b/calc.py
@@ -1,2 +1,2 @@
 def add(a, b):
-    return a - b
+    return a + b
diff --git a/legacy.py b/legacy.py
deleted file mode 100644
--- a/legacy.py
+++ /dev/null
@@ -1 +0,0 @@
-OLD = True
"""

TESTS = """\
diff --git a/test_calc.py b/test_calc.py
--- a/test_calc.py
+++ b/test_calc.py
@@ -1,6 +1,9 @@
 import unittest
 from calc import add

 class CalcTest(unittest.TestCase):
     def test_zero(self):
         self.assertEqual(add(0, 0), 0)
+
+    def test_add(self):
+        self.assertEqual(add(2, 3), 5)
diff --git a/old_test.py b/old_test.py
deleted file mode 100644
--- a/old_test.py
+++ /dev/null
@@ -1 +0,0 @@
-x = 1
"""


def record(**kw):
    return {
        "instance_id": "acme__calc-7",
        "repo": "acme/calc",
        "base_commit": "",
        "problem_statement": 'add() subtracts 🙃\r\n\r\nIt says """2 - 3""" with a \\ in it.',
        "patch": FIX,
        "test_patch": TESTS,
        "FAIL_TO_PASS": '["test_add (test_calc.CalcTest)"]',
        "PASS_TO_PASS": ["test_zero (test_calc.CalcTest)"],
        "version": "1.0",
        "created_at": "2020-01-01T00:00:00Z",
        **kw,
    }


class RecordsTest(unittest.TestCase):
    def test_lists_from_json_strings(self):
        r = imp.normalize(record(base_commit="abc1234"))
        self.assertEqual(r["FAIL_TO_PASS"], ["test_add (test_calc.CalcTest)"])
        self.assertEqual(r["PASS_TO_PASS"], ["test_zero (test_calc.CalcTest)"])

    def test_unsafe_ids(self):
        for bad in ({"instance_id": "../../etc"}, {"base_commit": "--upload-pack=x"}, {"repo": "acme/../x"}):
            with self.assertRaises(ValueError, msg=bad):
                imp.normalize(record(**{"base_commit": "abc1234", **bad}))

    def test_missing_field(self):
        with self.assertRaisesRegex(ValueError, "no patch"):
            imp.normalize(record(base_commit="abc1234", patch=""))

    def test_multi_swe_bench_fields(self):
        r = imp.normalize({
            "org": "acme", "repo": "calc", "number": 7, "base": {"sha": "abc1234"},
            "resolved_issues": [{"title": "add() subtracts", "body": "It does."}],
            "fix_patch": FIX, "test_patch": TESTS,
            "f2p_tests": {"calc::add": {}}, "p2p_tests": {"calc::zero": {}},
        })
        self.assertEqual((r["instance_id"], r["repo"], r["base_commit"]), ("acme__calc-7", "acme/calc", "abc1234"))
        self.assertEqual(r["problem_statement"], "add() subtracts\n\nIt does.")
        self.assertEqual((r["FAIL_TO_PASS"], r["PASS_TO_PASS"]), (["calc::add"], ["calc::zero"]))

    def test_file_formats(self):
        with tempfile.TemporaryDirectory() as tmp:
            rows = [record(base_commit="abc1234")]
            for name, text in (
                ("r.jsonl", "\n".join(json.dumps(r) for r in rows) + "\n"),
                ("r.json", json.dumps(rows)),
                ("rows.json", json.dumps({"rows": [{"row": r} for r in rows]})),
            ):
                path = Path(tmp) / name
                path.write_text(text)
                self.assertEqual([r["instance_id"] for r in imp.read_records(path)], ["acme__calc-7"], name)

    def test_toml_string_reads_back(self):
        text = 'a """quoted""" \\ path\r\nand a \x07 bell and a\ttab\nending in a quote "'
        parsed = tomllib.loads(f"s = {imp.toml_string(text)}")["s"]
        self.assertEqual(parsed, text.replace("\r\n", "\n"))

    def test_task_name(self):
        self.assertEqual(imp.task_name("django__django-11400"), "swebench-django-11400")
        self.assertEqual(imp.task_name("pylint-dev__pylint-4551"), "swebench-pylint-4551")


class CommandTest(unittest.TestCase):
    def test_django_labels(self):
        spec = imp.spec_for("django/django", "4.0")
        files = ["tests/forms_tests/tests/test_forms.py", "tests/queries/models.py", "tests/runtests.py",
                 "tests/delete/tests.py", "django/test/testcases.py"]
        self.assertEqual(imp.test_command(spec, {}, files)[-2:], ["forms_tests.tests.test_forms", "delete.tests"])

    def test_sympy_files(self):
        spec = imp.spec_for("sympy/sympy", "1.5")
        files = ["sympy/core/tests/test_basic.py", "sympy/core/basic.py", "doc/x.rst"]
        self.assertEqual(imp.test_command(spec, {}, files)[1:], ["bin/test", "-C", "--verbose", "sympy/core/tests/test_basic.py"])

    def test_pytest_files_from_node_ids(self):
        spec = imp.spec_for("pylint-dev/pylint", "2.9")
        rec = {"FAIL_TO_PASS": ["tests/a.py::test_x[1]"], "PASS_TO_PASS": ["tests/b.py::T::test_y", "tests/a.py::z"]}
        self.assertEqual(imp.test_command(spec, rec, [])[-2:], ["tests/a.py", "tests/b.py"])

    def test_python_versions(self):
        self.assertEqual(imp.spec_for("django/django", "3.0").python, "3.8")
        self.assertEqual(imp.spec_for("django/django", "4.2").python, "3.9")
        self.assertIsNone(imp.spec_for("acme/calc", "1.0"))


class LogTest(unittest.TestCase):
    def test_django(self):
        log = "\n".join([
            "test_a (m.tests.T) ... ok",
            "test_b (m.tests.T) ... FAIL",
            "test_c (m.tests.T)",
            "A docstring's first line. ... ERROR",
            "test_d (m.tests.T.test_d) ... skipped 'no db'",
            "test_e (m.tests.T) ... printed something",
            "ok",
        ])
        self.assertEqual(_swebench.parse_django(log), {
            "test_a (m.tests.T)": "pass", "test_b (m.tests.T)": "fail", "A docstring's first line.": "fail",
            "test_c (m.tests.T)": "fail", "test_d (m.tests.T.test_d)": "skip", "test_e (m.tests.T)": "pass",
        })

    def test_django_does_not_credit_an_interrupted_test_to_the_next_one(self):
        log = "test_interrupted (m.tests.T)\ntest_next (m.tests.T) ... ok\n"
        self.assertEqual(_swebench.parse_django(log), {"test_next (m.tests.T)": "pass"})

    def test_duplicate_names_keep_a_failure(self):
        log = "a/tests/test_x.py[1]\ntest_same F\nb/tests/test_y.py[1]\ntest_same ok\n"
        self.assertEqual(_swebench.parse_sympy(log), {"test_same": "fail"})
        log = "a/tests/test_x.py[1]\ntest_same s\nb/tests/test_y.py[1]\ntest_same ok\n"
        self.assertEqual(_swebench.parse_sympy(log), {"test_same": "skip"})

    def test_sympy(self):
        log = "sympy/core/tests/test_x.py[4] \ntest_a ok\ntest_b F\ntest_c E\ntest_d f\n"
        self.assertEqual(_swebench.parse_sympy(log), {"test_a": "pass", "test_b": "fail", "test_c": "fail", "test_d": "xfail"})

    def test_pytest(self):
        log = "\n".join([
            "PASSED tests/a.py::test_x[1]",
            "FAILED tests/a.py::test_y - AssertionError: no",
            "SKIPPED [1] tests/a.py:9: needs docutils",
            "XFAIL tests/a.py::test_z",
            "PASSED tests/a.py::test_w[a: int]",
        ])
        self.assertEqual(_swebench.parse_pytest(log), {
            "tests/a.py::test_x[1]": "pass", "tests/a.py::test_y": "fail", "tests/a.py::test_z": "xfail",
            "tests/a.py::test_w[a:": "pass",
        })

    def test_verdict(self):
        import contextlib
        import io

        spec = {"fail_to_pass": ["a", "b"], "pass_to_pass": ["c", "d", "e", "f"]}
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            problems = _swebench.verdict(spec, {"a": "pass", "b": "skip", "c": "fail",
                                               "d": "skip", "f": "xfail"})
        self.assertEqual(problems, ["FAIL_TO_PASS b: skip", "PASS_TO_PASS c: fail"])
        self.assertIn("d (skip)", output.getvalue())
        self.assertIn("e (not run or not named)", output.getvalue())
        self.assertIn("f (xfail)", output.getvalue())


class ImportTest(unittest.TestCase):
    """An instance of a tiny repository, imported, then checked unsolved and solved."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        repo = self.tmp / "clones" / "acme__calc"
        repo.mkdir(parents=True)
        (repo / "calc.py").write_text("def add(a, b):\n    return a - b\n")
        (repo / "test_calc.py").write_text(
            "import unittest\nfrom calc import add\n\nclass CalcTest(unittest.TestCase):\n"
            "    def test_zero(self):\n        self.assertEqual(add(0, 0), 0)\n"
        )
        (repo / "old_test.py").write_text("x = 1\n")
        (repo / "legacy.py").write_text("OLD = True\n")
        git(repo, "init", "-q", "-b", "main")
        git(repo, "add", "-A")
        git(repo, "commit", "-q", "-m", "base")
        self.base = git(repo, "rev-parse", "HEAD")
        self.out = self.tmp / "tasks"
        self.out.mkdir()
        for helper in ("_check.py", "_swebench.py"):
            shutil.copy(tasks.TASKS_DIR / helper, self.out / helper)
        spec = imp.Spec(None, "command", "exit", command=["{python}", "-m", "unittest", "-v", "test_calc"])
        self.task = imp.write_task(imp.normalize(record(base_commit=self.base)), self.out, self.tmp / "clones",
                                   self.tmp / "envs", "Test bench", spec)

    def test_files(self):
        self.assertEqual(self.task.name, "swebench-calc-7")
        meta = json.loads((self.task / "swebench.json").read_text())
        self.assertEqual(meta["test_files"], ["test_calc.py"])
        self.assertEqual(meta["deleted_test_files"], ["old_test.py"])
        self.assertEqual(meta["patch_files"], ["calc.py", "legacy.py"])
        self.assertIn("test_add", (self.task / "hidden" / "test_calc.py").read_text())
        self.assertEqual((self.task / "fix.patch").read_text(), FIX)
        raw = tomllib.loads((self.task / "task.toml").read_text())
        self.assertTrue(raw["opt_in"])
        self.assertEqual(raw["source"]["commit"], self.base)
        self.assertTrue(raw["prompt"].startswith('add() subtracts 🙃\n\nIt says """2 - 3""" with a \\ in it.'))
        self.assertTrue(raw["prompt"].endswith("Fix this in calc, with a test, and commit.\n"))
        self.assertNotIn("tools", raw)
        self.assertTrue(raw["description"].endswith("acme__calc-7: add() subtracts 🙃"))
        self.assertTrue(raw["source"]["git"].endswith("/clones/acme__calc"))

    def test_no_test_targets(self):
        spec = imp.spec_for("sympy/sympy", "1.5")
        with self.assertRaises(SystemExit):
            imp.write_task(imp.normalize(record(base_commit=self.base)), self.out, self.tmp / "clones",
                           self.tmp / "envs", "Test bench", spec)

    def test_check_fails_unsolved_and_passes_solved(self):
        work = self.tmp / "work"
        git(self.tmp, "clone", "-q", str(self.tmp / "clones" / "acme__calc"), str(work))
        # The fix keeps calc.py's size, so within one second a cached .pyc
        # of the unsolved file would still be used.
        env = {**GIT_ENV, "BENCH_HEAD_SHA": self.base, "PYTHONDONTWRITEBYTECODE": "1"}

        def run(script):
            return subprocess.run([sys.executable, "-I", str(self.task / script)], cwd=work, env=env,
                                  capture_output=True, text=True)

        unsolved = run("check.py")
        self.assertEqual(unsolved.returncode, 1, unsolved.stdout + unsolved.stderr)
        self.assertFalse((work / "old_test.py").exists())
        git(work, "checkout", "-q", "--", ".")
        self.assertEqual(run("solve.py").returncode, 0)
        solved = run("check.py")
        self.assertEqual(solved.returncode, 0, solved.stdout + solved.stderr)
        self.assertIn("committed: True", solved.stdout)
        # The gold patch's deletion is committed with the rest.
        self.assertNotIn("legacy.py", git(work, "ls-tree", "--name-only", "HEAD"))

    def test_check_rejects_a_runner_error_after_the_target_passes(self):
        work = self.tmp / "work"
        git(self.tmp, "clone", "-q", str(self.tmp / "clones" / "acme__calc"), str(work))
        meta_path = self.task / "swebench.json"
        meta = json.loads(meta_path.read_text())
        for log_format, name, passing in (
            ("django", "test_expected (case.Test)", "test_expected (case.Test) ... ok"),
            ("sympy", "test_expected", "test_expected ok"),
            ("pytest", "tests/test_case.py::test_expected", "PASSED tests/test_case.py::test_expected"),
        ):
            with self.subTest(log_format=log_format):
                meta["log_format"] = log_format
                meta["fail_to_pass"] = [name]
                meta["pass_to_pass"] = []
                meta["test_command"] = ["{python}", "-c",
                                        f"import sys; print({passing!r}); print('runner crashed'); sys.exit(2)"]
                meta_path.write_text(json.dumps(meta))
                proc = subprocess.run([sys.executable, "-I", str(self.task / "check.py")], cwd=work,
                                      env={**GIT_ENV, "BENCH_HEAD_SHA": self.base, "TMPDIR": str(self.tmp)},
                                      capture_output=True, text=True)
                self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
                self.assertIn("runner crashed", proc.stdout)

    def test_check_rejects_an_expected_failure_as_a_fixed_test(self):
        work = self.tmp / "work"
        git(self.tmp, "clone", "-q", str(self.tmp / "clones" / "acme__calc"), str(work))
        meta_path = self.task / "swebench.json"
        meta = json.loads(meta_path.read_text())
        for log_format, name, xfail in (
            ("django", "test_expected (case.Test)", "test_expected (case.Test) ... expected failure"),
            ("sympy", "test_expected", "test_expected f"),
            ("pytest", "tests/test_case.py::test_expected", "XFAIL tests/test_case.py::test_expected - known bug"),
        ):
            with self.subTest(log_format=log_format):
                meta["log_format"] = log_format
                meta["fail_to_pass"] = [name]
                meta["pass_to_pass"] = []
                meta["test_command"] = ["{python}", "-c", f"print({xfail!r})"]
                meta_path.write_text(json.dumps(meta))
                proc = subprocess.run([sys.executable, "-I", str(self.task / "check.py")], cwd=work,
                                      env={**GIT_ENV, "BENCH_HEAD_SHA": self.base, "TMPDIR": str(self.tmp)},
                                      capture_output=True, text=True)
                self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
                self.assertIn("FAIL_TO_PASS", proc.stdout)

    def test_check_rejects_a_failure_despite_a_zero_exit(self):
        work = self.tmp / "work"
        git(self.tmp, "clone", "-q", str(self.tmp / "clones" / "acme__calc"), str(work))
        meta_path = self.task / "swebench.json"
        meta = json.loads(meta_path.read_text())
        for log_format, name, log in (
            ("django", "test_expected (case.Test)",
             "test_expected (case.Test) ... ok\ntest_other (case.Test) ... FAIL"),
            ("sympy", "test_expected", "test_expected ok\ntest_other F"),
            ("pytest", "tests/test_case.py::test_expected",
             "PASSED tests/test_case.py::test_expected\nFAILED tests/test_case.py::test_other - no"),
        ):
            with self.subTest(log_format=log_format):
                meta["log_format"] = log_format
                meta["fail_to_pass"] = [name]
                meta["pass_to_pass"] = []
                meta["test_command"] = ["{python}", "-c", f"print({log!r})"]
                meta_path.write_text(json.dumps(meta))
                proc = subprocess.run([sys.executable, "-I", str(self.task / "check.py")], cwd=work,
                                      env={**GIT_ENV, "BENCH_HEAD_SHA": self.base, "TMPDIR": str(self.tmp)},
                                      capture_output=True, text=True)
                self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
                self.assertIn("test_other", proc.stdout)

    def test_a_timeout_keeps_what_the_tests_printed(self):
        meta_path = self.task / "swebench.json"
        meta = json.loads(meta_path.read_text())
        meta["test_command"] = ["{python}", "-c", "import sys, time; print('started', flush=True); time.sleep(30)"]
        meta["test_timeout_s"] = 2
        meta_path.write_text(json.dumps(meta))
        work = self.tmp / "work"
        git(self.tmp, "clone", "-q", str(self.tmp / "clones" / "acme__calc"), str(work))
        proc = subprocess.run([sys.executable, "-I", str(self.task / "check.py")], cwd=work,
                              env={**GIT_ENV, "BENCH_HEAD_SHA": self.base, "TMPDIR": str(self.tmp)},
                              capture_output=True, text=True)
        self.assertEqual(proc.returncode, 1)
        self.assertIn("timed out", proc.stdout)
        self.assertIn("started", proc.stdout)
        self.assertIn("started", (self.tmp / "acme__calc-7.test.log").read_text())


class ToolTest(unittest.TestCase):
    def test_a_path_replaces_a_default_tool(self):
        with tempfile.TemporaryDirectory() as tmp:
            bin_dir = Path(tmp) / "bin"
            bin_dir.mkdir()
            (bin_dir / "python3").symlink_to(sys.executable)
            task = tasks.Task(name="t", dir=Path(tmp), description="", prompt="", chore="", rules=[], timeout_s=1,
                              tools={"python3": sys.executable})
            task.agent_env({"PATH": str(bin_dir)}, bin_dir, Path(tmp))
            self.assertIn(sys.executable, (bin_dir / "python3").read_text())
            task.tools = {"python3": str(Path(tmp) / "missing")}
            with self.assertRaisesRegex(RuntimeError, "does not exist"):
                task.agent_env({"PATH": str(bin_dir)}, bin_dir, Path(tmp))


if __name__ == "__main__":
    unittest.main()
