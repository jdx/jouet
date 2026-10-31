---
description: Using jouet's PR and CI commands with GitLab merge requests, Forgejo or Gitea pull requests and the trails of repositories Entire hosts, and what each forge cannot do.
---

# GitLab, Forgejo and Entire

`jouet pr status`, `pr next`, `pr threads`, `wait ci`, `wait pr` and `wait run` work with GitLab merge requests, with Forgejo and Gitea pull requests, and with the trails of [repositories Entire hosts](#entire-hosted-repositories), as well as with GitHub. They print the same lines, the same JSON fields and the same exit codes on every forge. When a forge has no equivalent of something, the command exits 4 with the reason, before it sends anything.

Support for these forges is new and has only been tested against recorded responses. Report what breaks on your server as an [issue](https://github.com/jdx/jouet/issues).

## Naming a repository

A repository is on GitHub unless it names a host:

- **From the checkout:** the git remotes, picked in the order gh uses. A remote on github.com, gitlab.com, codeberg.org or gitea.com counts, as do an Entire `entire://` remote and one on a host named in the config. When a checkout has both a GitHub remote and a GitLab or Forgejo mirror, the GitHub remote wins.
- **`HOST/OWNER/NAME`**, as in `--repo gitlab.com/group/sub/project` or `codeberg.org/forgejo/forgejo#15077`. On GitLab the owner is a group path and can hold slashes, so a repository is split at its last `/`.
- **A URL:** `https://gitlab.com/group/project/-/merge_requests/12`, `https://codeberg.org/owner/repo/pulls/7` or an Entire trail's, `https://entire.com/et/project/repo/trails/3`. A GitLab MR can also be written `!12`.

```output
$ jouet pr status codeberg.org/forgejo/forgejo#15077
#15077 draft WIP: feat: Add support for OIDC...  CONFLICT  ci 2/16 fail: requirements / merge-conditions (pull_request), testing / backend-checks (pull_request)  threads 1  review -
1 PR, 1 needs attention: #15077 (conflict, checks_failing, unresolved_threads, draft)
```

## A self-hosted forge

Name the host in `~/.config/jouet/config.toml`:

```toml
[forges."git.example.com"]
kind = "gitlab"                 # or "forgejo"; "gitea" is the same API
token_env = "CORP_GITLAB_TOKEN" # optional: the variable holding this host's token
```

The API root is `https://HOST/api/v4` for GitLab and `https://HOST/api/v1` for Forgejo. Set `api_url` in the table only when the server is somewhere else.

A host that is neither known nor configured is refused with exit 4, and the message names the table to add. A self-hosted GitHub Enterprise server is still set with `JOUET_GITHUB_API_URL`.

## Tokens

Each token is sent only to the host it belongs to, so a token for one server never reaches another of the same kind:

| Token | Sent to |
|---|---|
| `JOUET_GITLAB_TOKEN`, `GITLAB_TOKEN`, `GITLAB_ACCESS_TOKEN` | the host in `GITLAB_HOST`, as glab reads it, else gitlab.com |
| `JOUET_FORGEJO_TOKEN`, `FORGEJO_TOKEN` | codeberg.org |
| `GITEA_TOKEN` | gitea.com |
| the variable a host's `token_env` names | that host |
| the host's `token` in glab's `config.yml`, or tea's login for its URL | that host |

The config's `token_env` is tried first, then the variables, then the CLI's config. A token the forge rejects (401) moves on to the next source. With no token at all, requests are sent without one. That is enough for public Forgejo repositories, but gitlab.com wants a token even for a public MR's discussions and checks. Errors name the source of a rejected token, such as `GITLAB_TOKEN` or `glab's config`, and never its value. A glab token kept in the system keyring is not read; set `GITLAB_TOKEN` instead.

## What maps to what

| jouet | GitLab | Forgejo and Gitea |
|---|---|---|
| checks | the head's commit statuses: its pipeline jobs and any external statuses. A job allowed to fail counts as skipped, and every other job is required | the head's commit statuses, where Forgejo Actions reports its jobs |
| a failed check's log | the job's trace, without its section markers | none: the API gives no job logs, so the check keeps its URL |
| review threads | resolvable discussions. A thread's ID is its first note's, the number in `#note_` links | review comments on the same file and line, as the web UI groups them. A thread's ID is its first comment's |
| a reply | a note in the discussion | a one-comment review on the thread's line |
| `review` | `approved` once no approvals are left, `review_required` while some are | the reviewers' latest reviews: changes requested wins over an approval |
| `merge_state` | from `detailed_merge_status` (`need_rebase` is `behind`, `checking` is `unknown`, any other block is `blocked`) | `clean`, or `dirty` on a conflict |
| `--mine` | the open MRs you opened, in each repository | the open PRs you opened, in each repository |
| `--since @SHA` | the MR's versions, one per push | the commit's own time, which can only be earlier |
| `wait run` | a pipeline, by ID or URL | refused (exit 4) |

## What is refused

Each of these exits 4 with the reason:

- **`--search`** takes GitHub's search syntax, which neither forge has. Name the PRs, or use `--mine`.
- **Resolving a thread on Forgejo.** Forgejo's API has no way to resolve a review conversation (checked through Forgejo 16). Gitea added one in 1.26, and jouet uses it there. jouet asks the server's version first, so with `reply --resolve` nothing is posted when resolving would fail.
- **`pr land`** merges GitHub PRs only so far. Use `glab mr merge` or `tea pulls merge`.
- **`ci explain`** judges a failure by GitHub Actions' run history, which jouet does not read on the other forges.
- **`wait run` on Forgejo:** jouet sees Forgejo Actions only through the commit statuses it posts. Use `wait ci` on the PR.
- **`wait ci --required` on Forgejo:** its API does not say which checks branch protection requires. Name them with `--check`.
- **`jouet stack`** works with GitHub PRs only.

```output
$ jouet pr threads resolve 26828843 codeberg.org/forgejo/forgejo#15077
jouet: codeberg.org runs Forgejo 16.0.0-dev-798-568c49e1+gitea-1.22.0, whose API cannot resolve review conversations (none through Forgejo 16 can); nothing was posted
hint: reply without --resolve, or resolve it on the web
```

Not done yet: `wait ci` on GitLab and Forgejo has no `base_touched` events, and an exhausted rate limit is retried a few times and then reported, not slept through as on GitHub.

## Entire-hosted repositories

A repository [Entire](https://entire.io) hosts itself (`/et/PROJECT/REPO`, cloned from `entire://CLUSTER.entire.io/et/PROJECT/REPO`) has trails instead of pull requests. jouet treats a trail as a PR: its number is the PR's number, and `entire.com/PROJECT/REPO#3` or the trail's URL names it from outside the checkout. A remote such as `entire://CLUSTER.entire.io/gh/OWNER/REPO` is a GitHub repository that Entire mirrors, and stays a GitHub repository.

Support for Entire-hosted repositories is new. It has been tried against one test repository and is tested against responses recorded there.

Everything is read and written through the `entire` CLI, which must be on `PATH` and logged in (`entire auth status`). jouet never handles an Entire token: `entire` finds its own login, or `ENTIRE_TOKEN`. Without the CLI, the command fails and says why. `JOUET_ENTIRE` and `JOUET_ENTIRE_TIMEOUT_MS` apply as they do for [Entire findings](/entire-gates).

| jouet | Entire |
|---|---|
| a PR | a trail, from `entire trail show`. A branch is the trail tracking it |
| checks | the trail's check runs (Buildkite and the like), and one **Entire Gates** check made of its blocking gates, read [as on GitHub](/entire-gates) |
| review threads | the trail's discussions (`entire trail comment`), each with its title first. A thread's ID is a number made from the discussion's ID |
| a reply, `--resolve` | `entire trail comment reply`, `resolve` and `unresolve` |
| review findings | the trail's findings, after the threads, as `entire:ID`, [as on GitHub](/entire-gates) |
| `review` | each reviewer's latest decision in `entire trail approvals`, or `review_required` while the approvals gate blocks. The approvals gate is the review, not one of the gates |
| `merge_state` | `clean`, `dirty` on a conflict, or `blocked` while Entire says the trail cannot merge yet |
| `--mine` | the open and draft trails of the user `entire` is logged in as |
| `--since @SHA` | the commit's time in this clone, which can only be earlier than its push |

```output
$ jouet pr threads
jdx/jouet-entire-forge-test#1  1 unresolved of 2 threads

[1836512261783001] discussion  jdx  9m ago
  Tests

  Could subtract get a test, so the a + b slip cannot come back?

Reply with `jouet pr threads reply <ID> [PR] --body TEXT`, ID being the number in brackets; add --resolve to resolve it too, or `jouet pr threads apply` takes many at once.

Entire trail #1: 1 open finding
These are Entire's, not the trail's discussions: resolving one leaves the other as it was.

[entire:01M69HBX3M] main.rs:10  low  01KVXJKTKXPZN75VHNT4D041RQ  8m ago
  main prints both results on one line with no labels; print them as add=4 subtract=2 so the output says which is which.
```

The CLI has no route to a commit's files, the merge base or the branch's commits, so `--context`, `--since @SHA` and the `{sha}` check in replies use this clone's git, and say so when it lacks the commit.

What Entire's CLI cannot do exits 4 before anything is sent:

- **`pr land`**: the `entire` CLI has no command that merges a trail, so `pr next` names none either, and says to merge it on entire.com.
- **`wait run`** and **`ci explain`**: Entire gives a trail's checks, but no runs, jobs or job logs. A failed check keeps its URL.
- **`wait ci --required`**: the CLI does not say which checks are required. Name them with `--check`.
- **`--search`**, as on GitLab and Forgejo.
