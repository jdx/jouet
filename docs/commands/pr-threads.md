---
description: List, reply to and resolve pull request review threads. Coming soon.
---

# jouet pr threads

::: info Coming soon
`jouet pr threads` is being built. This page describes what it is for; its flags and output will be documented when it lands.
:::

`jouet pr threads` lists, replies to and resolves review threads. It replaces hand-written `reviewThreads` GraphQL queries and the per-comment calls that reply to and resolve them.

It is meant to catch what those scripts get wrong: new comments filtered by `commit_id` instead of time, and replies such as "Fixed in abc123" that name a commit that was never pushed.

It will follow the [shared conventions](/conventions).
