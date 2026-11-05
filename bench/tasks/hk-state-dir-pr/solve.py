import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream follow-ups (jdx/hk#1705): 8bd97dfc moves `is_patch_of` above
# the test module, opens the repository from GIT_DIR / GIT_WORK_TREE like
# `Git::new()`, and keeps the per-user state dir for a core.sharedRepository
# repository; aa32cf67 stops the agent generators test snapshotting .git.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(config): honor GIT_DIR and shared repositories for state dir",
    "docs/environment_variables.md",
    "src/env.rs",
    "src/git.rs",
    "test/agent_generators.bats",
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
GIT_DIR = "State now resolves the repository like `Git::new()`: `Repository::open_from_env()` when `GIT_DIR`/`GIT_WORK_TREE` is set."
SHARED = "Repositories with `core.sharedRepository` set keep the per-user platform state dir, so no shared 0700 directory is created."
for thread_id, body in (("PRRT_kwDOdemo1", GIT_DIR), ("PRRT_kwDOdemo2", SHARED),
                        ("PRRT_kwDOdemo3", GIT_DIR), ("PRRT_kwDOdemo4", SHARED)):
    graphql(REPLY, id=thread_id, body=body)
    graphql(RESOLVE, id=thread_id)
graphql(REPLY, id="PRRT_kwDOdemo5", body="Yes: the logger needs these paths before a repository is opened, so they must not fail; `Git::new()` reports the real error.")
