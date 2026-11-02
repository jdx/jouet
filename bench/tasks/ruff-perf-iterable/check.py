import ast
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

build = subprocess.run(["cargo", "build", "--locked", "--bin", "ruff"], capture_output=True, text=True)
expect(build.returncode == 0, f"ruff build failed:\n{build.stderr[-3000:]}")
if build.returncode == 0:
    binary = Path.cwd() / "target/debug/ruff"
    code = '''def make_list(values, allowed):
    result = []
    for value in [values] if isinstance(values, str) else values:
        if value not in allowed:
            result.append(value)
    return result

def make_dict(values):
    result = {}
    for key, value in [values] if isinstance(values, tuple) else values:
        result[key] = value
    return result
'''
    walrus = '''def make_list(values, allowed):
    result = []
    for value in (items := values):
        if value not in allowed:
            result.append(value)
    return result
'''
    with tempfile.TemporaryDirectory(prefix="ruff-bench-") as tmp:
        fixture = Path(tmp) / "example.py"
        fixture.write_text(code)
        args = [str(binary), "check", "--isolated", "--preview", "--select", "PERF401,PERF403",
                "--unsafe-fixes", "--fix", str(fixture)]
        result = subprocess.run(args, capture_output=True, text=True)
        updated = fixture.read_text()
        expect(result.returncode == 0 and updated != code,
               f"conditional iterable fixes failed: exit {result.returncode}; {result.stdout} {result.stderr}")
        try:
            ast.parse(updated)
            before, after = {}, {}
            exec(code, before)
            exec(updated, after)
            for values in ("x", ["x", "y"]):
                expect(before["make_list"](values, {"y"}) == after["make_list"](values, {"y"}),
                       f"list fix changed behavior for {values!r}")
            for values in (("x", 1), [("x", 1), ("y", 2)]):
                expect(before["make_dict"](values) == after["make_dict"](values),
                       f"dict fix changed behavior for {values!r}")
        except Exception as error:
            expect(False, f"fixed source is invalid or cannot execute: {error}")
        fixture.write_text(walrus)
        result = subprocess.run(args, capture_output=True, text=True)
        try:
            updated = fixture.read_text()
            ast.parse(updated)
            before, after = {}, {}
            exec(walrus, before)
            exec(updated, after)
            same = before["make_list"](["x", "y"], {"y"}) == after["make_list"](["x", "y"], {"y"})
            expect(same and "Fix introduced a syntax error" not in result.stderr,
                   f"named-expression iterable must remain valid and preserve behavior: {result.stdout} {result.stderr}")
        except Exception as error:
            expect(False, f"named-expression iterable produced invalid code: {error}")
committed = new_commit()
print(f"committed: {committed}")
expect(committed, "the fix was not committed")
finish()
