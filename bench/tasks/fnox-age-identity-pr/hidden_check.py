"""Run the hidden Bats test without overwriting or retaining agent files."""

import re
import tempfile
from pathlib import Path


SOURCE = Path(__file__).parent / "hidden" / "age_identities.bats"


def failed_hidden(output):
    """Only a named hidden test's TAP failure establishes a failure on timeout."""
    if isinstance(output, bytes):
        output = output.decode(errors="replace")
    names = set(re.findall(r'^\s*@test\s+"([^"]+)"\s*\{', SOURCE.read_text(), re.MULTILINE))
    results = re.findall(r'^not ok [1-9]\d*\s+([^\n]+)$', output or "", re.MULTILINE)
    return any(name.strip() in names or re.sub(r'(?: # in \d+ ms| in \d+ms)$', '', name).strip() in names
               for name in results)


def run_hidden(run):
    directory = Path("test")
    if directory.is_symlink() or not directory.is_dir():
        raise OSError("test directory is missing or is a symlink")
    # Bats resolves the task's helper relative to the test file, so put the
    # unique temporary file beside the agent's tests, without replacing one.
    with tempfile.NamedTemporaryFile(prefix=".jouet-hidden-", suffix=".bats", dir=directory) as test:
        test.write(SOURCE.read_bytes())
        test.flush()
        return run("test/bats/bin/bats", "--tap", test.name)
