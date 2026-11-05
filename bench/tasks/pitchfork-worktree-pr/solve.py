import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream follow-ups (jdx/pitchfork#737): 0a5e3b3 canonicalizes the start
# dir and qualifies the docs, as the review asked; 8fa212b and 0fab67f compare
# canonical paths in the tests that CI failed. The symlink test compares
# against the canonical path too, so it passes under a symlinked TMPDIR.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(config): canonicalize the start dir and compare canonical paths in tests",
    "docs/concepts/namespaces.md",
    "src/pitchfork_toml.rs",
    "src/proxy/worktree.rs",
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
graphql(REPLY, id="PRRT_kwDOdemo1", body="`find_project_root` canonicalizes the start dir now, falling back to the "
        "path as given; a test covers a symlinked subdirectory.")
graphql(RESOLVE, id="PRRT_kwDOdemo1")
graphql(REPLY, id="PRRT_kwDOdemo2", body="The page now says isolation applies to directory-derived namespaces, and "
        "that an explicit top-level `namespace` needs a distinct value per worktree.")
graphql(RESOLVE, id="PRRT_kwDOdemo2")
graphql(REPLY, id="PRRT_kwDOdemo3", body="`all_merged_all_namespaces` is already synchronous and called from async "
        "code; the fast path adds one `read_dir` and one file read, and skips the `git worktree list` spawn. Moving "
        "config loading onto a blocking worker would change every caller, so it is left out of this PR.")
