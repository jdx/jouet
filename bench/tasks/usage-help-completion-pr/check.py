import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import dirty, expect, finish, github_state, new_commit

# What was committed, before the hidden tests are added to the agent's files.
expect(new_commit(), "nothing was committed")
left = [path for path in dirty() if path != "mise.lock"]
expect(not left, f"changes were left uncommitted: {left}")

env = {**os.environ, "CARGO_TERM_COLOR": "never"}


def run(*cmd: str, cwd: str = ".") -> tuple[bool, str]:
    proc = subprocess.run(list(cmd), check=False, capture_output=True, text=True, env=env, cwd=cwd, timeout=1500)
    out = proc.stdout + proc.stderr
    return proc.returncode == 0, out


# CI's failure: the Go conformance suite compares `go/argv`'s completion with
# the `usage` binary's, which it finds in target/debug.
ok, out = run("cargo", "build", "-p", "usage-cli")
expect(ok, f"usage-cli does not build:\n{out}")
if ok:
    ok, out = run("go", "test", "./...", cwd="go")
    expect(ok, f"`go test ./...` in go/ fails, as CI's `trusted / test` job did:\n{out}")

# The agent's own Rust suite, then the hidden tests drawn from the threads.
# The completion integration tests drive real shells and can time out on a
# loaded machine, so a failing suite gets one more run.
suite = ["cargo", "test", "--all", "--all-features", "--no-fail-fast"]
# On macOS zsh completion drives a nested pty and needs a controlling terminal.
# BSD script propagates the child exit status, so test failures still fail the check.
if sys.platform == "darwin":
    suite = ["script", "-q", "/dev/null", *suite]
ok, out = run(*suite)
if not ok:
    ok, out = run(*suite)
expect(ok, f"`cargo test --all --all-features` fails:\n{out}")

hidden = Path(__file__).parent / "hidden"
for name, target in (("complete.rs", "argv/src/complete.rs"), ("spec.rs", "lib/src/spec/mod.rs")):
    with open(target, "a") as f:
        f.write("\n" + (hidden / name).read_text())
Path("cli/tests/bench_hidden_help.rs").write_text((hidden / "complete_word_help.rs").read_text())
proc = subprocess.run(
    ["cargo", "test", "--all-features", "--no-fail-fast", "-p", "usage-argv", "-p", "usage-lib", "-p", "usage-cli",
     "bench_hidden"],
    check=False, capture_output=True, text=True, env=env, timeout=1500,
)
out = proc.stdout + proc.stderr
ran = sum(int(n) for n in re.findall(r"test result: \w+\. (\d+) passed", proc.stdout))
failed = [line for line in out.splitlines() if "FAILED" in line or "panicked" in line or line.startswith("error")]
expect(proc.returncode == 0,
       "the hidden tests fail (includes that turn the switches back on, a mount that keeps them, `--version` "
       "in a view, the root's `--help` over a hidden default-command one):\n" + "\n".join(failed[:20]) + out[-2000:])
expect(proc.returncode != 0 or ran == 5, f"expected the 5 hidden tests to run, {ran} passed")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 1605)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (
    ("PRRT_kwDOdemo1", "the Go conformance failure"),
    ("PRRT_kwDOdemo2", "includes merging the disable switches"),
    ("PRRT_kwDOdemo3", "`--version` in views (greptile)"),
    ("PRRT_kwDOdemo4", "`--version` in views (coderabbit)"),
    ("PRRT_kwDOdemo6", "root-supplied flags winning over the default command's"),
):
    expect(replied(thread_id), f"no reply on the thread about {what}")
    expect(threads[thread_id]["isResolved"], f"the thread about {what} was not resolved")
# coderabbit's suggestion about flags awaiting a value: upstream declined it,
# since declared and supplied flags are treated alike there, and coderabbit
# withdrew it. Either a reply saying why or the change will do, so only the
# reply is required.
expect(replied("PRRT_kwDOdemo5"), "the thread about flags awaiting a value got no reply")
finish()
