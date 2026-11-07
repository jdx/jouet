import os
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import (
    UNVERIFIED,
    committed_files,
    dirty,
    expect,
    finish,
    head_sha,
    new_commit,
)


def check_ruff(path: str, mode: str, added: bool) -> None:
    """Only count a new failure when the same file passes at the task base."""
    args = ["ruff", "check"] if mode == "lint" else ["ruff", "format", "--check"]
    try:
        current = subprocess.run([*args, "--", path], capture_output=True, text=True, timeout=60, check=False)
        if current.returncode == 0:
            return
        if current.returncode != 1:
            UNVERIFIED.append(f"Ruff {mode} could not check {path}: {current.stderr.strip()}")
            return
        if added:
            expect(False, f"Ruff {mode} fails on new file {path}:\n{current.stdout}{current.stderr}")
            return
        original = subprocess.run(["git", "show", f"{head_sha()}:{path}"], capture_output=True, timeout=30, check=False)
        if original.returncode:
            UNVERIFIED.append(f"Ruff {mode} cannot compare {path} with its base")
            return
        baseline = subprocess.run(
            [*args, "--stdin-filename", path, "-"], input=original.stdout,
            capture_output=True, timeout=60, check=False,
        )
        if baseline.returncode == 0:
            expect(False, f"Ruff {mode} fails on {path}:\n{current.stdout}{current.stderr}")
        else:
            UNVERIFIED.append(f"Ruff {mode} also fails or cannot run on the base version of {path}")
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"Ruff {mode} could not check {path}: {error}")


def read_report(report: Path) -> list[ET.Element]:
    """Require every pytest suite's totals to agree with its case outcomes."""
    root = ET.parse(report).getroot()
    suites = [root] if root.tag == "testsuite" else list(root) if root.tag == "testsuites" else []
    if not suites or any(suite.tag != "testsuite" for suite in suites):
        raise ValueError("missing pytest suites")
    cases = []
    for suite in suites:
        rows = suite.findall("testcase")
        actual = (len(rows), *(sum(row.find(kind) is not None for row in rows)
                                for kind in ("failure", "error", "skipped")))
        try:
            reported = tuple(int(suite.attrib[key]) for key in ("tests", "failures", "errors", "skipped"))
        except (KeyError, ValueError) as error:
            raise ValueError("missing or invalid pytest totals") from error
        if reported != actual or any(sum(row.find(kind) is not None for kind in ("failure", "error", "skipped")) > 1
                                     for row in rows):
            raise ValueError("pytest totals do not match test cases")
        cases.extend(rows)
    return cases


