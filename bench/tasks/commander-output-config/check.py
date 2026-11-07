import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit  # noqa: E402


def run_tests(checkout: Path, paths: list[str], label: str) -> None:
    report = checkout / "jouet-bench-jest-report.json"
    try:
        proc = subprocess.run(
            ["node_modules/.bin/jest", "--runInBand", "--runTestsByPath", *paths,
             "--json", f"--outputFile={report}"],
            cwd=checkout, capture_output=True, text=True, timeout=240, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"{label} could not run: {error}")
        return
    try:
        result = json.loads(report.read_text())
        total = result["numTotalTests"]
        passed = result["numPassedTests"]
        failed = result["numFailedTests"]
        pending = result["numPendingTests"]
        if any(type(value) is not int or value < 0 for value in (total, passed, failed, pending)):
            raise ValueError("invalid test counts")
        if passed + failed + pending != total:
            raise ValueError("inconsistent test counts")
        suites = result["testResults"]
        if not isinstance(suites, list):
            raise ValueError("missing test suites")
        found = set()
        statuses = []
        for suite in suites:
            path = Path(suite["name"]).resolve().relative_to(checkout.resolve()).as_posix()
            cases = suite["assertionResults"]
            if not isinstance(cases, list) or not cases or path in found:
                raise ValueError("empty or duplicate test suite")
            found.add(path)
            statuses.extend(case["status"] for case in cases)
        if (found != set(paths) or len(statuses) != total
                or statuses.count("passed") != passed or statuses.count("failed") != failed
                or statuses.count("pending") != pending):
            raise ValueError("missing test suites or assertions")
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        UNVERIFIED.append(f"{label} did not produce a complete Jest report: {error}")
        return
    expect(failed == 0, f"{label} reported {failed} failing tests:\n{proc.stdout}{proc.stderr}")
    expect(total > 0, f"{label} ran no tests")
    expect(proc.returncode == 0, f"{label} failed:\n{proc.stdout}{proc.stderr}")
    if pending:
        UNVERIFIED.append(f"{label} reported {pending} skipped tests")
    if total and not passed and not failed:
        UNVERIFIED.append(f"{label} did not pass any tests")


def check_phase(tests: list[str], *, hidden: bool) -> None:
    with tempfile.TemporaryDirectory(prefix="commander-check-") as directory:
        checkout = Path(directory)
        try:
            created = subprocess.run(["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                                     capture_output=True, text=True, timeout=30, check=False)
        except (OSError, subprocess.TimeoutExpired) as error:
            UNVERIFIED.append(f"could not create test checkout: {error}")
            return
        if created.returncode:
            UNVERIFIED.append(f"could not create test checkout: {created.stderr}")
            return
        try:
            dependencies = Path.cwd() / "node_modules"
            if not dependencies.is_dir():
                UNVERIFIED.append("Commander dependencies are unavailable")
            else:
                (checkout / "node_modules").symlink_to(dependencies, target_is_directory=True)
                if not hidden:
                    run_tests(checkout, tests, "committed tests")
                else:
                    target = checkout / "tests/command.configureOutput.test.js"
                    if not target.is_file() or target.is_symlink():
                        UNVERIFIED.append("hidden-test target is not a regular file")
                    else:
                        shutil.copyfile(Path(__file__).parent / "hidden" / target.name, target)
                        run_tests(checkout, [str(target.relative_to(checkout))], "hidden tests")
        finally:
            try:
                removed = subprocess.run(["git", "worktree", "remove", "--force", str(checkout)],
                                         capture_output=True, text=True, timeout=30, check=False)
                if removed.returncode:
                    UNVERIFIED.append(f"could not remove test checkout: {removed.stderr}")
            except (OSError, subprocess.TimeoutExpired) as error:
                UNVERIFIED.append(f"could not remove test checkout: {error}")


def main() -> None:
    expect(new_commit(), "nothing was committed")
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    tests = sorted(path for path in committed_files()
                   if path.startswith("tests/") and path.endswith((".test.js", ".spec.js"))
                   and Path(path).is_file() and not Path(path).is_symlink())
    expect(bool(tests), "no committed Commander test file was changed")
    if tests:
        check_phase(tests, hidden=False)
    check_phase([], hidden=True)
    finish()


if __name__ == "__main__":
    main()
