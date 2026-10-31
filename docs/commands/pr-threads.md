---
description: List, reply to and resolve pull request review threads, with one number per thread and no hand-written GraphQL.
---

# jouet pr threads

`jouet pr threads` lists, replies to and resolves review threads. It replaces hand-written `reviewThreads` GraphQL queries and the per-comment calls that reply to and resolve them, and it catches what those scripts get wrong: new comments filtered by `commit_id` instead of by when they were posted, and replies such as "Fixed in abc123" that name a commit that was never pushed.

```text
jouet pr threads [PR] [--all | --resolved] [--outdated | --no-outdated]
                 [--author LOGIN]... [--bots | --no-bots] [--since TIME]
                 [--path GLOB]... [--include-reviews] [--context N]
                 [--full] [--fail-if-unresolved] [--repo OWNER/NAME]

jouet pr threads reply     ID (--body TEXT | --body-file FILE) [--resolve] [PR]
jouet pr threads resolve   ID... [--reply TEXT] [PR]
jouet pr threads unresolve ID... [PR]
jouet pr threads apply     (--file FILE | -) [PR]
```

The flags are listed in the [CLI reference](/cli/pr/threads).

```output
$ jouet pr threads jdx/mise#14922
jdx/mise#14922  3 unresolved of 6 threads (1 outdated)

[4385981140] src/hook_env.rs:515  coderabbitai[bot]  3d ago  outdated
  **Minor** · **Functional Correctness** · **Quick win**

  **Do not treat option values as short flags.**

  For `mise hook-env -EJava`, `J` is part of the `-E` value, not `-J`. ...

  [details: Prompt for AI Agents]

[4386056098] docs/ide-integration.md:139  coderabbitai[bot]  3d ago
  ...
```

## Thread IDs

Each thread is addressed by the ID of its first comment: the number in the web UI's `#discussion_r…` links, such as `4385981140`. jouet works out the GraphQL thread ID that resolving needs and the thread a reply goes under, so an agent handles one number per thread. Giving the ID of a later comment fails with a message naming the thread it belongs to.

On GitLab a thread is a resolvable discussion, and its ID is its first note's, the number in `#note_…` links. On Forgejo and Gitea, review comments on the same line make one thread, and its ID is its first comment's. Forgejo's API cannot resolve a thread, so `--resolve` there exits 4 before anything is posted; Gitea 1.26 and later can. See [GitLab and Forgejo](/forges).

