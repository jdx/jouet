# Entire findings and gates

[Entire](https://entire.io) keeps a trail for each branch, with review findings from its agents, and adds an **Entire Gates** check to the pull request. The check fails when any blocking gate fails:

| Gate | Fails when | What jouet does with it |
|---|---|---|
| Checks | Another check failed | Treats it as derived: the failed checks are the cause, and are reported instead |
| Findings | Findings on the trail are open | Lists them, with their location, and names the first one to fix |
| Up to date | The branch is behind its base, or conflicts with it | Treats it as derived: reports `behind_base` with the command that merges the base |

Entire sits beside the forge rather than inside it. Trail numbers are not pull request numbers, and a finding's lifecycle is Entire's own: resolving a finding leaves every GitHub review thread as it was, and resolving a thread leaves the findings.

## What needs the `entire` CLI

The gates are read from the check's summary with the GitHub token jouet already uses, so `pr status`, `pr next`, `wait ci` and `ci explain` always say which gate fails and why. A summary in a form jouet does not know is never guessed at. `pr status` and `pr next` then take the gates from `entire trail show`, and say so. Without the CLI, and in `wait ci` and `ci explain`, the check's title is shown with `(jouet could not read the check's summary)`, and the failure counts as one of its own.

The findings themselves come from the `entire` CLI, and only when it is on PATH and logged in (`entire auth status`). Without it, commands carry on: text output names the failing gates and nothing more, and `--json` says why under `entire`:

```json
"entire": { "available": false, "reason": "entire is not on PATH" }
```

jouet runs `entire` to read and change findings and never handles its credentials. Every call has a timeout (`[entire] timeout_ms`, 5 seconds by default, or `JOUET_ENTIRE_TIMEOUT_MS`), and one command asks about each trail once. `pr status` and `pr next` start `entire` only for a pull request whose findings gate fails, so a repository without Entire never runs it. `JOUET_ENTIRE=false` (`[entire] enabled = false`) turns Entire off, as it does for [`pickup` and `sessions`](/commands/handoff).

The trail is the one the check links to, else the one tracking the pull request's head branch (`entire trail show --branch`), else, for a merged pull request, the newest trail that tracked it.

## In each command

- [`jouet pr status`](/commands/pr-status#entire-gates) adds a line under a pull request whose Entire Gates check fails, then its open findings. The check is not counted as a failing check of its own. Open findings are the `entire_findings` attention code.
- [`jouet pr next`](/commands/pr-next) says `entire_findings`, naming the first finding to fix and where it is, after the checks, review and thread states.
- [`jouet pr threads`](/commands/pr-threads#entire-findings) lists the open findings after the threads, addressed as `entire:ID`, and resolves, dismisses and reopens them through `entire trail finding`.
- [`jouet wait ci`](/commands/wait#entire-gates) does not count a derived Entire Gates failure: it waits for the other checks, and says what the failure comes down to.
- [`jouet ci explain`](/commands/ci#entire-gates) says what a failing Entire Gates check comes down to, next to the jobs it explains.

## An example

jdx/mise#13958 had three failing CI checks and two open findings, so its Entire Gates check failed on both Checks and Findings:

```console
$ jouet pr status jdx/mise#13958
#13958 draft feat(install): turn the identity...  CONFLICT  ci 29/55 fail: trusted / windows-e2e, trusted / test-ci, test-ci, Entire Gates  threads 0  review -
  entire: checks: CI checks have not passed; findings: Unresolved findings: 2 medium severity (trail #424)
    [entire:01M4C997EA] e2e/tasks/test_task_monorepo_deps_tool_install:44  medium  Identical `rm -rf "${MISE_DATA_DIR}/installs/tiny/1"* 2>/dev/null || true` pattern as...
    [entire:01M4C997DZ] e2e/cli/test_deps_tool_install:21  medium  `rm -rf "${MISE_DATA_DIR}/installs/tiny/1"* 2>/dev/null || true` only removes the compat symlink...
1 PR, 1 needs attention: #13958 (conflict, checks_failing, entire_findings, draft)
```

Once a finding is fixed and pushed:

```console
$ jouet pr threads resolve entire:01M4C997EA --reply 'Fixed in {sha}: the test uninstalls tiny first'
[entire:01M4C997EA] resolved on Entire trail #424
Entire findings are separate from GitHub threads: no thread was changed by these.
```
