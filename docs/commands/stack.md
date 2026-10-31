---
description: Check a stack of PRs, layer by layer, by ancestry and by content, so a restack that kept the commits but reverted their changes is caught.
---

# jouet stack

`jouet stack status` checks a stack of PRs, each built on the branch of the one below. For each layer it shows the layer's PR, and whether the layer really contains the layer below it.

```text
jouet stack status [BRANCH] [--json]
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
