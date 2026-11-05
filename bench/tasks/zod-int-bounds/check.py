import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit


def run(command: list[str], cwd: Path, timeout: int) -> subprocess.CompletedProcess | None:
    try:
        env = os.environ.copy()
        if Path("/tmp").is_dir():
            env["TMPDIR"] = "/tmp"
        return subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        if isinstance(error, subprocess.TimeoutExpired):
            for stream in (error.stdout, error.stderr):
                if stream:
                    text = stream.decode(errors="replace") if isinstance(stream, bytes) else stream
                    print(text, end="" if text.endswith("\n") else "\n")
        UNVERIFIED.append(f"could not run {command[0]}: {error}")
        return None


def diagnose_bigint(checkout: Path, related: Path) -> None:
    try:
        diagnostic = subprocess.run(["node_modules/.bin/vitest", "run", str(related)],
                                    cwd=checkout, capture_output=True, text=True, timeout=20, check=False)
        print(f"related pre-existing bigint case: {'passed' if diagnostic.returncode == 0 else 'failed'}")
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"related pre-existing bigint case: could not check ({error})")


def test_checkout(hidden: bool, tests: list[str]) -> None:
    with tempfile.TemporaryDirectory(prefix="zod-check-") as directory:
        checkout = Path(directory)
        created = run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"], Path.cwd(), 30)
        if created is None:
            return
        if created.returncode:
            UNVERIFIED.append(f"could not create test checkout: {created.stderr}")
            return
        try:
            for command in (["pnpm", "install", "--frozen-lockfile", "--ignore-scripts"],
                            ["pnpm", "build"]):
                setup = run(command, checkout, 180)
                if setup is None:
                    return
                if setup.returncode:
                    UNVERIFIED.append(f"checkout setup failed: {' '.join(command)}\n{setup.stdout}{setup.stderr}")
                    return
            changed = run(["git", "status", "--porcelain", "--untracked-files=no"], checkout, 30)
            if changed is None:
                return
            if changed.returncode or changed.stdout.strip():
                UNVERIFIED.append(f"checkout setup changed tracked files: {changed.stdout}{changed.stderr}")
                return
            if hidden:
                target = checkout / "packages/zod/src/v4/classic/tests/to-json-schema.test.ts"
                if not target.exists() and not target.is_symlink():
                    expect(False, "the hidden-test target was removed from the committed tree")
                    return
                if not target.is_file() or target.is_symlink() or not target.resolve().is_relative_to(checkout.resolve()):
                    UNVERIFIED.append("hidden-test target is not a regular file in the checkout")
                    return
                shutil.copyfile(Path(__file__).parent / "hidden/to-json-schema.test.ts", target)
                tests = [str(target.relative_to(checkout))]
            report = checkout / "benchmark-vitest-report.json"
            result = run(["node_modules/.bin/vitest", "run", *tests, "--reporter=json",
                          f"--outputFile={report}"], checkout, 240)
            if result is None:
                return
            label = "hidden tests" if hidden else "committed tests"
            details = []
            try:
                counts = json.loads(report.read_text())
                passed = counts["numPassedTests"]
                total = counts["numTotalTests"]
                failed = counts["numFailedTests"]
                if any(not isinstance(count, int) or isinstance(count, bool) or count < 0 for count in (passed, total, failed)):
                    raise TypeError("test counts are not integers")
                for suite in counts.get("testResults", []):
                    for assertion in suite.get("assertionResults", []):
                        if assertion.get("status") == "failed":
                            details.append(f"{assertion.get('fullName', '(unnamed test)')}:\n"
                                           + "\n".join(assertion.get("failureMessages") or []))
            except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
                UNVERIFIED.append(f"{label} did not report test counts: {error}")
            else:
                expect(failed == 0, f"{label} reported {failed} failing tests:\n" + "\n".join(details))
                if total == 0:
                    expect(False, f"{label} ran no tests")
                elif passed == 0 and result.returncode == 0:
                    UNVERIFIED.append(f"{label} did not pass any tests")
            expect(result.returncode == 0, f"{label} failed:\n{result.stdout}{result.stderr}\n" + "\n".join(details))
            if hidden:
                related = checkout / "packages/zod/src/v4/classic/tests/bigint.test.ts"
                if related.is_file() and not related.is_symlink() and related.resolve().is_relative_to(checkout.resolve()):
                    shutil.copyfile(Path(__file__).parent / "hidden/bigint.test.ts", related)
                    diagnose_bigint(checkout, related)
        finally:
            removed = run(["git", "worktree", "remove", "--force", str(checkout)], Path.cwd(), 30)
            if removed is not None and removed.returncode:
                UNVERIFIED.append(f"could not remove test checkout: {removed.stderr}")


def main() -> None:
    expect(new_commit(), "nothing was committed")
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    tests = sorted(path for path in committed_files()
                   if path.startswith("packages/") and path.endswith((".test.ts", ".spec.ts"))
                   and Path(path).is_file() and not Path(path).is_symlink()
                   and subprocess.run(["git", "cat-file", "-t", f"HEAD:{path}"],
                                      capture_output=True, text=True, check=False).stdout.strip() == "blob")
    expect(bool(tests), "no committed Zod test file was changed")
    if tests:
        test_checkout(hidden=False, tests=tests)
    test_checkout(hidden=True, tests=tests)
    finish()


if __name__ == "__main__":
    main()
