"""Run the hidden Bats test without overwriting or retaining agent files."""

import tempfile
from pathlib import Path


def run_hidden(run):
    directory = Path("test")
    if directory.is_symlink() or not directory.is_dir():
        raise OSError("test directory is missing or is a symlink")
    source = Path(__file__).parent / "hidden" / "age_identities.bats"
    # Bats resolves the task's helper relative to the test file, so put the
    # unique temporary file beside the agent's tests, without replacing one.
    with tempfile.NamedTemporaryFile(prefix=".jouet-hidden-", suffix=".bats", dir=directory) as test:
        test.write(source.read_bytes())
        test.flush()
        return run("test/bats/bin/bats", test.name)
