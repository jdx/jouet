import os
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, new_commit  # noqa: E402

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
run = subprocess.run(["python3", "-c", program], capture_output=True, text=True,
                     env={**os.environ, "PYTHONPATH": str(Path.cwd() / "src")}, timeout=60)
expect(run.returncode == 0, f"multipart cases failed:\n{run.stdout}\n{run.stderr}")
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"tracked changes were left uncommitted: {dirty()}")
finish()
