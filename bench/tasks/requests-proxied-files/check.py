import os
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, committed_files, dirty, expect, finish, new_commit  # noqa: E402


def check_tests(paths):
    """Run the agent's committed test files and require complete coverage."""
    deadline = time.monotonic() + 120
    for path in paths:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            UNVERIFIED.append(f"committed test budget exhausted before {path}")
            return
        check_test_file(path, remaining)


def check_test_file(path, timeout):
    with tempfile.TemporaryDirectory(prefix="requests-bench-check-") as tmp:
        report = Path(tmp) / "pytest.xml"
        try:
            proc = subprocess.run(
                ["python3", "-m", "pytest", "-o", "addopts=", f"--junitxml={report}", "--", path],
                capture_output=True, text=True, timeout=timeout,
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
        if proc.returncode == 1:
            expect(False, f"committed tests failed for {path}:\n{output}")
            return
        if proc.returncode != 0:
            UNVERIFIED.append(f"committed tests could not run for {path} (exit {proc.returncode}):\n{output}")
            return
        try:
            root = ET.parse(report).getroot()
            cases = root.findall(".//testcase")
            if not cases:
                UNVERIFIED.append(f"committed tests reported no test cases for {path}")
            elif any(case.find("failure") is not None or case.find("error") is not None for case in cases):
                expect(False, f"committed tests reported failures for {path} despite a successful exit:\n{output}")
            elif any(case.find("skipped") is not None for case in cases):
                UNVERIFIED.append(f"committed tests included skipped or expected-failure cases for {path}")
        except (OSError, ET.ParseError) as error:
            UNVERIFIED.append(f"committed tests did not produce a readable report for {path}: {error}")

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
    run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                         env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60)
    expect(run.returncode == 0, f"multipart cases failed:\n{run.stdout}\n{run.stderr}")
    expect(new_commit(), "nothing was committed")
    test_files = [path for path in committed_files() if path.startswith("tests/") and path.endswith(".py")
                  and Path(path).is_file() and not Path(path).is_symlink()
                  and subprocess.run(["git", "cat-file", "-t", f"HEAD:{path}"],
                                     capture_output=True, text=True).stdout.strip() == "blob"
                  and (Path(path).name.startswith("test_") or Path(path).name.endswith("_test.py"))]
    expect(bool(test_files), "no committed test file was changed")
    if test_files:
        check_tests(sorted(test_files))
    expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
    finish()


if __name__ == "__main__":
    main()
