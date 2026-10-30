---
description: One line per pull request with the reasons it needs attention. Coming soon.
---

# jouet pr status

::: info Coming soon
`jouet pr status` is being built. This page describes what it is for; its flags and output will be documented when it lands.
:::

`jouet pr status` gives one line per pull request, with the reasons each one needs attention. It replaces `for n in …; do gh pr view $n; done` loops.

It is meant to catch what those loops miss: `mergeable: UNKNOWN` means GitHub has not finished computing the answer, not that there is no conflict, and a pull request with no CI runs is often one with a merge conflict.

It will follow the [shared conventions](/conventions).
