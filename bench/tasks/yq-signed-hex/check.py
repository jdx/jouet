import subprocess
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import expect, finish, new_commit  # noqa: E402

# Add an independent issue-specific test without replacing the project's or
# agent's tests. Signed octal was already broken and is outside this issue.
shutil.copyfile(Path(__file__).parent / "hidden" / "signed_hex_test.go", "pkg/yqlib/zz_bench_signed_hex_test.go")

# Only the fix's tests: the base already fails TestTomlScenarios.
proc = subprocess.run(
    ["go", "test", "./pkg/yqlib/", "-run", "^(TestJSONScenarios|TestParseInt64|TestBenchmarkSignedHexJSON)$"],
    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=540,
)
output = proc.stdout
if len(output) > 8000:
    output = f"... {len(output) - 8000} earlier characters omitted\n" + output[-8000:]
expect(proc.returncode == 0, f"the issue or existing tests fail:\n{output}")
# Not a condition of the check: the prompt asks for a commit.
print(f"committed: {new_commit()}")
finish()
