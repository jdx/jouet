import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, git, github_state, new_commit  # noqa: E402

# What was committed, before the hidden test replaces the agent's copy.
expect(new_commit(), "nothing was committed")
expect(not dirty(), f"changes were left uncommitted: {dirty()}")
source = git("show", "HEAD:src/cli/uninstall.rs")
expect("#[test]" in source, "src/cli/uninstall.rs has no unit test, which the review asked for")

# The fix's own e2e test, the one CI ran (the PR's copy, unless the agent
# weakened it): 1.1.1 is not 1.1.12, `latest`, and java's `+build`.
shutil.copyfile(Path(__file__).parent / "hidden" / "test_uninstall", "e2e/cli/test_uninstall")


def run(*cmd: str) -> tuple[bool, str]:
    env = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd()}
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=1500)
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-3000:]


ok, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    ok, out = run("mise", "run", "test:e2e", "test_uninstall")
    expect(ok, f"the e2e test CI ran still fails:\n{out}")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 15116)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (("PRRT_kwDOdemo1", "the backend's own matching"), ("PRRT_kwDOdemo2", "unit tests")):
    expect(replied(thread_id), f"no reply on the thread asking for {what}")
    expect(threads[thread_id]["isResolved"], f"the thread asking for {what} was not resolved")
expect(replied("PRRT_kwDOdemo3"), "the question about node@20 got no answer")
expect(not threads["PRRT_kwDOdemo3"]["isResolved"], "the question about node@20 was resolved")
old = threads["PRRT_kwDOdemo0"]
expect(old["isResolved"] and len(old["comments"]["nodes"]) == 1, "the thread that was already resolved was changed")
finish()
