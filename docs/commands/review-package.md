---
description: One file for a reviewer, holding a range's commits, a diffstat and the full diff with extra context, refusing a range that would show the wrong diff.
---

# jouet review-package

`jouet review-package` writes one file that holds everything a reviewer needs for a range of commits: the commits, a diffstat, and the full diff with extra context. Hand the file to a reviewing agent or a person instead of a branch name and a list of commands to run. The idea comes from obra/superpowers.

```text
jouet review-package BASE [HEAD] [-o FILE] [--context N]
```

```sh
$ jouet review-package origin/main
wrote /home/me/src/app/.git/jouet/review-1a2b3c4d5e6f-7a8b9c0d1e2f.md
  origin/main..HEAD: 3 commits, 5 files changed, +120 -31
```

`HEAD` defaults to `HEAD`. The flags are listed in the [CLI reference](/cli/review-package).

## The file

It is markdown:

1. a header naming the base and head, with their full SHAs, and the totals;
2. **Commits**, oldest first: the short SHA, the subject, the author and the date;
3. **Diffstat**, with renames and new or deleted files listed;
4. **Diff**: the full diff, with renames detected and `--context` lines (default 10) around each change.

The diffstat and diff sit in code fences longer than any run of backticks inside them, so a diff of a markdown file cannot end a fence early.

The file goes to `<git dir>/jouet/review-BASE-HEAD.md`, which is outside the working tree and so never committed. `-o FILE` writes it elsewhere, and `-o -` prints it to stdout.

## Refusals

A package is refused, with nothing written, when:

- **BASE is not an ancestor of HEAD (exit 4).** The diff would then include changes HEAD does not make. The hint names the merge base to use instead, or notices that the two arguments are swapped.
- **The range is empty (exit 4).** There is nothing to review.
- **BASE or HEAD names no commit (exit 3).**

```text
jouet: main is not an ancestor of feat/x, so the diff would include changes feat/x does not make
hint: their merge base is 1a2b3c4d5e6f; to review what feat/x adds since then: jouet review-package 1a2b3c4d5e6f feat/x
```

## JSON

```json
{
  "path": "/home/me/src/app/.git/jouet/review-1a2b3c4d5e6f-7a8b9c0d1e2f.md",
  "base": { "name": "origin/main", "sha": "1a2b3c4d5e6f..." },
  "head": { "name": "HEAD", "sha": "7a8b9c0d1e2f..." },
  "commits": 3,
  "files": 5,
  "insertions": 120,
  "deletions": 31
}
```

With `-o -`, `path` is left out and `text` holds the package.
