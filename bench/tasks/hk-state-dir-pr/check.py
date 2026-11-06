import os
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, git, github_state, new_commit  # noqa: E402

# What was committed, before the hidden tests replace the agent's copies.
# mise.lock may change when the agent runs `mise run`; that is not source.
expect(new_commit(), "nothing was committed")
left = [path for path in dirty() if path != "mise.lock"]
expect(not left, f"changes were left uncommitted: {left}")
# The bats test CI failed stays, with its assertion: whether hk stops writing
# .git/hk for `hk agent` or the test stops snapshotting .git, it has to pass.
generators = git("show", "HEAD:test/agent_generators.bats")
expect('@test "agent generators do not edit host configuration"' in generators
       and 'assert_equal "$after" "$before"' in generators,
       "the agent generators test CI failed was removed or weakened")


def run(*cmd: str) -> tuple[bool, str]:
    # The tests set git's global config in their own HOME, so drop the
    # checker's GIT_CONFIG_GLOBAL.
    env = {k: v for k, v in os.environ.items() if k != "GIT_CONFIG_GLOBAL"}
    env["MISE_TRUSTED_CONFIG_PATHS"] = os.getcwd()
    proc = subprocess.run(cmd, capture_output=True, text=True, env=env, timeout=900)
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-3000:]


# The lint job's failure: CI runs clippy through `mise run lint` like this.
ok, out = run("cargo", "clippy", "--quiet", "--all-targets", "--", "-D", "warnings")
expect(ok, f"cargo clippy fails:\n{out}")

# The review, as tests the agent never saw (state follows GIT_DIR; a shared
# repository gets no state directory only one account can write), and the
# PR's own state tests as pushed, so dropping the change does not pass.
hidden = Path(__file__).parent / "hidden"
for name in ("state_dir_repo.bats", "output_file.bats", "patch_backup.bats"):
    shutil.copyfile(hidden / name, f"test/{name}")
ok, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    ok, out = run("test/bats/bin/bats", "test/agent_generators.bats")
    expect(ok, f"the bats test CI failed still fails:\n{out}")
    ok, out = run("test/bats/bin/bats", "test/state_dir_repo.bats", "test/output_file.bats", "test/patch_backup.bats")
    expect(ok, f"the hidden state tests fail:\n{out}")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 1705)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (
    ("PRRT_kwDOdemo1", "GIT_DIR (greptile)"),
    ("PRRT_kwDOdemo2", "core.sharedRepository (greptile)"),
    ("PRRT_kwDOdemo3", "GIT_DIR (cursor)"),
    ("PRRT_kwDOdemo4", "core.sharedRepository (cursor)"),
):
    expect(replied(thread_id), f"no reply on the thread about {what}")
    expect(threads[thread_id]["isResolved"], f"the thread about {what} was not resolved")
expect(replied("PRRT_kwDOdemo5"), "the question about falling back to the platform state dir got no answer")
expect(not threads["PRRT_kwDOdemo5"]["isResolved"], "the question about the fallback was resolved")
finish()
