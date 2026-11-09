import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from _check import UNVERIFIED, dirty, expect, finish, git, github_state, new_commit  # noqa: E402
from hidden_check import failed_hidden, run_hidden  # noqa: E402

# What was committed, before the hidden test is added. `mise install` may
# rewrite mise.lock, which is not the agent's change.
expect(new_commit(), "nothing was committed")
left = [path for path in dirty() if path != "mise.lock"]
expect(not left, f"changes were left uncommitted: {left}")


def run(*cmd: str, env: dict[str, str] | None = None) -> tuple[bool, str]:
    proc = subprocess.run(cmd, capture_output=True, text=True, env={**os.environ, **(env or {})}, timeout=600)
    return proc.returncode == 0, (proc.stdout + proc.stderr)[-3000:]


# The agent's own environment must not pick the identity for the tests.
clean_env = {key: value for key, value in os.environ.items()
             if key not in ("FNOX_AGE_KEY", "FNOX_AGE_KEY_FILE", "FNOX_AGE_IDENTITY")}
os.environ.clear()
os.environ.update(clean_env)

ok, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    # What CI failed: `mise run render` must leave docs/public/schema.json as
    # committed. Compared as JSON, so the formatter does not matter.
    proc = subprocess.run(["target/debug/fnox", "schema"], capture_output=True, text=True, timeout=60)
    try:
        committed = json.loads(git("show", "HEAD:docs/public/schema.json"))
        rendered = json.loads(proc.stdout)
        expect(committed == rendered,
               "docs/public/schema.json as committed is not what `fnox schema` renders, so CI's render check still fails")
    except (json.JSONDecodeError, subprocess.CalledProcessError) as error:
        expect(False, f"could not compare the rendered schema: {error}\n{proc.stderr[-1000:]}")

    # The PR's own age unit tests still pass, and the selection test no
    # longer depends on FNOX_AGE_IDENTITY in the process environment (the
    # test-isolation thread).
    ok, out = run("cargo", "test", "-p", "fnox-core", "--lib", "providers::age")
    expect(ok, f"the age provider's unit tests fail:\n{out}")
    ok, out = run("cargo", "test", "-p", "fnox-core", "--lib", "providers::age::tests::select",
                  env={"FNOX_AGE_IDENTITY": "yk2"})
    counts = [int(n) for n in re.findall(r"running (\d+) tests?", out)]
    expect(sum(counts) > 0, "the age provider's identity-selection unit test is gone")
    expect(ok, f"with FNOX_AGE_IDENTITY exported, the identity-selection unit test fails:\n{out}")

    # The hidden test, which the agent never saw: what the threads asked of
    # the built fnox (an inline FNOX_AGE_KEY outranks an unknown name, a
    # selected identity replaces the `identity` reference, the daemon reads
    # FNOX_AGE_IDENTITY per request), and that the feature still works.
    try:
        ok, out = run_hidden(run)
        expect(ok, f"the hidden named-identity tests fail:\n{out}")
    except subprocess.TimeoutExpired as error:
        output = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
        print(output, end="")
        if failed_hidden(output):
            expect(False, "a hidden named-identity test failed before the Bats suite timed out")
        else:
            UNVERIFIED.append(f"the hidden named-identity tests timed out: {error}")
    except OSError as error:
        UNVERIFIED.append(f"the hidden named-identity tests could not complete: {error}")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 1099)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (
    ("PRRT_kwDOdemo1", "the identity reference winning over a selected identity"),
    ("PRRT_kwDOdemo2", "FNOX_AGE_KEY being blocked by an unknown name"),
    ("PRRT_kwDOdemo3", "the daemon caching FNOX_AGE_IDENTITY"),
    ("PRRT_kwDOdemo4", "the unit test reading the process environment"),
):
    expect(replied(thread_id), f"no reply on the thread about {what}")
    expect(threads[thread_id]["isResolved"], f"the thread about {what} was not resolved")
# Upstream deferred this one (key_file resolves before selection, in shared
# generated code): a reply is required, and fixing and resolving it is fine.
expect(replied("PRRT_kwDOdemo5"), "no reply on the thread about key_file resolving before selection")
finish()
