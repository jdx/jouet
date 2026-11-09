import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, head_sha, new_commit  # noqa: E402


def run_tests(checkout: Path, paths: list[str], label: str) -> None:
    report = checkout / "jouet-bench-jest-report.json"
    try:
        proc = subprocess.run(
            ["node_modules/.bin/jest", "--runInBand", "--runTestsByPath", *paths,
             "--json", f"--outputFile={report}"],
            cwd=checkout, capture_output=True, text=True, timeout=240, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        if isinstance(error, subprocess.TimeoutExpired):
            for stream in (error.stdout, error.stderr):
                if stream:
                    text = stream.decode(errors="replace") if isinstance(stream, bytes) else stream
                    print(text, end="" if text.endswith("\n") else "\n")
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


def original_tests() -> dict[str, bytes] | None:
    """Read changed test files as they were when the agent started."""
    try:
        changed = set(subprocess.run(
            ["git", "diff", "--no-renames", "--name-only", "-z", head_sha(), "HEAD", "--", "tests"],
            capture_output=True, timeout=30, check=True,
        ).stdout.split(b"\0"))
        tree = subprocess.run(["git", "ls-tree", "-rz", "--full-tree", head_sha(), "--", "tests"],
                              capture_output=True, timeout=30, check=True).stdout
        originals = {}
        for entry in tree.split(b"\0"):
            if not entry:
                continue
            meta, raw_path = entry.split(b"\t", 1)
            mode, kind, oid = meta.split(b" ")
            path = raw_path.decode("utf-8")
            if raw_path not in changed or not path.endswith((".test.js", ".spec.js")):
                continue
            if kind != b"blob" or mode not in (b"100644", b"100755"):
                raise ValueError(f"original test {path} is not a regular file")
            originals[path] = subprocess.run(["git", "cat-file", "blob", oid.decode("ascii")],
                                             capture_output=True, timeout=30, check=True).stdout
        return originals
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        UNVERIFIED.append(f"could not read original tests: {error}")
        return None


def check_phase(tests: list[str], *, hidden: bool, originals: dict[str, bytes] | None = None) -> None:
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
                if originals is not None:
                    for path, contents in originals.items():
                        target = checkout / path
                        if (target.is_symlink() or not target.parent.is_dir()
                                or (target.exists() and not target.is_file())
                                or not target.resolve().is_relative_to(checkout.resolve())):
                            UNVERIFIED.append(f"original test path is not a safe regular file: {path}")
                            return
                        try:
                            target.write_bytes(contents)
                        except OSError as error:
                            UNVERIFIED.append(f"could not restore original test {path}: {error}")
                            return
                    run_tests(checkout, sorted(originals), "original tests on the fix")
                elif not hidden:
                    run_tests(checkout, tests, "committed tests")
                else:
                    target = checkout / "tests/command.configureOutput.test.js"
                    if not target.exists() and not target.is_symlink():
                        expect(False, "the hidden-test target was removed from the committed tree")
                    elif not target.is_file() or target.is_symlink() or not target.resolve().is_relative_to(checkout.resolve()):
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
    changed = committed_files()
    tests = sorted(path for path in changed
                   if path.startswith("tests/") and path.endswith((".test.js", ".spec.js"))
                   and Path(path).is_file() and not Path(path).is_symlink())
    expect(bool(tests), "no committed Commander test file was changed")
    if tests:
        check_phase(tests, hidden=False)
    originals = original_tests()
    if originals:
        check_phase([], hidden=False, originals=originals)
    check_phase([], hidden=True)
    finish()


if __name__ == "__main__":
    main()
