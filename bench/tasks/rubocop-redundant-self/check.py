import json
import os
import pwd
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit

SPEC = "spec/rubocop/cop/style/redundant_self_spec.rb"
DEADLINE = time.monotonic() + 120


def run(command, cwd, env, *, text=None, timeout=45):
    remaining = min(timeout, DEADLINE - time.monotonic())
    if remaining <= 0:
        UNVERIFIED.append(f"time budget exhausted before {command[0]}")
        return None
    try:
        proc = subprocess.Popen(command, cwd=cwd, env=env, stdin=subprocess.PIPE if text is not None else subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        try:
            stdout, stderr = proc.communicate(text, timeout=remaining)
        except BaseException as error:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = proc.communicate()
            if isinstance(error, subprocess.TimeoutExpired):
                for output in (stdout, stderr):
                    if output:
                        print(output, end="" if output.endswith("\n") else "\n")
            raise
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"could not run {command[0]}: {error}")
        return None
    return subprocess.CompletedProcess(command, proc.returncode, stdout, stderr)


def phase(spec):
    try:
        source = tomllib.loads((Path(__file__).parent / "task.toml").read_text())["source"]["git"]
        if source.startswith("~/"):
            source = str(Path(pwd.getpwuid(os.getuid()).pw_dir) / source[2:])
        bundle = Path(source) / "vendor/bundle"
    except (OSError, KeyError, TypeError, AttributeError, tomllib.TOMLDecodeError) as error:
        UNVERIFIED.append(f"cannot find the task's pinned Ruby dependencies: {error}")
        return
    if not bundle.is_dir():
        UNVERIFIED.append("the task's pinned Ruby dependencies are unavailable")
        return
    parent = "/private/tmp" if Path("/private/tmp").is_dir() else "/tmp"
    with tempfile.TemporaryDirectory(prefix="rubocop-check-", dir=parent) as directory:
        checkout = Path(directory) / "work"
        created = None
        try:
            created = run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                          Path.cwd(), os.environ.copy(), timeout=15)
            if created is None:
                return
            if created.returncode:
                UNVERIFIED.append(f"could not create committed checkout: {created.stderr}")
                return
            (checkout / "vendor").mkdir(exist_ok=True)
            copied = run(["cp", "-R", str(bundle), str(checkout / "vendor/bundle")], checkout, os.environ.copy())
            if copied is None:
                return
            if copied.returncode:
                UNVERIFIED.append(f"could not copy pinned dependencies: {copied.stderr}")
                return
            try:
                shutil.copyfile(Path(__file__).parent / "Gemfile.lock", checkout / "Gemfile.lock")
            except OSError as error:
                UNVERIFIED.append(f"could not copy the task's dependency lockfile: {error}")
                return
            env = os.environ.copy()
            env.update({"BUNDLE_PATH": str(checkout / "vendor/bundle"), "BUNDLE_APP_CONFIG": str(checkout / ".bundle"),
                        "BUNDLE_FROZEN": "true", "BUNDLE_DISABLE_SHARED_GEMS": "true"})
            if spec:
                result = run(["bundle", "exec", "rspec", SPEC], checkout, env)
                if result is None:
                    return
                output = result.stdout + result.stderr
                expect(result.returncode == 0, f"committed spec failed:\n{output}")
                counts = re.search(r"(\d+) examples?, (\d+) failures?(?:, (\d+) pending)?", output)
                if not counts:
                    UNVERIFIED.append("the committed spec did not report its test count")
                elif int(counts.group(1)) == 0:
                    expect(False, "the committed spec ran no examples")
                elif int(counts.group(1)) == int(counts.group(3) or 0):
                    UNVERIFIED.append("the committed spec marked every example pending")
                elif int(counts.group(2)):
                    expect(False, f"the committed spec reported failures:\n{output}")
            else:
                for source, expected in [("self.a, *self.b = c", 0), ("*self.a, self.b = c", 0),
                                         ("a, (self.b, *self.c) = d", 0), ("a, b = *self.c", 1), ("self.c", 1)]:
                    result = run(["bundle", "exec", "rubocop", "--config", "/dev/null", "--only", "Style/RedundantSelf",
                                  "--format", "json", "--cache", "false", "--stdin", "case.rb"], checkout, env,
                                 text=source + "\n")
                    if result is None:
                        return
                    try:
                        doc = json.loads(result.stdout)
                        offenses = [o for f in doc["files"] for o in f["offenses"]
                                    if o["cop_name"] == "Style/RedundantSelf"]
                        expect(len(offenses) == expected and result.returncode in (0, 1),
                               f"{source}: expected {expected} offenses, got {len(offenses)} (exit {result.returncode})")
                    except (ValueError, KeyError, TypeError):
                        expect(False, f"{source}: RuboCop did not produce a usable report (exit {result.returncode}): {result.stderr}")
        finally:
            try:
                removed = subprocess.run(["git", "worktree", "remove", "--force", str(checkout)],
                                         capture_output=True, text=True, timeout=30, check=False)
                if removed.returncode and created is not None and created.returncode == 0:
                    UNVERIFIED.append(f"could not remove committed checkout: {removed.stderr}")
            except (OSError, subprocess.TimeoutExpired) as error:
                UNVERIFIED.append(f"could not remove committed checkout: {error}")


def main():
    files = committed_files()
    expect(new_commit(), "nothing was committed")
    expect(SPEC in files and Path(SPEC).is_file() and not Path(SPEC).is_symlink(), "no regression spec was committed")
    expect(any(p.startswith("changelog/") and p.endswith(".md") for p in files), "no changelog entry was committed")
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    if SPEC in files and Path(SPEC).is_file() and not Path(SPEC).is_symlink():
        phase(spec=True)
    phase(spec=False)
    finish()


if __name__ == "__main__":
    main()
