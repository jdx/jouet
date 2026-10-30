import unittest

from jbench.graphql import Executor, parse

ROOT = {
    "viewer": {"__typename": "User", "login": "jdx"},
    "repository": {
        "__typename": "Repository",
        "name": "demo",
        "pullRequests": {
            "nodes": [
                {"__typename": "PullRequest", "id": "PR_1", "number": 1, "state": "MERGED", "headRefName": "a"},
                {
                    "__typename": "PullRequest",
                    "id": "PR_7",
                    "number": 7,
                    "state": "OPEN",
                    "headRefName": "b",
                    "reviewThreads": {
                        "nodes": [
                            {"__typename": "PullRequestReviewThread", "id": "T1", "isResolved": False},
                            {"__typename": "PullRequestReviewThread", "id": "T2", "isResolved": True},
                        ]
                    },
                    "checks": [
                        {"__typename": "CheckRun", "name": "test"},
                        {"__typename": "StatusContext", "context": "ci"},
                    ],
                },
            ]
        },
    },
}


def run(query, variables=None, mutations=None):
    executor = Executor(ROOT, mutations)
    return executor.execute(query, variables), executor.missing


class ParseTest(unittest.TestCase):
    def test_rejects_two_operations(self):
        response, _ = run("{ viewer { login } } { viewer { login } }")
        self.assertIn("errors", response)

    def test_strings_and_comments(self):
        doc = parse('# comment\nquery { a(b: "x \\" y", c: [1, 2.5, true, null, ENUM]) }')
        self.assertEqual(doc.selections[0].args, {"b": 'x " y', "c": [1, 2.5, True, None, "ENUM"]})


class ExecuteTest(unittest.TestCase):
    def test_singular_with_arguments_picks_from_the_plural(self):
        response, missing = run(
            'query($n: Int!) { repository(owner: "jdx", name: "demo") { pr: pullRequest(number: $n) { id } } }',
            {"n": 7},
        )
        self.assertEqual(response["data"]["repository"]["pr"], {"id": "PR_7"})
        self.assertEqual(missing, [])

    def test_connections_and_filters(self):
        response, _ = run(
            "{ repository { open: pullRequests(states: [OPEN], first: 1) { totalCount nodes { number } "
            "pageInfo { hasNextPage } } } }"
        )
        self.assertEqual(
            response["data"]["repository"]["open"],
            {"totalCount": 1, "nodes": [{"number": 7}], "pageInfo": {"hasNextPage": False}},
        )

    def test_edges(self):
        response, _ = run("{ repository { pullRequests { edges { node { number } } } } }")
        edges = response["data"]["repository"]["pullRequests"]["edges"]
        self.assertEqual([e["node"]["number"] for e in edges], [1, 7])

    def test_fragments_and_inline_fragments(self):
        response, _ = run(
            "query { repository { pullRequest(number: 7) { checks { __typename ...run ... on StatusContext { context } } } } }"
            " fragment run on CheckRun { name }"
        )
        self.assertEqual(
            response["data"]["repository"]["pullRequest"]["checks"],
            [{"__typename": "CheckRun", "name": "test"}, {"__typename": "StatusContext", "context": "ci"}],
        )

    def test_include_and_skip(self):
        response, _ = run(
            "query($x: Boolean!) { viewer { login @include(if: $x) } repository { name @skip(if: $x) } }", {"x": False}
        )
        self.assertEqual(response["data"], {"viewer": {}, "repository": {"name": "demo"}})

    def test_missing_fields_are_null_and_reported(self):
        response, missing = run("{ viewer { login email } }")
        self.assertEqual(response["data"]["viewer"], {"login": "jdx", "email": None})
        self.assertEqual(missing, ["User.email"])

    def test_node_lookup(self):
        response, _ = run('{ node(id: "T2") { ... on PullRequestReviewThread { isResolved } } }')
        self.assertEqual(response["data"]["node"], {"isResolved": True})

    def test_type_introspection_lists_fixture_fields(self):
        response, _ = run('{ t: __type(name: "PullRequestReviewThread") { fields { name } } }')
        self.assertEqual({f["name"] for f in response["data"]["t"]["fields"]}, {"id", "isResolved"})

    def test_mutation(self):
        calls = []

        def resolve(args, _):
            calls.append(args)
            return {"thread": {"isResolved": True}}

        response, _ = run(
            "mutation($id: ID!) { resolveReviewThread(input: {threadId: $id}) { thread { isResolved } } }",
            {"id": "T1"},
            {"resolveReviewThread": resolve},
        )
        self.assertEqual(response["data"]["resolveReviewThread"]["thread"]["isResolved"], True)
        self.assertEqual(calls, [{"input": {"threadId": "T1"}}])

    def test_unknown_mutation_is_an_error(self):
        response, _ = run("mutation { mergePullRequest(input: {}) { clientMutationId } }")
        self.assertIn("no mutation mergePullRequest", response["errors"][0]["message"])


if __name__ == "__main__":
    unittest.main()
