---
description: Wait for CI or another condition without sleep-polling. Coming soon.
---

# jouet wait

::: info Coming soon
`jouet wait` is being built. This page describes what it is for; its flags and output will be documented when it lands.
:::

`jouet wait` blocks until a condition holds, then prints one summary. It replaces `sleep N; check; repeat` loops around `gh pr checks` and `gh run view`.

It is meant to catch what those loops miss: a conflicted pull request never gets CI, so a loop waiting for checks waits forever, and `gh run view --log-failed` refuses while a run is still in progress.

It will follow the [shared conventions](/conventions), including exit code `124` when it is still waiting at its timeout.