def check_committed_tests(tests: list[str]) -> None:
    """Run the committed tests in isolation and refuse incomplete reports."""
    with tempfile.TemporaryDirectory(prefix="click-bench-tests-") as directory:
        checkout = Path(directory) / "work"
        report = Path(directory) / "pytest.xml"
        try:
            created = subprocess.run(
                ["git", "worktree", "add", "--detach", "--quiet", str(checkout), "HEAD"],
                capture_output=True, text=True, timeout=30, check=False,
            )
            if created.returncode:
                UNVERIFIED.append(f"could not create committed test checkout: {created.stderr.strip()}")
                return
            try:
                for name in tests:
                    path = checkout / name
                    if (not path.is_file() or path.is_symlink()
                            or not path.resolve().is_relative_to(checkout.resolve())):
                        UNVERIFIED.append(f"committed test is not a regular file in the checkout: {name}")
                        return
                try:
                    proc = subprocess.run(
                        ["python3", "-m", "pytest", "-o", "addopts=", f"--junitxml={report}", "--", *tests],
                        cwd=checkout, capture_output=True, text=True, timeout=120, check=False,
                        env={**os.environ, "PYTHONPATH": str(checkout / "src")},
                    )
                except subprocess.TimeoutExpired as error:
                    def as_text(value):
                        return value.decode(errors="replace") if isinstance(value, bytes) else value or ""
                    output = (as_text(error.stdout) + as_text(error.stderr))[-3000:]
                    UNVERIFIED.append(f"committed tests timed out: {error}\n{output}")
                    return
                output = (proc.stdout + proc.stderr)[-3000:]
                try:
                    cases = read_report(report)
                except (OSError, ET.ParseError, ValueError) as error:
                    UNVERIFIED.append(f"committed tests produced no readable report: {error}\n{output}")
                    return
                if any(case.find("failure") is not None for case in cases):
                    expect(False, f"committed tests failed:\n{output}")
                elif proc.returncode != 0 or any(case.find("error") is not None for case in cases):
                    UNVERIFIED.append(f"committed tests could not complete (exit {proc.returncode}):\n{output}")
                elif not cases or any(case.find("skipped") is not None for case in cases):
                    UNVERIFIED.append(f"committed tests reported no cases or incomplete coverage:\n{output}")
            finally:
                removed = subprocess.run(
                    ["git", "worktree", "remove", "--force", str(checkout)],
                    capture_output=True, text=True, timeout=30, check=False,
                )
                if removed.returncode:
                    UNVERIFIED.append(f"could not remove test checkout: {removed.stderr.strip()}")
        except subprocess.TimeoutExpired as error:
            def as_text(value):
                return value.decode(errors="replace") if isinstance(value, bytes) else value or ""
            output = (as_text(error.stdout) + as_text(error.stderr))[-3000:]
            UNVERIFIED.append(f"committed tests timed out: {error}\n{output}")
        except OSError as error:
            UNVERIFIED.append(f"committed tests could not run: {error}")


def changed_tests() -> list[str] | None:
    """Find changed test files present at HEAD, including renamed files."""
    try:
        result = subprocess.run(
            ["git", "diff", "--name-only", "--diff-filter=ACMRT", "-z", head_sha(), "HEAD", "--", "tests/"],
            capture_output=True, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        UNVERIFIED.append(f"could not list committed tests: {error}")
        return None
    if result.returncode:
        UNVERIFIED.append("could not list committed tests")
        return None
    paths = (os.fsdecode(path) for path in result.stdout.split(b"\0") if path)
    return sorted(path for path in paths if path.startswith("tests/") and path.endswith(".py")
                  and (Path(path).name.startswith("test_") or Path(path).name.endswith("_test.py")))

# Test the public Command API without replacing the agent's tests.
program = r'''
import click

cases = [
    ("Weigh apples vs. pears.", 30, "Weigh apples vs. pears."),
    ("Choose e.g. apples and pears.", 40, "Choose e.g. apples and pears."),
    ("Continue here. another word follows.", 50, "Continue here. another word follows."),
    ("First sentence. Second sentence.", 40, "First sentence."),
    ("Pick a fruit. 3 remain.", 40, "Pick a fruit."),
    ("First line\ncontinues.\n\nSecond paragraph.", 45, "First line continues."),
    ("Weigh apples vs. pears and plums.", 20, "Weigh apples vs...."),
    ("ab cd", 2, "..."),
]
for text, limit, expected in cases:
    actual = click.Command("example", help=text).get_short_help_str(limit)
    if actual != expected:
        raise AssertionError(f"{text!r}: got {actual!r}, expected {expected!r}")
assert click.Command("example", help="Long text", short_help="Exact.").get_short_help_str() == "Exact."
'''
def main() -> None:
    run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                         env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60, check=False)
    expect(run.returncode == 0, f"short-help cases failed:\n{run.stdout}\n{run.stderr}")
    expect(new_commit(), "nothing was committed")
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    changed = committed_files()
    tests = changed_tests()
    if tests is not None:
        expect(bool(tests), "no committed Python test file was changed")
        if tests:
            check_committed_tests(tests)
    added_result = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=A", head_sha(), "HEAD"],
        capture_output=True, text=True, check=True,
    )
    added = set(added_result.stdout.splitlines())
    for path in sorted(changed):
        if path.endswith(".py") and Path(path).is_file():
            check_ruff(path, "lint", path in added)
            check_ruff(path, "format", path in added)
    finish()


if __name__ == "__main__":
    main()
