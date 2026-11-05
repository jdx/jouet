import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream follow-up (jdx/fnox#1099, ace6c26): an inline FNOX_AGE_KEY
# skips named selection, a selected identity clears the `identity`
# reference, FNOX_AGE_IDENTITY is read per call, the selection test takes the
# environment as parameters, and the rendered schema is committed.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(age): keep FNOX_AGE_KEY first and read FNOX_AGE_IDENTITY per call",
    "crates/fnox-core/src/env.rs",
    "crates/fnox-core/src/providers/age.rs",
    "crates/fnox-core/src/providers/mod.rs",
    "docs/public/schema.json",
    "docs/reference/environment.md",
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
for thread_id, body in (
    ("PRRT_kwDOdemo1", "Selecting a named identity now clears the `identity` reference, so the old provider is not contacted."),
    ("PRRT_kwDOdemo2", "With `FNOX_AGE_KEY` set, named-identity selection is skipped, so an unknown name no longer blocks it."),
    ("PRRT_kwDOdemo3", "`FNOX_AGE_IDENTITY` is read on each selection through `env::fnox_age_identity()`."),
    ("PRRT_kwDOdemo4", "The selection logic takes the env identity as a parameter, and the test passes it explicitly."),
):
    graphql(REPLY, id=thread_id, body=body)
    graphql(RESOLVE, id=thread_id)
# The upstream answer, left open.
graphql(REPLY, id="PRRT_kwDOdemo5", body="Not changing this in this PR: `key_file` is resolved by the shared generated "
        "provider-resolution code before selection runs; skipping it needs a change there, for a follow-up.")
