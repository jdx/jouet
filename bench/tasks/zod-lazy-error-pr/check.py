import json
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, github_state, new_commit  # noqa: E402

expect(new_commit(), "nothing was committed")
expect(not dirty(), f"changes were left uncommitted: {dirty()}")

tests = "packages/zod/src/v4/classic/tests"
# What CI failed, as the agent left it: the issue-schema tests must pass in
# both vitest projects, none skipped or removed (9 tests, each run twice).
report = Path("vitest-report.json")
own = subprocess.run(
    ["node_modules/.bin/vitest", "run", f"{tests}/issue-schema.test.ts", "--reporter=json",
     f"--outputFile={report}"],
    capture_output=True, text=True, timeout=240,
)
results = json.loads(report.read_text()) if report.exists() else {}
report.unlink(missing_ok=True)
passed = results.get("numPassedTests", 0)
failures = [
    f"{a['fullName']}: {''.join(a.get('failureMessages') or []).strip()[:300]}"
    for file in results.get("testResults", []) for a in file.get("assertionResults", [])
    if a.get("status") == "failed"
]
details = "\n".join(failures) or (own.stdout + own.stderr)[-3000:]
expect(results.get("numFailedTests", 1) == 0 and passed >= 18,
       f"the issue-schema tests CI failed do not all pass in the work tree: {passed} passed, "
       f"{results.get('numFailedTests')} failed:\n{details}")

# The upstream tests, which the agent never saw, replace whatever is in the
# work tree: the issue-schema tests CI failed (227d555), the review's
# synchronously throwing check (3ccbab4), and the PR's own tests that the
# error is still built on first read, so dropping the change does not pass.
for name in ("issue-schema.test.ts", "standard-schema.test.ts", "error.test.ts"):
    shutil.copyfile(Path(__file__).parent / "hidden" / name, f"{tests}/{name}")
proc = subprocess.run(
    ["node_modules/.bin/vitest", "run", *(f"{tests}/{name}" for name in
                                          ("issue-schema.test.ts", "standard-schema.test.ts", "error.test.ts"))],
    capture_output=True, text=True, timeout=240,
)
expect(proc.returncode == 0, f"the hidden tests fail:\n{(proc.stdout + proc.stderr)[-3000:]}")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 6519)
thread = next(t for t in pr["reviewThreads"]["nodes"] if t["id"] == "PRRT_kwDOdemo1")
replied = any(c["author"]["login"] == viewer for c in thread["comments"]["nodes"][1:])
expect(replied, "no reply on the review thread about the synchronous throw")
expect(thread["isResolved"], "the review thread about the synchronous throw was not resolved")
finish()
