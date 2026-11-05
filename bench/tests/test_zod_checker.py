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

CHECKER = Path(__file__).resolve().parents[1] / "tasks/zod-int-bounds/check.py"


class ZodCheckerTest(unittest.TestCase):
    def test_bigint_timeout_is_only_diagnostic(self):
        spec = importlib.util.spec_from_file_location("zod_checker", CHECKER)
        checker = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(checker)
        output = io.StringIO()
        with mock.patch.object(checker.subprocess, "run", side_effect=subprocess.TimeoutExpired("vitest", 20)), \
                contextlib.redirect_stdout(output):
            checker.diagnose_bigint(Path("."), Path("bigint.test.ts"))
        self.assertIn("related pre-existing bigint case: could not check", output.getvalue())

    def test_committed_tests_and_hidden_tests_use_separate_checkouts(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / "work"
            work.mkdir()
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
            tests = work / "packages/zod/src/v4/classic/tests"
            tests.mkdir(parents=True)
            target = tests / "to-json-schema.test.ts"
            target.write_text("base test\n")
            (tests / "bigint.test.ts").write_text("base bigint test\n")
            source = work / "packages/zod/source.ts"
            source.write_text("base source\n")
            git("add", "packages")
            git("commit", "-qm", "base")
            env["BENCH_HEAD_SHA"] = git("rev-parse", "HEAD")
            binary = root / "bin"
            binary.mkdir()
            pnpm = binary / "pnpm"
            pnpm.write_text('#!/bin/sh\nmkdir -p node_modules/.bin\ncp "$FAKE_VITEST" node_modules/.bin/vitest\n')
            pnpm.chmod(0o755)
            vitest = root / "vitest"
            vitest.write_text(
                '#!/bin/sh\n'
                'case "$*" in *bigint.test.ts*) exit "${FAIL_BIGINT:-0}";; esac\n'
                'for arg in "$@"; do case "$arg" in --outputFile=*) report=${arg#--outputFile=};; esac; done\n'
                'hash=$(git hash-object packages/zod/src/v4/classic/tests/to-json-schema.test.ts)\n'
                'printf "%s|%s\\n" "$PWD" "$hash" >> "$TRACE"\n'
                'if [ "${FAIL_AGENT:-}" = 1 ] && [ "$hash" = "$AGENT_HASH" ]; then exit 1; fi\n'
                'if [ "${REQUIRE_HELPER:-}" = 1 ] && [ ! -f packages/zod/helper.ts ]; then exit 1; fi\n'
                'if [ -n "${TEST_REPORT:-}" ]; then printf "%s\\n" "$TEST_REPORT" > "$report";\n'
                'else printf "{\\"numPassedTests\\":1,\\"numTotalTests\\":1,\\"numFailedTests\\":0}\\n" > "$report"; fi\n'
            )
            vitest.chmod(0o755)
            trace = root / "trace"
            env.update({"PATH": f"{binary}{os.pathsep}{env['PATH']}", "FAKE_VITEST": str(vitest),
                        "TRACE": str(trace)})

            def check():
                return subprocess.run([sys.executable, str(CHECKER)], cwd=work, env=env,
                                      capture_output=True, text=True, check=False)

            self.assertIn("nothing was committed", check().stdout)
            source.write_text("fixed source\n")
            git("add", "packages/zod/source.ts")
            git("commit", "-qm", "fix")
            self.assertIn("no committed Zod test file was changed", check().stdout)
            target.write_text("agent test\n")
            git("add", str(target.relative_to(work)))
            git("commit", "-qm", "test")
            env["AGENT_HASH"] = git("hash-object", str(target.relative_to(work)))
            trace.write_text("")
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            entries = [line.split("|", 1) for line in trace.read_text().splitlines()]
            self.assertEqual(len(entries), 2)
            self.assertEqual(entries[0][1], env["AGENT_HASH"])
            self.assertNotEqual(entries[1][1], env["AGENT_HASH"])
            self.assertNotEqual(entries[0][0], entries[1][0])
            self.assertTrue(all(not Path(path).exists() for path, _ in entries))
            self.assertEqual(target.read_text(), "agent test\n")
            env["FAIL_BIGINT"] = "1"
            result = check()
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("related pre-existing bigint case: failed", result.stdout)
            del env["FAIL_BIGINT"]
            env["FAIL_AGENT"] = "1"
            self.assertIn("committed tests failed", check().stdout)
            del env["FAIL_AGENT"]
            (work / "packages/zod/helper.ts").write_text("untracked\n")
            env["REQUIRE_HELPER"] = "1"
            self.assertNotEqual(check().returncode, 0)
            del env["REQUIRE_HELPER"]
            env["TEST_REPORT"] = '{"numPassedTests":0,"numTotalTests":0,"numFailedTests":0}'
            self.assertIn("ran no tests", check().stdout)
            env["TEST_REPORT"] = '{"numPassedTests":0,"numTotalTests":1,"numFailedTests":0}'
            self.assertEqual(check().returncode, 3)
            env["TEST_REPORT"] = '{"numPassedTests":1,"numTotalTests":2,"numFailedTests":1}'
            self.assertIn("reported 1 failing tests", check().stdout)
            del env["TEST_REPORT"]
            source.write_text("uncommitted\n")
            self.assertIn("tracked changes were left uncommitted", check().stdout)
            self.assertEqual(target.read_text(), "agent test\n")


if __name__ == "__main__":
    unittest.main()
