---
description: Check a stack of PRs layer by layer, by ancestry and by content, so a restack that kept the commits but reverted their changes is caught, and bring it up to date by merging.
---

# jouet stack

`jouet stack status` checks a stack of PRs, each built on the branch of the one below. For each layer it shows the layer's PR, and whether the layer really contains the layer below it. `jouet stack sync` brings the layers up to date by merging, and verifies each merge.

```text
jouet stack status [BRANCH] [--json]
jouet stack sync [BRANCH] [--dry-run] [--push] [--no-fetch] [--json]
```

## Why

`gh stack view` shows the order a stack is meant to have, not the order its branches are actually in. A restack can also look done and not be. For example, a merge that resolved a conflict by keeping the child's side, or a rebase that dropped a commit, gives the child its parent's commits but not their changes. Every later layer then silently reverts the fix. Branches have to be checked by ancestry, and also by content.

## Before and after

`feat/cache` is built on `feat/parser`. After a fix landed on `feat/parser`, `feat/cache` was restacked, but the restack kept its own version of `src.rs`. `git log` shows the fix in its history, and `gh stack view` lists the stack in order. `jouet stack status` finds the problem:

```output
$ jouet stack status
STACK main ← feat/parser ← feat/cache (from gh-stack)
  feat/parser  contains main; 2 unpushed
* feat/cache   has feat/parser's commits but not its changes to src.rs (undoes 2904879 parser: don't panic on empty input); 2 unpushed
    restore it with `git checkout feat/parser -- src.rs` and commit, unless the revert is meant
```

It exits 1 whenever a layer needs attention, so an agent can check a stack before it pushes.

## Which branches form the stack

- **gh-stack's metadata**, when [gh-stack](https://github.com/github/gh-stack) tracks the branch: the `gh-stack` file in the worktree's git directory, or the repository's common one. One GitHub query then asks for every layer's PR. If GitHub cannot be reached, the layers are still checked and a note says that merged PRs are not known.
- **Otherwise, the PRs' bases on GitHub.** jouet follows the branch's PR down, base by base, to the default branch. It then follows the open PRs built on the branch up, for as long as there is exactly one at each step. If several PRs are built on one branch, a note says so and the stack stops there. A branch with no PR that gh-stack does not track is in no stack (exit 3).

## What is checked

For each layer, jouet reads the local branch, because it may hold fixes not yet pushed. It reads the remote-tracking branch as of the last fetch. Nothing is fetched or changed.

- **Ancestry.** Whether the parent's tip, or for the bottom layer the remote trunk, is in the layer's history. A layer without it `needs PARENT merged in`.
- **Content.** Every file the parent layer changed since it branched off its own parent must have that change in the layer's tree. jouet merges the parent's change into the layer's version of the file (`git merge-file`), which must leave the file as it is. A file whose change is not there is reported as one of:
  - `undoes COMMIT`, when the file is exactly as it was before one of the parent's commits;
  - `reverted`, when it is as it was before the parent's changes;
  - `missing`, when part of the change is not there;
  - `changed differently`, when the layer changed the same lines and jouet cannot tell.

  This is checked only when ancestry holds; a layer without its parent's commits plainly lacks their changes.
- **Merged prerequisites.** When the layer below has merged, its branch has left the stack, and the layer `needs origin/main merged in` until the trunk is an ancestor. Merging the trunk works where a rebase onto it would conflict, because a squash merge leaves the old commits behind.
- **The remote.** `N unpushed` counts local commits the remote does not have. This is information, not a problem. `N behind origin/BRANCH` counts remote commits the local branch lacks, and that is a problem: restacking from the local branch would leave them out.

## Syncing a stack

`jouet stack sync` brings each layer up to date by merging its parent into it, from the bottom of the stack up. Here a fix on `feat/parser` reaches the layers above it:

```output
$ jouet stack sync --dry-run
PLAN main ← feat/parser ← feat/cache ← feat/cli (dry run: nothing fetched, merged or pushed)
  feat/parser  up to date
  feat/cache   would merge feat/parser
  feat/cli     would merge feat/cache
$ jouet stack sync
SYNC main ← feat/parser ← feat/cache ← feat/cli
  feat/parser  up to date
  feat/cache   merged feat/parser (737a42b)
  feat/cli     merged feat/cache (e631215)
```

Each rule below comes from something that went wrong when stacks were restacked by hand:

- **It merges, and never rebases or force-pushes.** A rebase rewrites commits that other sessions and reviewers have already seen, and needs a force push. A merge needs neither.
- **It starts from the local branch, never `origin/…`.** Running `git checkout -B layer origin/layer` during a restack drops fixes that were committed but not yet pushed. sync uses the local branch, and creates it from the remote's only when there is no local branch. It refuses (exit 4) a local branch that is behind its remote, because someone pushed work the local branch lacks.
- **It merges the trunk after a prerequisite merges.** Once the layer below has merged, often as a squash, the layer gets the trunk merged in (`origin/main`). A rebase onto the trunk would conflict with the old commits instead.
- **It verifies each merge before committing it.** Each merge is made with `--no-commit`. As in `stack status`, every change the merge brings in must be in the merged tree, and a merge that would leave some out is aborted. sync also stops at a layer that is in order by history but lacks its parent's changes, because merging again cannot bring them back.
- **It stops at the first problem and says what to do.** For a conflict, it lists the files:
  - in a temporary worktree, it aborts the merge and gives the commands to redo it by hand;
  - in this worktree, it leaves the conflict for you to resolve.

  For a content problem, it names the files and the `git checkout PARENT -- FILE` command that restores them.
- **It leaves other worktrees alone.** A layer checked out in another worktree belongs to whoever works there, so sync stops (exit 4) and says to run it there. The layer checked out in this worktree is merged in place, and only when it has no uncommitted changes. A layer checked out nowhere is merged in a temporary worktree, and its branch is moved only if nobody moved it in the meantime.
- **It pushes only with `--push`, and never force-pushes.** Each layer that is ahead of its remote branch is pushed with a plain push. A layer that was never pushed is left alone.

sync fetches the remote first; `--no-fetch` skips that. `--dry-run` fetches nothing and only shows the plan. sync exits 1 on a conflict or a failed check, 4 when it refused before merging, and 5 when a fetch or push failed. Without GitHub, sync cannot tell which PRs have merged, so it merges each layer's old parent branch rather than the trunk; a note says so.

## JSON

`--json` prints `source` (`gh-stack` or `github`), `trunk`, `remote`, and `layers`, bottom first. Each layer has:

- `pr`: `number`, `url`, `state`, `base`, `head` and `head_sha`;
- `branch`, `parent`, and `local` and `remote` (commit SHAs);
- `unpushed` and `behind`;
- `contains_parent`;
- `content`: the parent's changes the layer lacks, each a `path`, a `problem`, and the `commit` it undoes when known;
- `needs_trunk` and `merged`;
- `needs`: the codes `missing_branch`, `parent_changes_missing`, `behind_remote`, `needs_trunk` and `needs_parent`.

Fields that are empty are left out. `notes` lists anything else jouet had to say.

`stack sync --json` prints `source`, `trunk`, `notes` and `report`. The report has:

- `steps`: each layer's `branch` and `action` (`up_to_date`, `merged`, `would_merge` or `skipped_merged`), with `from` and `commit` for a merge;
- `stopped`: where sync stopped, with `branch`, `reason`, `message`, `next` and `files`;
- `pushed`: for each push, `branch`, `ok` and `error`;
- `dry_run`.
