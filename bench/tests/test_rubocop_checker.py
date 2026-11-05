import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

TASKS = Path(__file__).resolve().parents[1] / "tasks"


class RubocopCheckerTest(unittest.TestCase):
    @unittest.skipUnless(os.name == "posix", "process groups are required")
    def test_timed_out_check_stops_its_child(self):
        spec = importlib.util.spec_from_file_location("rubocop_checker", TASKS / "rubocop-redundant-self/check.py")
        checker = importlib.util.module_from_spec(spec)
        with mock.patch.dict(sys.modules), mock.patch.object(sys, "path", sys.path.copy()):
            sys.modules.pop("_check", None)
            spec.loader.exec_module(checker)
            with tempfile.TemporaryDirectory() as directory:
                marker = Path(directory) / "survived"
                result = checker.run(["/bin/sh", "-c", f'(sleep 1; touch "{marker}") & wait'],
                                     Path(directory), os.environ.copy(), timeout=0.1)
                self.assertIsNone(result)
                subprocess.run(["sleep", "1"], check=True)
                self.assertFalse(marker.exists())

    def test_committed_spec_and_cli_run_in_separate_checkouts(self):
        with tempfile.TemporaryDirectory(prefix="rubocop-checker-test-", dir="/tmp") as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
            task = root / "tasks/rubocop-redundant-self"
            task.mkdir(parents=True)
            shutil.copyfile(TASKS / "_check.py", task.parent / "_check.py")
            shutil.copyfile(TASKS / "rubocop-redundant-self/check.py", task / "check.py")
            (task / "Gemfile.lock").write_text("lock\n")
            source = root / "source"
            (source / "vendor/bundle").mkdir(parents=True)
            (task / "task.toml").write_text(f'[source]\ngit = {json.dumps(str(source))}\n')
            redirected = {"GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE",
                          "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"}
            env = {key: value for key, value in os.environ.items() if key not in redirected}
            env.update({"GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_NOSYSTEM": "1",
                        "GIT_AUTHOR_NAME": "Bench", "GIT_AUTHOR_EMAIL": "bench@example.invalid",
                        "GIT_COMMITTER_NAME": "Bench", "GIT_COMMITTER_EMAIL": "bench@example.invalid"})

            def git(*args):
                return subprocess.run(["git", *args], cwd=work, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()

            git("init", "-q")
            spec = work / "spec/rubocop/cop/style/redundant_self_spec.rb"
            spec.parent.mkdir(parents=True)
            spec.write_text("base spec\n")
            (work / "source.rb").write_text("before\n")
            git("add", "spec", "source.rb")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            binary = root / "bin"
            binary.mkdir()
            bundle = binary / "bundle"
            bundle.write_text(
                '#!/bin/sh\n'
                'case "$*" in *rspec*)\n'
                'printf "%s\\n" "$PWD" >> "$TRACE"\n'
                'if [ "${REQUIRE_HELPER:-}" = 1 ] && [ ! -f helper.rb ]; then exit 1; fi\n'
                'if [ "${FAIL_SPEC:-}" = 1 ]; then echo "1 example, 1 failure"; exit 1; fi\n'
                'echo "${SPEC_SUMMARY:-1 example, 0 failures}"; exit 0;;\n'
                '*) printf "%s\\n" "$PWD" >> "$TRACE"; read -r input\n'
                'case "$input" in "a, b = *self.c"|"self.c") '
                'echo \'{"files":[{"offenses":[{"cop_name":"Style/RedundantSelf"}]}]}\'; exit 1;;\n'
                '*) echo \'{"files":[{"offenses":[]}]}\'; exit 0;; esac;; esac\n'
            )
            bundle.chmod(0o755)
            trace = root / "trace"
            env.update({"PATH": f"{binary}{os.pathsep}{env['PATH']}", "TRACE": str(trace)})

            def check(extra=None):
                return subprocess.run([sys.executable, str(task / "check.py")], cwd=work, env={**env, **(extra or {})},
                                      capture_output=True, text=True, check=False)

            self.assertIn("nothing was committed", check().stdout)
            (work / "source.rb").write_text("fixed\n")
            git("add", "source.rb")
            git("commit", "-qm", "fix")
            self.assertIn("no regression spec was committed", check().stdout)
            spec.write_text("agent spec\n")
            (work / "changelog").mkdir()
            (work / "changelog/fix.md").write_text("fix\n")
            git("add", "spec", "changelog")
            git("commit", "-qm", "spec")
            trace.write_text("")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            redirected_env = {"GIT_DIR": str(root / "missing"), "GIT_WORK_TREE": str(root),
                              "GIT_INDEX_FILE": str(root / "missing-index")}
            redirected_result = check(redirected_env)
            self.assertEqual(redirected_result.returncode, 0, redirected_result.stdout + redirected_result.stderr)
            locations = trace.read_text().splitlines()[:6]
            self.assertEqual(len(locations), 6)
            self.assertNotEqual(locations[0], locations[1])
            self.assertEqual(len(set(locations[1:])), 1)
            self.assertTrue(all(not Path(path).exists() for path in locations))
            self.assertEqual(spec.read_text(), "agent spec\n")
            self.assertEqual(len(git("worktree", "list", "--porcelain").split("worktree ")) - 1, 1)
            env["FAIL_SPEC"] = "1"
            self.assertIn("committed spec failed", check().stdout)
            del env["FAIL_SPEC"]
            (work / "helper.rb").write_text("untracked\n")
            env["REQUIRE_HELPER"] = "1"
            self.assertNotEqual(check().returncode, 0)
            del env["REQUIRE_HELPER"]
            env["SPEC_SUMMARY"] = "0 examples, 0 failures"
            self.assertIn("ran no examples", check().stdout)
            env["SPEC_SUMMARY"] = "2 examples, 0 failures, 1 pending"
            self.assertEqual(check().returncode, 3)
            env["SPEC_SUMMARY"] = "missing summary"
            self.assertEqual(check().returncode, 3)
            del env["SPEC_SUMMARY"]
            (work / "source.rb").write_text("uncommitted\n")
            self.assertIn("tracked changes were left uncommitted", check().stdout)
            self.assertEqual(spec.read_text(), "agent spec\n")


if __name__ == "__main__":
    unittest.main()
