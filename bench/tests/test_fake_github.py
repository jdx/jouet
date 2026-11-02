import io
import json
import unittest
import urllib.request
import zipfile

from jbench.fake_github import FakeGitHub, materialize

FIXTURE = {
    "repo": "jdx/demo",
    "rest": {
        "/repos/jdx/demo/pulls/7": {"number": 7, "head": {"sha": "abc"}},
        "/repos/jdx/demo/actions/jobs/1/logs": {"$text": "the log", "$until": 30, "$before": {"$status": 404}},
        "/repos/jdx/demo/actions/runs/1/logs": {"$zip": {"0_test.txt": "the log"}},
        "/repos/jdx/demo/actions/runs/1/jobs": {"total_count": 1, "jobs": [{"id": 1}]},
    },
    "graphql": {
        "viewer": {"__typename": "User", "login": "jdx"},
        "repository": {
            "__typename": "Repository",
            "pullRequests": {
                "nodes": [
                    {
                        "__typename": "PullRequest",
                        "id": "PR_7",
                        "number": 7,
                        "reviewThreads": {
                            "nodes": [
                                {
                                    "__typename": "PullRequestReviewThread",
                                    "id": "T1",
                                    "isResolved": False,
                                    "path": "a.py",
                                    "line": 3,
                                    "comments": {
                                        "nodes": [
                                            {
                                                "__typename": "PullRequestReviewComment",
                                                "id": "C1",
                                                "databaseId": 11,
                                                "body": "rename this",
                                                "author": {"login": "reviewer"},
                                            }
                                        ]
                                    },
                                }
                            ]
                        },
                        "check": {"state": "SUCCESS", "$until": 30, "$before": {"state": "PENDING"}},
                    }
                ]
            },
        },
    },
}


def get(url, proxy=None):
    handlers = [urllib.request.ProxyHandler({"http": proxy} if proxy else {})]
    try:
        with urllib.request.build_opener(*handlers).open(url) as r:
            return r.status, r.headers.get("Content-Type"), r.read()
    except urllib.error.HTTPError as e:
        with e:
            return e.code, e.headers.get("Content-Type"), e.read()


def post(url, body):
    request = urllib.request.Request(url, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request) as r:
        return r.status, json.load(r)


class MaterializeTest(unittest.TestCase):
    def test_before_and_after(self):
        tree = {"a": {"state": "done", "$until": 10, "$before": {"state": "pending"}}}
        self.assertEqual(materialize(tree, 5), {"a": {"state": "pending"}})
        self.assertEqual(materialize(tree, 10), {"a": {"state": "done"}})


class ServerTest(unittest.TestCase):
    def test_rest_as_a_proxy_target_and_directly(self):
        with FakeGitHub(FIXTURE) as fake:
            status, _, body = get("http://api.github.localhost/repos/jdx/demo/pulls/7", proxy=fake.url)
            self.assertEqual((status, json.loads(body)["number"]), (200, 7))
            status, _, body = get(fake.url + "/api/v3/repos/jdx/demo/pulls/7?per_page=1")
            self.assertEqual(status, 200)
            status, _, _ = get(fake.url + "/repos/jdx/demo/nope")
            self.assertEqual(status, 404)
            summary = fake.world.summary()
        self.assertEqual(summary["requests"], 3)
        self.assertEqual(summary["unanswered_examples"], ["GET /repos/jdx/demo/nope -> 404"])

    def test_an_attempts_jobs_are_the_runs_jobs(self):
        with FakeGitHub(FIXTURE) as fake:
            status, _, body = get(fake.url + "/repos/jdx/demo/actions/runs/1/attempts/1/jobs?per_page=100")
        self.assertEqual((status, json.loads(body)["total_count"]), (200, 1))

    def test_text_status_and_zip(self):
        with FakeGitHub(FIXTURE) as fake:
            self.assertEqual(get(fake.url + "/repos/jdx/demo/actions/jobs/1/logs")[0], 404)
        with FakeGitHub(FIXTURE, offset_s=60) as fake:
            status, kind, body = get(fake.url + "/repos/jdx/demo/actions/jobs/1/logs")
            self.assertEqual((status, kind.split(";")[0], body), (200, "text/plain", b"the log"))
            status, kind, body = get(fake.url + "/repos/jdx/demo/actions/runs/1/logs")
            self.assertEqual(kind, "application/zip")
            self.assertEqual(zipfile.ZipFile(io.BytesIO(body)).read("0_test.txt"), b"the log")

    def test_time_and_writes_persist(self):
        query = "{ repository { pullRequest(number: 7) { check { state } reviewThreads { nodes { isResolved comments { totalCount } } } } } }"
        with FakeGitHub(FIXTURE) as fake:
            _, before = post(fake.url + "/graphql", {"query": query})
            self.assertEqual(before["data"]["repository"]["pullRequest"]["check"]["state"], "PENDING")
            post(
                fake.url + "/graphql",
                {
                    "query": "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { id } } }",
                    "variables": {"id": "T1"},
                },
            )
            status, reply = post(fake.url + "/repos/jdx/demo/pulls/7/comments/11/replies", {"body": "done"})
            self.assertEqual((status, reply["in_reply_to_id"]), (201, 11))
            _, after = post(fake.url + "/graphql", {"query": query})
            comments = json.loads(get(fake.url + "/repos/jdx/demo/pulls/7/comments")[2])
        thread = after["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"][0]
        self.assertEqual(thread, {"isResolved": True, "comments": {"totalCount": 2}})
        self.assertEqual([c["body"] for c in comments], ["rename this", "done"])
        # The fixture passed in is never changed.
        self.assertFalse(
            FIXTURE["graphql"]["repository"]["pullRequests"]["nodes"][0]["reviewThreads"]["nodes"][0]["isResolved"]
        )

    def test_a_reply_reads_back_like_githubs(self):
        # GitHub never sends null for a new comment's ID or URL; a reply
        # once came back without `fullDatabaseId` and under `pull/0`.
        reply = "mutation($id: ID!) { addPullRequestReviewThreadReply(input: {pullRequestReviewThreadId: $id, body: \"ok\"}) { comment { url } } }"
        query = "{ repository { pullRequest(number: 7) { reviewThreads { nodes { comments { nodes { fullDatabaseId url path author { __typename login } } } } } } } }"
        with FakeGitHub(FIXTURE) as fake:
            _, posted = post(fake.url + "/graphql", {"query": reply, "variables": {"id": "T1"}})
            status, rest = post(fake.url + "/repos/jdx/demo/pulls/7/comments", {"body": "too", "in_reply_to": 11})
            _, after = post(fake.url + "/graphql", {"query": query})
        url = posted["data"]["addPullRequestReviewThreadReply"]["comment"]["url"]
        self.assertEqual(url, "https://github.com/jdx/demo/pull/7#discussion_r900001")
        self.assertEqual((status, rest["in_reply_to_id"]), (201, 11))
        comments = after["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"][0]["comments"]["nodes"]
        self.assertEqual(
            comments[1],
            {"fullDatabaseId": "900001", "url": url, "path": "a.py", "author": {"__typename": "User", "login": "jdx"}},
        )
        self.assertEqual(len(comments), 3)


if __name__ == "__main__":
    unittest.main()
