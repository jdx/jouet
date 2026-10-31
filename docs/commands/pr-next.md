---
description: The one thing each pull request needs next, the command that does it, and an exit code that says whose move it is.
---

# jouet pr next

`jouet pr next` says the one thing each pull request needs next, why, and the command that does it. It follows Kent C. Dodds' `ship-pr.mjs`: instead of reading a pull request's whole status and deciding what matters, an agent runs one command and does what it says.

```text
jouet pr next [PR...] [--mine] [--search QUERY] [--repo OWNER/NAME]... [--settle DUR]
```

The flags are listed in the [CLI reference](/cli/pr/next). Pull requests are chosen the same way as for [`jouet pr status`](/commands/pr-status#choosing-pull-requests).

`jouet pr next` runs the built-in [`pr-next` routine](/routines), so its decisions are a Luau program you can read with `joue pr-next --help` and copy into a routine of your own. The routine selects and fetches pull requests through the same primitive as `pr status`; against GitHub it takes the same time as the Rust version it replaced.

```text
$ jouet pr next --repo jdx/mise 15187 15169 15172 15205
#15187 done merged: Merged
#15169 done closed: Closed without merging; reopen it to continue
#15172 act  draft: The PR is a draft
       next: gh pr ready 15172 --repo jdx/mise
#15205 act  checks_failing: 2 failing: title, status
       next: jouet wait ci jdx/mise#15205
4 PRs: 2 to act on (#15172, #15205), 2 done (#15187, #15169)
```

## The order

The states are checked in a fixed order, and the first that applies is the answer. A pull request that is a draft and conflicts is a draft first; one that conflicts and fails CI has a conflict first, because `pull_request` CI cannot run until the conflict is gone.

| Action | Kind | When | Next |
|---|---|---|---|
| `merged` | done | It is merged | |
| `closed` | done | It was closed without merging | |
| `draft` | act | It is a draft | `gh pr ready N --repo OWNER/NAME` |
| `conflict` | act | It conflicts with its base | `git fetch origin BASE && git merge origin/BASE` |
| `mergeable_unknown` | wait | GitHub has not finished computing mergeability or the merge state | `jouet wait pr PR --until mergeable` |
| `behind_base` | act | Branch protection requires the branch to be up to date with its base | `git fetch origin BASE && git merge origin/BASE` |
| `checks_failing` | act | A check failed | `jouet wait ci PR`, which shows the failed step's log |
| `checks_pending` | wait | A check is still running, or none has registered yet on a recent push | `jouet wait ci PR` |
| `changes_requested` | act | A reviewer requested changes | `jouet pr threads PR --include-reviews` |
| `unresolved_threads` | act | Review threads are unresolved | `jouet pr threads PR` |
| `awaiting_approval` | wait | An approving review is required | `jouet wait pr PR --until approved` |
| `blocked` | wait | GitHub reports the merge as blocked by another branch protection rule or ruleset | `jouet wait pr PR --until any-change` |
| `auto_merge` | wait | Auto-merge is enabled, so GitHub will merge it | `jouet wait pr PR --until merged` |
| `ready_to_merge` | done | Nothing stands in the way | `jouet pr land PR` |

The data and the reasons come from the same query and [attention codes](/commands/pr-status#attention) as `jouet pr status`, so the two commands never disagree. `--settle` waits for an unknown mergeable state the same way.

Two choices are deliberate:

- **Updating is a merge.** Both `conflict` and `behind_base` suggest merging the base into the branch, never a rebase and force-push.
- **No checks is not a blocker on its own.** A push with no checks yet is `checks_pending` for 5 minutes. After that, a repository with no CI can still be `ready_to_merge`: branch protection knows which checks it requires, and a missing required check makes GitHub report the merge as `blocked`.

`ready_to_merge` is "done" for the agent: landing the pull request is a decision for whoever owns it, made with [`jouet pr land`](/commands/pr-land).

## JSON

```json
{
  "prs": [
    {
      "repo": "jdx/mise",
      "number": 15205,
      "title": "chore(deps): update aube to v2.8.5",
      "url": "https://github.com/jdx/mise/pull/15205",
      "action": "checks_failing",
      "kind": "act",
      "reason": "2 failing: title, status",
      "command": "jouet wait ci jdx/mise#15205"
    }
  ]
}
```

`command` is left out when there is nothing to run. `not_found` and `incomplete` are as for `jouet pr status`.

## Exit codes

`jouet pr next` answers with its exit code, so a script or an agent can branch on it without reading the output. `10` and `20` sit outside the [shared table](/conventions#exit-codes) on purpose, as in `ship-pr`, so that "act" and "wait" can never be mistaken for an error:

| Code | When |
|---|---|
| `0` | Nothing is left for the agent: every pull request is merged, closed, or ready to merge |
| `10` | The agent must act on at least one pull request |
| `20` | Nothing needs the agent, but at least one pull request is waiting on CI, a reviewer or GitHub |
| `2` | Bad arguments |
| `3` | A pull request was not found. The others are still reported |
| `5` | Authentication, network or rate-limit failure |

With several pull requests, `10` wins over `20`, and `20` over `0`.
