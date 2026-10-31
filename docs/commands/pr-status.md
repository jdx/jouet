---
description: One line per pull request with the reasons it needs attention, from one batched GitHub query.
---

# jouet pr status

`jouet pr status` gives one line per pull request, with the reasons each one needs attention. It replaces `for n in …; do gh pr view $n; done` loops, and catches what those loops miss: `mergeable: UNKNOWN` means GitHub has not finished computing the answer, not that there is no conflict, and a pull request with no CI runs is often one with a merge conflict.

```text
jouet pr status [PR...] [--mine] [--search QUERY] [--repo OWNER/NAME]...
                [--settle DUR] [--fail-on-attention]
```

The flags are listed in the [CLI reference](/cli/pr/status).

For the single next action on each pull request, with the command that does it, use [`jouet pr next`](/commands/pr-next).

```output
$ jouet pr status --repo jdx/mise 14955 15147 15187 99999999
#14955 draft  fix(hook-env): install the error...        CONFLICT  ci 25/51 fail: untrusted / lint, untrusted / test-ci, test-ci  threads 0  review -
#15147 draft  fix(backend): lock pipx dependency...      CONFLICT  ci 28/50                                                       threads 1  review -
#15187 merged fix(dotfiles): keep saving a directory...  -         ci 26/48                                                       threads 0  review -
not found: jdx/mise#99999999
3 PRs, 2 need attention: #14955 (conflict, checks_failing, draft), #15147 (conflict, unresolved_threads, draft); 1 not found
```

## Choosing pull requests

