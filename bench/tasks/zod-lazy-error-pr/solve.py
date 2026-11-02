import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream follow-ups (colinhacks/zod#6519): 227d555 reads the error in
# the tests that only observe the error maps, and 3ccbab4 makes the async
# branch of `~standard.validate` an async function, with its test.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix: reject instead of throwing when a sync check fails under ~standard",
    "packages/zod/src/v4/classic/tests/issue-schema.test.ts",
    "packages/zod/src/v4/classic/tests/standard-schema.test.ts",
    "packages/zod/src/v4/core/schemas.ts",
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
graphql(REPLY, id="PRRT_kwDOdemo1", body="The async branch is an async function now, so a throwing check rejects; "
        "a test pins it.")
graphql(RESOLVE, id="PRRT_kwDOdemo1")
