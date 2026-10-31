---
description: Draft mise tasks for the commands agents keep repeating in a repository, and add one through mise, in the file the user chooses.
---

# jouet task

Agents retype the same command sequences: `cargo build && cargo test -p app`, a lint followed by its fix. Many of these belong in the project's task runner, where a person can run them too. `jouet task propose` finds them in your session transcripts and drafts a [mise task](https://mise.jdx.dev/tasks/) for each. `jouet task apply` adds one through `mise tasks add`, into `mise.local.toml` or `mise.toml`, whichever the user picks.

```text
jouet task propose [--since TIME] [--harness claude|codex|all] [--json]
jouet task apply ID --file local|shared [--name NAME] [--dry-run] [--json]
```

The flags are listed in the CLI reference: [`task propose`](/cli/task/propose) and [`task apply`](/cli/task/apply).

::: warning Needs a recent mise
`jouet task apply` needs mise 2026.10.25 or later, the first release with `mise tasks set` and `mise tasks rm`, file selection for `mise tasks add`, and `--json` on all three. With an older mise it refuses with exit 4 and says which version it needs. `jouet task propose` only reads `mise tasks ls --json`, which older versions have.
:::

## Proposing

`jouet task propose` runs in a repository. It reads the Claude Code and Codex transcripts of sessions that ran there (the 8 most recent from the last 14 days, or since `--since`) and looks at every shell command they ran. Before comparing commands, it normalizes them:

- a trailing `| tail -20` or `| head`, and the `2>&1` before it, is dropped, because it only trims what the agent reads;
- a leading `cd` to the repository's root is dropped, and paths under the root are made relative.

A command is worth a task when it is a sequence (`&&`, `||` or `;`) or at least 80 characters, does the project's work, and is not just `git`, `gh`, `ls`, `grep` and the like. It is drafted when agents ran it at least 3 times in at least 2 sessions. A command that holds something that looks like a secret is never drafted. Neither is one an existing task already runs, according to `mise tasks ls --json`.

Each proposal has:

