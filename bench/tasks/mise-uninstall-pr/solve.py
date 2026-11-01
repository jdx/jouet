import json
import os
import shutil
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The fix as merged (jdx/mise#15116), with its unit tests: the backend's own
# matching instead of fixed boundaries.
shutil.copyfile(Path(__file__).parent / "solution" / "uninstall.rs", "src/cli/uninstall.rs")
commit("fix(uninstall): match through the backend's own fuzzy matching", "src/cli/uninstall.rs")


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
graphql(REPLY, id="PRRT_kwDOdemo1", body="Each candidate now goes through `fuzzy_match_filter`, so java's `+build` matches.")
graphql(RESOLVE, id="PRRT_kwDOdemo1")
graphql(REPLY, id="PRRT_kwDOdemo2", body="Added unit tests for `1.1.1` against `1.1.12` and a whole version.")
graphql(RESOLVE, id="PRRT_kwDOdemo2")
graphql(REPLY, id="PRRT_kwDOdemo3", body="Yes: `20` ends at a `.`, so every 20.x matches.")