The pull request is chosen as for [`jouet pr status`](/commands/pr-status#choosing-pull-requests): a number, a URL, `owner/repo#123` or a branch, else the current branch's pull request, with the repository from `--repo` or the git remotes.

## Listing

- **What is listed.** Unresolved threads, outdated ones included and marked. `--all` adds resolved threads and `--resolved` lists only those. `--outdated` and `--no-outdated` keep or drop threads whose line has since changed.
- **Who started it.** `--author LOGIN` (repeatable) keeps threads started by that user; `coderabbitai` matches `coderabbitai[bot]`. `--bots` and `--no-bots` keep or drop threads started by bots: logins ending in `[bot]`, plus any listed in the config's `bots` or in `JOUET_BOTS`.
- **Where.** `--path` (repeatable) takes a glob, where `*` and `?` stay within a directory and `**` crosses them, a directory such as `src/backend`, or a file name such as `*.rs`.
- **Reviews and comments.** `--include-reviews` also lists review summaries and the pull request's conversation comments, as `kind: "review"` and `kind: "comment"`. They cannot be replied to or resolved here.
- **How to reply.** When it shows an unresolved thread, the text listing ends with the `reply` usage, since that is where an agent has the IDs and decides to reply.
- **Context.** `--context N` shows N lines of the file around each thread's line. They come from the working tree when HEAD is the pull request's head and the file is unchanged, and from GitHub otherwise. A comment on the base side of the diff, such as one on a deleted line, is marked `base side`: its line numbers belong to the base version, so its context comes from the merge base, as GitHub's diff shows it.

### `--since`

`--since TIME` keeps threads with any comment posted at or after `TIME`. It goes by when comments were posted, never by the commit they are attached to: review bots post on the commit they reviewed, which can be older than comments made after it.

`TIME` is RFC 3339 (`2026-10-30T11:00:00Z`), a duration ago (`2h`), or `@SHA`, meaning when that commit was pushed to the pull request. `--since @SHA` is the usual "what did the bots say about my last push" query. The push time comes from the head repository's activity log, so it works for pull requests from forks, for a commit pushed together with later ones, and for a head that a later force-push replaced. If the log has no push of the commit, jouet uses the commit's own time, which can only be earlier, and says so.

### Condensed bodies

Review bots wrap a few sentences of finding in state comments, badges and collapsed prompts. By default each comment is condensed:

- HTML comments and badge images are removed. A badge with alt text, such as greptile's priority, becomes `[P1]`.
- `<details>` blocks collapse to `[details: SUMMARY]`.
- Each comment is cut at 30 lines, ending with `... N more lines (--full shows all)`.

Suggestion blocks are always kept whole, even inside a collapsed `<details>`. `--full` shows bodies unaltered.

## Replying and resolving

```output
$ jouet pr threads apply - <<'JSON'
[{"id": 4385981140, "reply": "Fixed in {sha}.", "resolve": true},
 {"id": 4386056098, "resolve": true}]
JSON
[4385981140] replied, resolved  https://github.com/jdx/mise/pull/14922#discussion_r…
[4386056098] resolved
```

`apply` takes a JSON list of `{"id", "reply", "resolve"}` entries from `--file` or stdin, so a whole review sweep is posted in one call. `reply`, `resolve` and `unresolve` do the same for threads named on the command line. A pull request given after the IDs of `resolve` or `unresolve` must not be a bare number, which would read as a thread ID: write it as `#123`.

- **Validate first, then write.** Every ID is checked before anything is posted. An unknown ID fails the whole call and posts nothing. A thread someone already resolved, often a review bot after a push, still takes your reply and stays resolved; resolving it again does nothing. `--ignore-resolved` leaves resolved threads alone instead.
- **`{sha}`** in a reply becomes the short SHA of HEAD. jouet refuses it unless HEAD is on the pull request's branch on GitHub, so a reply never names a commit reviewers cannot see.
- **Footer.** `JOUET_REPLY_FOOTER`, or `reply_footer` in `~/.config/jouet/config.toml` (under `$XDG_CONFIG_HOME` when that is set), is appended to every reply after a blank line. An empty `JOUET_REPLY_FOOTER` turns it off.
- **Empty replies** are a usage error, never an empty comment.
- **Retries never double-post.** A reply whose exact text is already the thread's last comment from you is not posted again, and a thread that is already resolved is not resolved again, so re-running the same file after a failure only does what is left.
- **A failure stops the batch.** The output lists what was posted and what was not attempted, and the exit code is 5. Writes are never retried automatically, because a failed response can follow a write that succeeded.
- **`--dry-run`** checks everything and prints each reply exactly as it would be posted.

Each thread gets one line: `replied`, `resolved`, `unresolved`, `reply already posted`, `skipped: REASON` or `failed: REASON`, then the reply's URL.

## Entire findings

When the `entire` CLI is on PATH and logged in, the open findings on the pull request's [Entire](/entire-gates) trail are listed after the threads, under a heading that names the trail:

```text
Entire trail #424: 2 open findings
These are Entire's, not GitHub threads: resolving one leaves the other as it was.

[entire:01M4C997EA] e2e/tasks/test_task_monorepo_deps_tool_install:44  medium  agent:trail-review  23d ago
  Identical `rm -rf "${MISE_DATA_DIR}/installs/tiny/1"* 2>/dev/null || true` pattern as test_deps_tool_install: ...
```

- **Handles.** A finding is addressed as `entire:` and the start of its ID, ten characters unless two findings share them. Any unique start of eight characters or more works, and so does the whole 26-character ID on its own. A number is always a thread.
- **Filters.** `--all` adds resolved and dismissed findings, `--resolved` lists only those. A stale finding, raised on an earlier version of the code, counts as outdated for `--outdated` and `--no-outdated`; an agent's (`agent:...`) counts as a bot's for `--bots` and `--no-bots`. `--author`, `--path` and `--since` apply as to threads. Bodies are condensed the same way.
- **Resolving.** `resolve entire:ID --reply REASON` runs `entire trail finding resolve`, with the reply as the reason Entire records. `--dismiss` dismisses instead, and needs a reason. `reply entire:ID --body REASON` takes `--resolve` or `--dismiss`: Entire has no reply to a finding of its own. `unresolve entire:ID` reopens one. In `apply`, an entry for a finding has its handle as the `id`, with `"resolve": true`, `"dismiss": true` or `"unresolve": true`.
- **Separate from threads.** Findings and threads can be named in one call. Each is written by its own system: a finding never resolves a thread, and a thread never resolves a finding.
- **The same safeguards.** Every handle is matched before anything changes, `{sha}` and the reply footer apply to the reason, a finding already resolved or dismissed is left alone (as a resolved thread is), the first failure stops the batch, and `--dry-run` prints each `entire` command instead of running it.
- **No GitHub needed** for findings on a branch's trail: `jouet pr threads resolve entire:ID BRANCH --reply ...` asks only `entire`, unless the reason uses `{sha}`.
- **Without `entire`**, listing carries on with the threads alone and `--json` says why under `entire`; changing a finding is refused with exit 4.

## JSON

`--json` prints one document. Fields that are null, false or empty are left out:

```json
{
  "repo": "jdx/mise",
  "pr": 14922,
  "counts": { "unresolved": 3, "resolved": 3, "outdated": 1 },
  "threads": [
    {
      "id": 4386056098,
      "node_id": "PRRT_kwDOIvux3s6wlY_C",
      "kind": "thread",
      "path": "docs/ide-integration.md",
      "line": 139,
      "start_line": 139,
      "original_line": 139,
      "author": "coderabbitai[bot]",
      "is_bot": true,
      "url": "https://github.com/jdx/mise/pull/14922#discussion_r4386056098",
      "comments": [
        {
          "id": 4386056098,
          "author": "coderabbitai[bot]",
          "created_at": "2026-10-27T07:23:29Z",
          "body": "**Minor** · **Maintainability & Code Quality** · ...",
          "body_truncated": true
        }
      ]
    }
  ]
}
```

- `counts.outdated` counts unresolved threads that are outdated.
- `since` is the `--since` cutoff in UTC, when one was given.
- A comment's `suggestion` holds the text of its first suggestion block; `body` keeps every block.
- `start_line` is the first line of a comment on a range of lines. A range that starts on the other side of the diff has none.
- `side` is `left` for a comment on the base side of the diff, whose `line` is a line of the base version. It is absent for the head's side.
- `context` is `{"start": N, "lines": [...]}` with `--context`.
- `entire` holds the findings, apart from `threads` and `counts`, which stay GitHub's: `available` (and the `reason` when it is false), the `trail` number and `url`, and `findings`, each with `handle`, `id`, `status`, `severity`, `stale`, `path`, `line`, `end_line`, `title`, `body`, `author`, `created_at` and `status_reason`. `incomplete` says Entire listed more than it let jouet read.
- `unreadable` counts comments left out because GitHub sent no ID for them, in `fullDatabaseId` or in the URL. A thread whose first comment is one of them is left out whole, since that ID is its handle. The listing still succeeds, and the text output says how many were left out.

Writes print `{"repo", "pr", "dry_run", "results": [...]}`, one result per thread with `replied`, `already_replied`, `resolved`, `unresolved`, `url`, `skipped` or `failed`, and in a dry run the `body` that would be posted. Findings changed are under `entire: {"trail", "results": [...]}`, each with the handle as given (`id`), the whole ID (`finding`), `resolved`, `dismissed` or `reopened`, `already` (`resolved`, `dismissed` or `open`, when it was left alone), `failed`, and in a dry run the `command`. `pr` is absent when only findings on a branch's trail were changed.

## Exit codes

| Code | When |
|---|---|
| `0` | Listed, or every write was applied or skipped, even with unresolved threads |
| `1` | With `--fail-if-unresolved`, a listed thread is unresolved |
| `2` | Bad arguments, an empty reply, a malformed `apply` list, or a dismissal without a reason |
| `3` | The pull request, a thread ID, a finding, its trail, or the `@SHA` commit does not exist |
| `4` | Nothing was posted: `{sha}` names a commit that is not on the pull request's branch, or a finding was named and `entire` is missing or logged out |
| `5` | Authentication, network or rate-limit failure, including a batch stopped part way |
