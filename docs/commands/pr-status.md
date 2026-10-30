---
description: One line per pull request, with the reasons it needs attention. Coming soon.
---

# jouet pr status

::: info Coming soon
`jouet pr status` is being built in an open pull request. This page says what it is for; its flags and output will be documented when it lands.
:::

`jouet pr status` will print one line per pull request, with the reasons each one needs attention. It replaces loops like this one:

```sh
for n in 101 102 103; do
  gh pr view "$n" --json state,mergeable,statusCheckRollup
done
```

Those loops miss two things. `mergeable: UNKNOWN` means GitHub has not finished computing the answer, not that there is no conflict. And a pull request with no CI runs is often one with a merge conflict, which a loop like this never connects.

It will follow the [shared conventions](/conventions).