- **A pull request** can be given as a number, a URL, `owner/repo#123` or a branch name. A GitLab merge request or a Forgejo pull request names its host, as in `gitlab.com/group/project!12`; see [GitLab and Forgejo](/forges). With none given, it is the pull request for the current branch. For a branch, an open pull request wins over a closed one.
- **`--mine`** adds the open pull requests authored by the token's user.
- **`--search QUERY`** adds the results of a [GitHub search](https://docs.github.com/en/search-github/searching-on-github/searching-issues-and-pull-requests). `is:pr` is added for you.
- **The repository** comes from `--repo`, which can be repeated, or else from the git remotes, picked in the order gh uses: a remote set with `gh repo set-default`, then `upstream`, `github`, `origin`. `--mine` and `--search` cover every `--repo`, or this checkout's repository, unless the query names its own `repo:`, `org:` or `user:`.

Arguments and selectors combine, and a pull request named twice is reported once.

## What each line shows

Each pull request reports its state (`open`, `draft`, `merged` or `closed`), whether it can merge (`ok`, `CONFLICT` or `unknown`), its checks as passing out of all, with the names of the failing ones, its unresolved review threads and its review decision. `auto-merge` is added when auto-merge is enabled. Titles are cut at a word boundary after 40 characters. The summary line is always last.

## Attention

Every open pull request lists the reasons it needs attention, as fixed codes with a message for each:

| Code | When |
|---|---|
| `conflict` | It conflicts with its base. `pull_request` workflows will not run until it is rebased |
| `checks_failing` | A check failed. The message names every failing check |
| `checks_missing` | No checks registered more than 5 minutes after the last push |
| `changes_requested` | A reviewer requested changes |
| `unresolved_threads` | Review threads are unresolved |
| `entire_findings` | The Entire Gates check fails on open findings. The message names the first to fix, when the `entire` CLI could list them |
| `entire_gates` | The Entire Gates check fails on a gate jouet does not know, or names none |
| `behind_base` | GitHub requires the branch to be updated before it can merge, or the Entire Gates check fails because it is not up to date |
| `draft` | It is a draft |
| `mergeable_unknown` | GitHub still had not computed mergeability after `--settle` |

Merged and closed pull requests never need attention.

To block until a pull request has no attention codes left, or until one appears that only the agent can clear, use [`jouet wait pr --until ready`](/commands/wait#wait-pr).

## Entire Gates

A pull request whose [Entire Gates](/entire-gates) check fails gets a line saying which gates fail and why, and, when the findings gate fails and the `entire` CLI is on PATH and logged in, up to three open findings with the handle `jouet pr threads` takes:

```text
#13958 draft feat(install): turn the identity...  CONFLICT  ci 29/55 fail: trusted / windows-e2e, trusted / test-ci, test-ci, Entire Gates  threads 0  review -
  entire: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity (trail #424)
    [entire:01M4C997EA] e2e/tasks/test_task_monorepo_deps_tool_install:44  medium  Identical `rm -rf "${MISE_DATA_DIR}/installs/tiny/1"* 2>/dev/null || true` pattern as...
    [entire:01M4C997DZ] e2e/cli/test_deps_tool_install:21  medium  `rm -rf "${MISE_DATA_DIR}/installs/tiny/1"* 2>/dev/null || true` only removes the compat symlink...
```

The check still counts in the `ci` column, as GitHub reports it, but never in `checks_failing`: a failed checks gate repeats the checks already named there, a branch that is not up to date is `behind_base`, and open findings are `entire_findings`. A line that only repeats other failures ends `; repeats what is reported above`.

## Waiting for mergeability with `--settle`

GitHub computes mergeability lazily, so a pull request that has not been looked at recently reports `UNKNOWN`. `--settle DUR` (default `20s`) re-queries those pull requests until GitHub answers or the time runs out. `--settle 0` turns this off.

## JSON

`--json` prints one document. Fields that are null, false, zero or empty are left out:

```json
{
  "prs": [
    {
      "repo": "jdx/mise",
      "number": 14955,
      "title": "fix(hook-env): install the error reporter before the fast path",
      "url": "https://github.com/jdx/mise/pull/14955",
      "state": "draft",
      "base": "main",
      "head": "fix/hook-env-error-hook-order",
      "head_sha": "a03b3ef655",
      "pushed_at": "2026-10-25T13:25:21Z",
      "mergeable": "conflicting",
      "merge_state": "dirty",
      "checks": {
        "pass": 25,
        "fail": 3,
        "skipped": 23,
        "failing": ["untrusted / lint", "untrusted / test-ci", "test-ci"]
      },
      "attention": [
        { "code": "conflict", "message": "Conflicts with main; pull_request workflows will not run until it is rebased" },
        { "code": "checks_failing", "message": "3 failing: untrusted / lint, untrusted / test-ci, test-ci" },
        { "code": "draft", "message": "The PR is a draft" }
      ]
    }
  ],
  "not_found": ["jdx/mise#99999999"]
}
```

- `review` is GitHub's review decision, lowercased: `approved`, `changes_requested` or `review_required`.
- `pushed_at` is the later of the head commit's committer date and the last force push, because GitHub's API records no time for a plain push.
- `incomplete` lists searches that matched more pull requests than GitHub lists (1,000), with how many matched and how many were listed.
- `entire`, on a pull request with an Entire Gates check, holds the check's `state` (`passed`, `failed`, `pending` or `neutral`), its `title`, each of its `gates` with `gate`, `status` and `reason`, the `trail` number and `url`, and, when the findings gate fails, the open `findings` (`id`, `handle`, `severity`, `path`, `line`, `title`). `findings_incomplete` says Entire listed more than it let jouet read, and `error` why `entire` could not be asked.
- `entire` at the top level, present when some pull request's findings gate fails, is `{"available": true}` or `{"available": false, "reason": "..."}`.

## GitHub access

- **Tokens** are tried in order: `JOUET_GITHUB_TOKEN`, `GH_TOKEN`, `GITHUB_TOKEN`, then `gh auth token`. A token GitHub rejects moves on to the next one, with a warning such as `GH_TOKEN was rejected (401); using gh auth token`, so a stale `GH_TOKEN` no longer needs `GH_TOKEN= gh …`.
- **One query** fetches up to 50 pull requests. The names of failing checks come from a second query, made only when some check failed. Review threads, failing checks and search results past the first page are always fetched, never cut off.
- **Retries.** Server errors, dropped connections and GitHub's secondary rate limits are retried with backoff. Each request times out after 30 seconds.
- **`JOUET_GITHUB_API_URL`** points jouet at another API root, such as a GitHub Enterprise server.

## Exit codes

| Code | When |
|---|---|
| `0` | Every pull request was found |
| `1` | With `--fail-on-attention`, at least one pull request needs attention |
| `2` | Bad arguments |
| `3` | A pull request was not found. The others are still reported |
| `5` | Authentication, network or rate-limit failure |