- **The task:** a name taken from what the command does (`build-test`, `fmt-clippy`, `release` for `./scripts/release.sh`), its `run`, and a description that says jouet added it and why.
- **`depends`**, when the sequence starts with an existing task: `mise run build && ./target/debug/app check` becomes a task that depends on `build` and runs the rest. Only the first step becomes a dependency, because mise runs several dependencies at once and the sequence ran them in order.
- **`sources`**, the files in the repository the command names (`scripts/*.sh`), so mise can skip the task while they are unchanged.
- **The evidence:** runs, sessions, failures, the time the runs took, the output they put into the agent's context, and the runs of the same shape in the [hook ledger](/setup#the-ledger), which also counts sessions whose transcripts were not read.
- **The suggested file, and why.** See [Which file](#which-file).
- **The question** for the agent to ask the user, and the `apply` command for the suggested answer.

```console
$ jouet task propose
read 2 sessions in /work/app since 14d
t1  build-test  ran 3 times in 2 sessions, 6s, about 36 tokens of output
    run      cargo build && cargo test -p app
    suggest  mise.toml: it runs only tools the project already uses (cargo); it uses only paths inside the repository
    ask      Add the mise task `build-test` (`cargo build && cargo test -p app`) to mise.local.toml, just for you and not committed, or to mise.toml, shared with everyone who works on this repository? I suggest mise.toml: it runs only tools the project already uses (cargo); it uses only paths inside the repository.
    apply    jouet task apply t1 --file shared  (or --file local)
```

Proposals are kept with [winding](/winding)'s notes, in `$XDG_STATE_HOME/jouet/wind.json`. Running `propose` again refreshes a pending proposal's evidence and keeps its id and name. It records at most three new proposals per run. `jouet wind show ID` prints one, and `jouet wind dismiss ID` drops it for good.

### Which file

`mise.local.toml` is the default. It is personal and kept out of git. jouet suggests it when:

- the command uses a path that only exists on this machine: an absolute path outside the repository, `~`, or `$HOME`;
- the repository has no mise config in git, so this task would be the first one shared;
- nothing in the repository's CI, docs or build files runs any part of the command, and it runs a tool the project doesn't name. In that case only your sessions run it.

jouet suggests `mise.toml` when none of the above apply and either the repository's CI workflows, README, CONTRIBUTING, docs, Makefile, justfile, `hk.pkl` or `package.json` run part of the command, or every program it runs is one the project already uses: a tool in its mise config, the runner for one of its manifests (`cargo` for `Cargo.toml`, `npm` for `package.json`, `go` for `go.mod`), or a script in the repository.

The suggestion is advice. The user decides.

## Applying

In an agent session, the user chooses the file. The agent asks them the proposal's question, giving jouet's suggestion, and then passes their answer:

```console
$ jouet task apply t1
jouet: choose the file for the task `build-test`: ask the user whether it goes in mise.local.toml (just theirs, not committed) or mise.toml (shared with everyone who works on the repository, committed). Suggest mise.toml: it runs only tools the project already uses (cargo); it uses only paths inside the repository
hint: then `jouet task apply t1 --file shared`, or `--file local`
$ jouet task apply t1 --file shared
task build-test added in /work/app/mise.toml; run it with `mise run build-test`
```

Without `--file`, `apply` always refuses with exit 4, whoever runs it, and never picks a file on its own.

`apply` never edits TOML itself. It runs mise:

- `--file local` runs `mise tasks add --json --env local`, which writes `mise.local.toml` (or `.mise.local.toml`, if that exists).
- `--file shared` runs `mise tasks add --json --path` with the repository's shared config: the first of `mise.toml`, `.mise.toml`, `mise/config.toml` and the other config paths mise reads that git tracks. If there is none, it uses `mise.toml`.
- For a task jouet added before, it runs `mise tasks set` instead, which changes the task in place. A task of the same name that jouet did not add is left alone: `apply` refuses with exit 4, and `--name NAME` picks another name.

The command, the mise version, and the JSON mise printed (the file, `added`, `updated` or `unchanged`, the task's table, and whether mise trusted the file's directory) are all in `--json`. `--dry-run` passes mise's `--dry-run` and records nothing. When the task goes in `mise.local.toml` and git does not ignore that file, `apply` warns, because a local task that gets committed is no longer only yours.

mise quotes each word it is given for a task's command, so it cannot write a sequence such as `cargo build && cargo test` from the words after `--`. A command with shell syntax (operators, pipes, quotes, redirections, a leading `VAR=value`) is therefore written as `sh -c '...'`. A command of plain words is written as it is.

## Winding

The [hook ledger](/winding#the-loop) notices the same sequences without reading any transcript. It keeps only command shapes, so it can see that `cargo build && cargo test -p <arg>` succeeded 3 times in 2 sessions in a work tree, but not what `<arg>` was. When that happens, the next session start in that repository shows one line:

```text
jouet wind noticed `cargo build && cargo test -p <arg>` run 3 times in 2 sessions in this repository; a mise task may replace it. At a natural pause, `jouet task propose` drafts one from the session transcripts (`jouet wind dismiss t1` drops it).
```

`jouet task propose` then drafts the task under the same id. If no transcript ran one exact command that often, the command varies from run to run, and the proposal lapses. A drafted proposal that is still pending is named at later session starts in the same repository:

```text
jouet task t1 is ready: a mise task `build-test` for `cargo build && cargo test -p app`, which agents ran 3 times in 2 sessions here (`jouet task propose` shows it in full). At a natural pause, ask the user whether it goes in mise.local.toml (just theirs) or mise.toml (shared), suggesting mise.toml because it runs only tools the project already uses (cargo); then `jouet task apply t1 --file local|shared`, or `jouet wind dismiss t1`.
```

These lines count against the same budget as the rest of winding: two lines per session start, two new notes a day, and three showings before a line lapses.

## Unwinding

jouet remembers the tasks it added. Its post-tool-use hook counts each `mise run NAME` an agent session runs. `jouet unwind` proposes removing a task jouet added at least 21 days ago that no agent session has run since, as long as the task still runs what jouet wrote. A task someone changed is theirs, and is left alone. Runs by people outside agent sessions are not counted, so ask the user before removing one. `jouet unwind --apply` removes the proposed tasks with `mise tasks rm --yes --path FILE NAME`.
