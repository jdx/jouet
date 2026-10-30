---
description: List, reply to and resolve pull request review threads, without hand-written GraphQL. Coming soon.
---

# jouet pr threads

::: info Coming soon
`jouet pr threads` is being built in an open pull request. This page says what it is for; its flags and output will be documented when it lands.
:::

`jouet pr threads` will list, reply to and resolve review threads. It replaces hand-written queries like this one, plus one more call for each reply and each resolve:

```sh
gh api graphql -f query='query {
  repository(owner: "jdx", name: "jouet") {
    pullRequest(number: 42) {
      reviewThreads(first: 100) {
        nodes { id isResolved comments(first: 20) { nodes { body path line } } }
      }
    }
  }
}'
```

Those scripts tend to go wrong in the same two ways: they find new comments by `commit_id` instead of by time, and they post replies such as "Fixed in abc123" that name a commit that was never pushed.

It will follow the [shared conventions](/conventions).
