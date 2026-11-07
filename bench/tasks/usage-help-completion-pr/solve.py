import json
import os
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

# The upstream follow-ups (jdx/usage#1605): 7eb1d6e offers the supplied flags
# in the Go runtime and `--version` through views; 343a135 moves the
# stay-disabled rule from `SpecCommand::merge` to `mount`, and adds the
# supplied flags before the default command's in `complete-word`.
subprocess.run(["git", "apply", str(Path(__file__).parent / "fix.patch")], check=True)
commit(
    "fix(complete): offer help and version in the Go runtime and in views",
    "argv/src/complete.rs",
    "cli/src/cli/complete_word.rs",
    "go/argv/complete.go",
    "lib/src/spec/cmd.rs",
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
    ("PRRT_kwDOdemo1", "Go completion now offers the unclaimed `--help`/`-h` (and `--version`/`-V` where accepted); "
     "`TestTheCursorGetsTheReferencesAnswer` passes."),
    ("PRRT_kwDOdemo2", "Restored the plain overwrite in `SpecCommand::merge` so includes behave as before; the "
     "stay-disabled rule now lives only in `mount`."),
    ("PRRT_kwDOdemo3", "In a view, completion now also offers `--version`/`-V` from the root's settings, as the "
     "parser does."),
    ("PRRT_kwDOdemo4", "Fixed: in a view, `--version`/`-V` come from the root's settings."),
    ("PRRT_kwDOdemo6", "Fixed in `usage complete-word`: supplied flags are added before the default command's "
     "flags, so its hidden `--help` no longer takes the spelling."),
):
    graphql(REPLY, id=thread_id, body=body)
    graphql(RESOLVE, id=thread_id)
graphql(REPLY, id="PRRT_kwDOdemo5", body="Not changed: declared and supplied flags are treated alike while a flag "
        "awaits a value (a dash-prefixed word is offered as a flag in both engines), so a supplied `--help` isn't "
        "special there.")
