---
description: Merge a pull request that is ready, through the API, without bypassing branch protection, and confirm it merged.
---

# jouet pr land

`jouet pr land` merges a pull request that [`jouet pr next`](/commands/pr-next) says is ready, and confirms that it merged. It follows steipete's `/landpr`, adapted so that nothing can be merged by accident: it refuses anything that is not ready or not tested, never force-pushes, and never bypasses branch protection.

```text
jouet pr land [PR] [--repo OWNER/NAME] [--method squash|merge|rebase] [--update]
              [--yes] [--dry-run] [--timeout DUR] [--interval DUR] [--settle DUR]
              [--skip-verify REASON]
```

The flags are listed in the [CLI reference](/cli/pr/land). `PR` is a number, a URL, `owner/repo#123` or a branch, and defaults to the current branch's pull request.

## What it does

1. **Refuse unless ready.** It fetches the pull request as `jouet pr next` does. Unless the answer is `ready_to_merge`, it exits `4` and says what is missing and what to run next. With `--update`, `behind_base` is accepted too.
2. **Refuse unless tested.** While jouet's hooks keep a ledger, it also exits `4` unless a full test run passed at the head's tree (see [below](#tests-before-landing)).
3. **Update, with `--update`.** A branch that is behind its base is updated on GitHub with the update-branch endpoint, which merges the base into it. jouet never rebases or force-pushes. The request names the head that was checked, so a push in between makes GitHub refuse it.
4. **Wait for checks.** After an update, it waits for the new head's checks with the same waiting as [`jouet wait ci`](/commands/wait), up to `--timeout` (default `30m`). A failed check stops it. Then the pull request goes through step 1 again, since an update can dismiss an approval.
5. **Merge.** It merges with `--method` (default `squash`) through the merge endpoint, naming the head SHA it checked. This is the merge a person gets from the button: branch protection applies, and nothing asks to bypass it. If GitHub refuses, jouet reports GitHub's reason and exits `4`.
6. **Confirm.** It reads the pull request back until GitHub reports it merged, and prints the merge commit. If the response to the merge was lost, this step finds out whether it happened; the merge itself is never sent twice.

```text
$ jouet pr land 7 --repo jdx/demo --yes
jouet: squash-merge jdx/demo#7 at aaaaaaa111
merged jdx/demo#7 (squash) as ccccccc3333333333333333333333333333333333
  https://github.com/jdx/demo/pull/7
```

## Tests before landing

jouet's hooks record every test run with its scope and the tree it ran against ([how](/setup#finishing-on-a-failing-check)). Before anything is sent to GitHub, `jouet pr land` hashes the tree of the pull request's head the same way and looks in the ledger for the newest full test run at that tree. It refuses (exit `4`) when there is none, when that run failed, or when the head is not in the local clone to hash (fetch it). A partial run (`cargo test -p a`, one test file, a name filter) never counts, and a run in another checkout of the same content does.

```text
$ jouet pr land 7 --yes
jouet: jdx/demo#7 was not merged: no full test run has passed at its head (aaaaaaa111); partial runs do not count
hint: run the full test suite on this commit, or pass --skip-verify REASON
```

`--skip-verify REASON` lands it anyway, for a change the suite cannot cover or one CI already ran in full, and keeps the reason in the ledger as a `skip-verify` line. The check is of the head before any `--update`: the merge of the base that `--update` makes is checked by the pull request's own CI. Without a ledger (`hooks.ledger` or `hooks.verify` off, or hooks that have never run) there is nothing to check against, and it is skipped.

## Asking first

`jouet pr land` is the one jouet command that can ask a question, and it asks only a person:

- **`--dry-run`** prints the plan and changes nothing.
- **`--yes`** merges without asking.
- **Neither:** when stdin and stderr are both terminals and no coding agent is detected, it shows the plan on stderr and asks `Merge OWNER/NAME#N? [y/N]`. Only `y` or `yes` goes ahead. Anyone else, an agent at a terminal included, gets a usage error (exit `2`) before anything is sent to GitHub. Agents are detected the same way as for [display modes](/conventions#display-modes); `--display` and `JOUET_DISPLAY` do not count, because they choose how output looks, not who is there to answer. `--json` never asks either.

```text
$ jouet pr land 7 --repo jdx/demo --dry-run --update --method merge
jouet pr land would:
  1. merge main into feat on GitHub (update-branch, never a force-push)
  2. wait up to 30m for the checks on the new head
  3. merge jdx/demo#7 at the new head through the API, honouring branch protection
  4. confirm jdx/demo#7 is merged
dry run: nothing was changed
```

## JSON

```json
{
  "repo": "jdx/demo",
  "pr": 7,
  "url": "https://github.com/jdx/demo/pull/7",
  "result": "merged",
  "method": "squash",
  "head_sha": "aaaaaaa111",
  "plan": [
    "squash-merge jdx/demo#7 at aaaaaaa111 through the API, honouring branch protection",
    "confirm jdx/demo#7 is merged"
  ],
  "merge_commit": "ccccccc3333333333333333333333333333333333"
}
```

After `--update`, `updated` is `true`, `head_sha` is the new head, and `ci` is the [`jouet wait ci` document](/commands/wait) for its checks, failure logs included. `result` is `merged`, `dry_run`, or, when the checks after `--update` did not pass, `ci_failed`, `conflict` or `timeout`. A refusal is an error document, `{"error": {"code": "precondition", "message": "...", "hint": "..."}}`, like any other.

## Exit codes

| Code | When |
|---|---|
| `0` | Merged and confirmed, or a dry run of a pull request that is ready |
| `1` | The checks failed after `--update`; nothing was merged |
| `2` | Bad arguments, or neither `--yes` nor `--dry-run` with no person to ask |
| `3` | The pull request was not found |
| `4` | Refused: not ready to merge, no full test pass recorded at the head, not confirmed at the prompt, the head moved, or GitHub refused the update or the merge (branch protection, a method the repository does not allow, missing permission). Nothing was merged |
| `5` | Authentication, network or rate-limit failure, or GitHub did not report the merge |
| `124` | The checks after `--update` were still running at `--timeout` |
