import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import UNVERIFIED, dirty, expect, finish, git, github_state, new_commit  # noqa: E402

# What was committed, before the check builds, renders and swaps in the hidden
# test. Running mise during the task can re-resolve `latest` tools in
# mise.lock; that churn is not the fix, and a PR that leaves it out is right to.
expect(new_commit(), "nothing was committed")
left = [path for path in dirty() if path != "mise.lock"]
expect(not left, f"changes were left uncommitted: {left}")
SCHEMAS = ("schema/mise.json", "schema/mise-task.json", "schema/miserc.json")
committed = {path: git("show", f"HEAD:{path}") for path in ("mise.usage.kdl", "man/man1/mise.1", *SCHEMAS)}

env = {**os.environ, "MISE_TRUSTED_CONFIG_PATHS": os.getcwd(), "CARGO_TERM_COLOR": "never", "NO_COLOR": "1"}


def run(*cmd: str, timeout: int = 1500, **extra: str) -> tuple[bool, str, str]:
    proc = subprocess.run(cmd, capture_output=True, text=True, env={**env, **extra}, timeout=timeout)
    return proc.returncode == 0, proc.stdout, (proc.stdout + proc.stderr)[-3000:]


ok, _, out = run("cargo", "build")
expect(ok, f"cargo build fails:\n{out}")
if ok:
    # What CI failed: `mise run render` must leave the man page as committed.
    # render:usage writes mise.usage.kdl from the CLI, and render:mangen the
    # man page from that file with `usage`, then drops one deprecated flag.
    ok, kdl, out = run("target/debug/mise", "usage", CLICOLOR_FORCE="0")
    expect(ok and kdl.strip() == committed["mise.usage.kdl"],
           "mise.usage.kdl as committed is not what `mise usage` writes, so CI's render check still fails")
    ok, man, out = run("mise", "x", "usage", "--", "usage", "generate", "manpage", "--file", "mise.usage.kdl", timeout=600)
    if ok:
        man = re.sub(r"\.TP\n\\fB\\-\\-from\\-git\\fR[^\n]*\nDeprecated alias for \\-\\-adopt\n", "", man)
        expect(man.strip() == committed["man/man1/mise.1"],
               "man/man1/mise.1 as committed is not what `mise run render:mangen` renders, so CI's render check "
               "still fails")
    else:
        UNVERIFIED.append(f"could not run `usage generate manpage` to render the man page:\n{out}")

    # The schemas are generated from settings.toml, which a fix may touch:
    # they must still be what render:schema writes. Compared as JSON, so the
    # formatter does not matter.
    ok, _, out = run("cargo", "run", "--quiet", "--ignore-rust-version", "--package", "mise-config",
                     "--example", "render-schema")
    expect(ok, f"the schema renderer fails:\n{out}")
    if ok:
        for path in SCHEMAS:
            expect(json.loads(Path(path).read_text()) == json.loads(committed[path]),
                   f"{path} as committed is not what `mise run render:schema` writes")
        git("checkout", "HEAD", "--", *SCHEMAS)

    # The PR's own unit tests of the miserc loader still pass.
    ok, _, out = run("cargo", "test", "--lib", "miserc")
    expect(ok, f"the miserc unit tests fail:\n{out}")

    # The hidden test, upstream's e2e test as merged: the PR's cases, plus what
    # the threads asked. A global `env = ["floor"]` survives a project
    # `[env_sets] floor = []`, and `MISE_ENV=''` selects no environment even
    # when a miserc sets `env`. The PR's --save-env test runs as committed.
    shutil.copyfile(Path(__file__).parent / "hidden" / "test_miserc_env_sets", "e2e/config/test_miserc_env_sets")
    ok, _, out = run("mise", "run", "test:e2e", "test_miserc_env_sets", "test_bootstrap_save_env", MISE_TASK_SKIP="build")
    expect(ok, f"the e2e tests fail (a project set named like a global environment, an empty MISE_ENV, or "
               f"--save-env):\n{out}")

state = github_state()
viewer = state["graphql"]["viewer"]["login"]
pr = next(p for p in state["graphql"]["repository"]["pullRequests"]["nodes"] if p["number"] == 15327)
threads = {t["id"]: t for t in pr["reviewThreads"]["nodes"]}


def replied(thread_id):
    return any(c["author"]["login"] == viewer for c in threads[thread_id]["comments"]["nodes"][1:])


for thread_id, what in (
    ("PRRT_kwDOdemo1", "a project set dropping a global environment (Greptile)"),
    ("PRRT_kwDOdemo2", "an empty MISE_ENV loading the miserc env"),
    ("PRRT_kwDOdemo3", "a project set dropping a global environment (Cursor)"),
):
    expect(replied(thread_id), f"no reply on the thread about {what}")
    expect(threads[thread_id]["isResolved"], f"the thread about {what} was not resolved")
# CodeRabbit's suggestion to drop env_sets from the mise.toml settings schema:
# upstream declined it, as `env`, `auto_env` and `ceiling_paths` are listed
# there too. A reply is required. Resolving it means the change was taken, so
# a resolved thread needs env_sets gone from the settings schema.
expect(replied("PRRT_kwDOdemo4"), "the thread about env_sets in the mise.toml schema got no reply")
if threads["PRRT_kwDOdemo4"]["isResolved"]:
    settings = json.loads(committed["schema/mise.json"])["$defs"]["settings"]["properties"]
    expect("env_sets" not in settings,
           "the thread about env_sets in the mise.toml schema was resolved, but the schema still lists it")
finish()
