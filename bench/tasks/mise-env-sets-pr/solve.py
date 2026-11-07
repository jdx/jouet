import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream follow-ups (jdx/mise#15327): a45126a keeps an empty MISE_ENV
# empty and drops project set names that match a global `env` entry, 8fafb6f
# and 571e93b test them, and 9f62cbab regenerates the man page for
# --save-env, which is what CI's render check wanted.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(config): keep an empty MISE_ENV empty and global envs out of project env_sets",
    "docs/configuration/environments.md",
    "e2e/config/test_miserc_env_sets",
    "man/man1/mise.1",
    "settings.toml",
    "src/cli/bootstrap.rs",
    "src/config/miserc.rs",
    "src/env.rs",
)


def graphql(query, **variables):
    request = urllib.request.Request(
        os.environ["BENCH_GITHUB_URL"] + "/graphql",
        data=json.dumps({"query": query, "variables": variables}).encode(),
        headers={"Content-Type": "application/json"},
    )
    response = json.load(urllib.request.urlopen(request))
    assert "errors" not in response, response


REPLY = """mutation($id: ID!, $body: String!) {
  addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $id, body: $body}) { comment { url } }
}"""
RESOLVE = "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }"
graphql(REPLY, id="PRRT_kwDOdemo1", body="A project set can no longer take a name that the global miserc selects in "
        "`env` or defines as a set; the e2e covers `floor = []`.")
graphql(RESOLVE, id="PRRT_kwDOdemo1")
graphql(REPLY, id="PRRT_kwDOdemo2", body="`explicit_environment` returns `None` only when nothing is given, so an "
        "empty `MISE_ENV` selects nothing again; the e2e covers it.")
graphql(RESOLVE, id="PRRT_kwDOdemo2")
graphql(REPLY, id="PRRT_kwDOdemo3", body="Same fix: project set names that match a global `env` entry or global set "
        "are dropped when loading, so the global environments and their order are untouched.")
graphql(RESOLVE, id="PRRT_kwDOdemo3")
graphql(REPLY, id="PRRT_kwDOdemo4", body="Not changing this here. `env_sets` follows the other `rc` settings (`env`, "
        "`auto_env`, `ceiling_paths`), which the generated `[settings]` schema also lists although mise reads them "
        "only from miserc; `mise settings set` already refuses them with a pointer to miserc. Hiding `rc` keys from "
        "`mise.toml` would be its own change for all of them.")
