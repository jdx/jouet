import json
import os
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from _check import commit  # noqa: E402

p = Path("invoice.py")
text = p.read_text().replace("tmp", "subtotal")
old = "    return (total(lines) / len(lines))"
assert old in text
text = text.replace(old, '    if not lines:\n        return Decimal("0.00")\n' + old)
p.write_text(text)
commit("fix: name the subtotal and average an empty invoice as zero", "invoice.py")


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
graphql(REPLY, id="PRRT_kwDOdemo1", body="Renamed to `subtotal`.")
graphql(RESOLVE, id="PRRT_kwDOdemo1")
graphql(REPLY, id="PRRT_kwDOdemo2", body='An empty invoice now averages to `Decimal("0.00")`.')
graphql(RESOLVE, id="PRRT_kwDOdemo2")
graphql(REPLY, id="PRRT_kwDOdemo3", body="Yes: invoices round half-up to cents, as the README says.")
