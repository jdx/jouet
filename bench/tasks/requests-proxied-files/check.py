import os
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit


def check_tests(paths):
    """Run the agent's committed test files and require complete coverage."""
    deadline = time.monotonic() + 120
    for path in paths:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            UNVERIFIED.append(f"committed test budget exhausted before {path}")
            return
        check_test_file(path, remaining)


def read_report(report):
    """Require complete pytest suite totals before accepting its cases."""
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


def skipped_details(cases):
    skipped = [case for case in cases if case.find("skipped") is not None]
    lines = []
    for case in skipped[:10]:
        name = "::".join(part for part in (case.get("classname"), case.get("name")) if part) or "<unnamed>"
        reason = case.find("skipped").get("message", "")
        description = f"{name}: {reason}" if reason else name
        if len(description) > 240:
            description = f"{description[:240]}... ({len(description) - 240} characters omitted)"
        lines.append(description)
    if len(skipped) > len(lines):
        lines.append(f"... {len(skipped) - len(lines)} more cases omitted")
    return "\n".join(lines)


def check_test_file(path, timeout):
    with tempfile.TemporaryDirectory(prefix="requests-bench-check-") as tmp:
        report = Path(tmp) / "pytest.xml"
        try:
            proc = subprocess.run(
                ["python3", "-m", "pytest", "-o", "addopts=", f"--junitxml={report}", "--", path],
                capture_output=True, text=True, timeout=timeout, check=False,
                env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")},
            )
        except subprocess.TimeoutExpired as error:
            def as_text(value):
                return value.decode(errors="replace") if isinstance(value, bytes) else value or ""
            output = (as_text(error.stdout) + as_text(error.stderr))[-3000:]
            UNVERIFIED.append(f"committed tests timed out for {path}:\n{output}")
            return
        except OSError as error:
            UNVERIFIED.append(f"committed tests could not run for {path}: {error}")
            return
        output = (proc.stdout + proc.stderr)[-3000:]
        try:
            cases = read_report(report)
            if any(case.find("failure") is not None for case in cases):
                expect(False, f"committed tests failed for {path}:\n{output}")
            elif proc.returncode != 0 or any(case.find("error") is not None for case in cases):
                UNVERIFIED.append(f"committed tests could not complete for {path} (exit {proc.returncode}):\n{output}")
            elif not cases:
                UNVERIFIED.append(f"committed tests reported no test cases for {path}")
            elif any(case.find("skipped") is not None for case in cases):
                UNVERIFIED.append(
                    f"committed tests included skipped or expected-failure cases for {path}:\n{skipped_details(cases)}"
                )
        except (OSError, ET.ParseError, ValueError) as error:
            UNVERIFIED.append(f"committed tests did not produce a readable report for {path}: {error}\n{output}")

# Exercise the public API without making a network request or replacing tests.
program = r'''
import io
import tempfile
import requests

class Proxy:
    def __init__(self, wrapped):
        self.wrapped = wrapped
    def __getattr__(self, name):
        return getattr(self.wrapped, name)

def body(value):
    request = requests.Request("POST", "https://example.invalid/upload", files={"file": value})
    prepared = request.prepare()
    assert prepared.headers["Content-Type"].startswith("multipart/form-data;")
    return prepared.body

with tempfile.NamedTemporaryFile(mode="w+b") as handle:
    handle.write(b"named temporary content")
    handle.seek(0)
    assert b"named temporary content" in body(handle)
assert b"proxied binary content" in body(("proxy.bin", Proxy(io.BytesIO(b"proxied binary content"))))
assert b"ordinary binary content" in body(("normal.bin", io.BytesIO(b"ordinary binary content")))
assert b"literal bytes" in body(("literal.bin", b"literal bytes"))
'''
def main():
    run = subprocess.run(["python3", "-c", program], capture_output=True, text=True, check=False,
                         env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60)
    expect(run.returncode == 0, f"multipart cases failed:\n{run.stdout}\n{run.stderr}")
    expect(new_commit(), "nothing was committed")
    test_files = [path for path in committed_files() if path.startswith("tests/") and path.endswith(".py")
                  and Path(path).is_file() and not Path(path).is_symlink()
                  and subprocess.run(["git", "cat-file", "-t", f"HEAD:{path}"],
                                     capture_output=True, text=True, check=False).stdout.strip() == "blob"
                  and (Path(path).name.startswith("test_") or Path(path).name.endswith("_test.py"))]
    expect(bool(test_files), "no committed test file was changed")
    if test_files:
        check_tests(sorted(test_files))
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    finish()


if __name__ == "__main__":
    main()
