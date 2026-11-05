import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, git, github_state, head_sha, new_commit  # noqa: E402

# What was committed, before the hidden tests are appended to the agent's files.
expect(new_commit(), "nothing was committed")
left = [path for path in dirty() if path != "mise.lock"]
expect(not left, f"changes were left uncommitted: {left}")

# The docs thread: the namespace page now says an explicit top-level
# `namespace` overrides the directory-derived one.
added = [line[1:] for line in git("diff", head_sha(), "HEAD", "--", "docs/concepts/namespaces.md").splitlines()
         if line.startswith("+") and not line.startswith("+++")]
expect(any(re.search(r"explicit|top-level|overrid", line, re.I) and "namespace" in line for line in added),
       "docs/concepts/namespaces.md does not say that an explicit `namespace` overrides worktree isolation")

# CI's failure, reproduced on Linux: the Windows runner's temp path is not the
# canonical path, and neither is a symlinked TMPDIR's. The agent's own tests
# must pass there, and so must upstream's, appended as a module of their own.
scratch = Path(tempfile.mkdtemp(prefix="pf-check-"))
(scratch / "real").mkdir()
(scratch / "tmp").symlink_to("real")
env = {**os.environ, "TMPDIR": str(scratch / "tmp"), "CARGO_TERM_COLOR": "never"}


def cargo_test(*args: str) -> tuple[bool, str, int]:
    proc = subprocess.run(["cargo", "test", "--lib", *args], capture_output=True, text=True, env=env, timeout=1500)
    out = proc.stdout + proc.stderr
    failed = [line for line in out.splitlines() if "FAILED" in line or "panicked" in line or line.startswith("error")]
    passed = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", proc.stdout))
    return proc.returncode == 0, "\n".join(failed[:20]) + "\n" + proc.stdout[-2000:] + proc.stderr[-1000:], passed


ok, out, _ = cargo_test()
expect(ok, f"the lib tests fail with a symlinked TMPDIR, as CI's Windows runner failed them:\n{out}")

hidden = Path(__file__).parent / "hidden"
for name, target in (("pitchfork_toml.rs", "src/pitchfork_toml.rs"), ("worktree.rs", "src/proxy/worktree.rs")):
    with open(target, "a") as f:
        f.write("\n" + (hidden / name).read_text())
ok, out, ran = cargo_test("bench_hidden")
expect(ok, f"the hidden tests fail (a symlinked start dir, and canonical paths under a symlinked TMPDIR):\n{out}")
expect(not ok or ran == 4, f"expected the 4 hidden tests to run, {ran} passed")
shutil.rmtree(scratch, ignore_errors=True)

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 737)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (("PRRT_kwDOdemo1", "canonicalizing the start dir"), ("PRRT_kwDOdemo2", "the namespace docs")):
    expect(replied(thread_id), f"no reply on the thread asking for {what}")
    expect(threads[thread_id]["isResolved"], f"the thread asking for {what} was not resolved")
# Greptile's suggestion to move discovery onto a blocking worker: upstream
# declined it, as the existing callers are synchronous. Either a reply saying
# why or the change will do, so only the reply is required.
expect(replied("PRRT_kwDOdemo3"), "the thread about blocking filesystem calls got no reply")
finish()
