import os
import re
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, dirty, expect, finish, new_commit

DEADLINE = time.monotonic() + 780


def run(command, cwd, env):
    remaining = DEADLINE - time.monotonic()
    if remaining <= 0:
        UNVERIFIED.append(f"time budget exhausted before {command[0]}")
        return None
    try:
        proc = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = proc.communicate(timeout=remaining)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = proc.communicate()
            UNVERIFIED.append(f"timed out running {command[0]}:\n{stdout}\n{stderr}")
            return None
    except OSError as error:
        UNVERIFIED.append(f"could not run {command[0]}: {error}")
        return None
    return subprocess.CompletedProcess(command, proc.returncode, stdout, stderr)


def main():
    for key in ("GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_PREFIX"):
        os.environ.pop(key, None)
    expect(new_commit(), "the fix was not committed")
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    changes = subprocess.run(["git", "diff", "--unified=0", os.environ["BENCH_HEAD_SHA"], "HEAD", "--", "src", "tests"],
                             capture_output=True, text=True, check=True).stdout
    added_test = re.search(r"^\+\s*#\[(?:test|test_case|rstest)(?:\(|\])", changes, re.MULTILINE)
    if not added_test:
        UNVERIFIED.append("no new test marker was found; changes to existing tests need separate review")
    if not new_commit() or dirty():
        finish()

    with tempfile.TemporaryDirectory(prefix="fd-check-") as tmp:
        checkout = Path(tmp) / "work"
        created = None
        try:
            created = run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                          Path.cwd(), os.environ.copy())
            if created is None:
                return finish()
            if created.returncode:
                UNVERIFIED.append(f"could not create committed checkout: {created.stderr}")
                return finish()
            env = os.environ.copy()
            env["CARGO_TARGET_DIR"] = str(checkout / "target")
            build = run(["cargo", "build", "--locked", "--bin", "fd"], checkout, env)
            if build is None:
                return finish()
            expect(build.returncode == 0, f"build failed:\n{build.stdout}\n{build.stderr}")
            if build.returncode == 0:
                with tempfile.TemporaryDirectory(prefix="fd-cases-") as cases_dir:
                    directory = Path(cases_dir)
                    for name in ("a.txt", ".hidden.txt", "ignored.txt"):
                        (directory / name).touch()
                    (directory / ".ignore").write_text("ignored.txt\n")
                    case_env = {**env, "HOME": cases_dir, "XDG_CONFIG_HOME": str(directory / "config")}
                    cases = [
                        (["-u", "--ignore"], {"a.txt", ".hidden.txt", ".ignore"}),
                        (["-u", "--no-hidden"], {"a.txt", "ignored.txt"}),
                        (["--ignore", "-u"], {"a.txt", ".hidden.txt", ".ignore", "ignored.txt"}),
                        (["--no-hidden", "-u"], {"a.txt", ".hidden.txt", ".ignore", "ignored.txt"}),
                        (["-u", "--ignore", "--no-hidden"], {"a.txt"}),
                        (["--ignore", "--no-hidden", "-u"], {"a.txt", ".hidden.txt", ".ignore", "ignored.txt"}),
                        (["-u", "--ignore", "-u"], {"a.txt", ".hidden.txt", ".ignore", "ignored.txt"}),
                    ]
                    for flags, want in cases:
                        result = run([str(checkout / "target/debug/fd"), "--color=never", "--type", "f", *flags],
                                     directory, case_env)
                        if result is None:
                            return finish()
                        got = set(result.stdout.splitlines())
                        expect(result.returncode == 0 and got == want,
                               f"{flags}: expected {sorted(want)}, got {sorted(got)}; stderr: {result.stderr}")
                tests = run(["cargo", "test", "--locked", "--all-targets"], checkout, env)
                if tests is None:
                    return finish()
                output = tests.stdout + tests.stderr
                expect(tests.returncode == 0, f"committed tests failed:\n{output}")
                summaries = re.findall(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
                if not summaries:
                    UNVERIFIED.append("cargo did not report test counts")
                elif not sum(int(passed) for passed, _, _ in summaries):
                    expect(False, "cargo ran no tests")
                elif any(int(failed) for _, failed, _ in summaries):
                    expect(False, f"cargo reported test failures:\n{output}")
                elif any(int(ignored) for _, _, ignored in summaries):
                    UNVERIFIED.append("cargo reported ignored tests")
                else:
                    print(f"committed tests: {sum(int(passed) for passed, _, _ in summaries)} passed")
        finally:
            try:
                removed = subprocess.run(["git", "worktree", "remove", "--force", str(checkout)],
                                         capture_output=True, text=True, timeout=60, check=False)
                if removed.returncode and created is not None and created.returncode == 0:
                    print(f"warning: could not remove committed checkout: {removed.stderr}", file=sys.stderr)
            except (OSError, subprocess.TimeoutExpired) as error:
                print(f"warning: could not remove committed checkout: {error}", file=sys.stderr)
    finish()


if __name__ == "__main__":
    main()
