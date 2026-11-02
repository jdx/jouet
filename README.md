<p align="center">
  <img src="docs/public/logo.svg" alt="The jouet logo, a coral wind-up tin robot with a brass key on top" width="160">
</p>

<h1 align="center">jouet</h1>

<p align="center">
  <strong>Wind-up tools for coding agents.</strong><br>
  It tells an agent which checks are slow, and turns what went wrong in past sessions into rules, tasks and routines for the next one.
</p>

<p align="center">
  <a href="https://jouet.jdx.dev/getting-started">Get started</a> ·
  <a href="https://jouet.jdx.dev">Documentation</a> ·
  <a href="https://jouet.jdx.dev/why">Why jouet</a> ·
  <a href="https://github.com/jdx/jouet/releases">Releases</a>
</p>

jouet (pronounced "zhweh", French for "toy") is a command-line toolkit for
coding agents such as Claude Code and Codex. It does two things:

- **It remembers what sessions cost.** Hooks time every build, test and lint,
  and the agent hears at session start which ones are slow, so it can finish
  its edits and build once instead of after every change. Across sessions,
  jouet notices commands that keep failing and corrections that keep coming
  back, and drafts a rule, a mise task or a routine for each.
- **It has a command for each chore agents script by hand.** Exact edits
  across files, budgeted searches and reads, waits for CI that take one call,
  only the tests a change affects. When an agent hand-rolls one anyway, a hook
  names the command.

> [!CAUTION]
> **jouet is in early development.** How much it helps depends on your
> language, test runner and agent, and your setup may need some configuration,
> or a fix in jouet, before you see any savings. Commands, flags, exit codes
> and JSON output may change incompatibly between releases until 1.0.

## Quick start

1. Install jouet with [mise](https://mise.jdx.dev) (2026.10.25 or later):

   ```sh
   mise use -g jouet
   ```

   With an older mise, use `mise use -g github:jdx/jouet`. Or run
   `cargo install jouet --locked`, or unpack a
   [release archive](https://github.com/jdx/jouet/releases) for Linux, macOS
   (Apple Silicon) or Windows. [All installation options →](https://jouet.jdx.dev/getting-started#install)

2. Connect your agent. This installs jouet's hooks, which time the checks and
   brief each session, and the jouet-init skill:

   ```sh
   jouet setup claude   # or: codex, cursor, gemini, copilot
   ```

3. Start your agent in a project and ask it to use the **jouet-init** skill:
   `/jouet-init` in Claude Code, `$jouet-init` in Codex, or "Use the
   jouet-init skill to configure jouet for this project." The agent reads the
   project's checks and timing history, adjusts jouet's configuration where
   the project needs it, and checks what it changed.
   [Initializing a project →](https://jouet.jdx.dev/project-setup)

## Learning from past sessions

The hooks time each build, test and lint, and
[`jouet cost`](https://jouet.jdx.dev/commands/cost) lists the timings. The
primer names the slowest checks, so the agent can batch its edits before a
long run, or pick a narrower check while it iterates.

[`jouet wind`](https://jouet.jdx.dev/winding) looks across sessions for a
command that keeps failing, long sleeps, or the same commands run over and
over, and drafts a rule or a mise task for each.
[Session review](https://jouet.jdx.dev/winding#reviewing-whole-sessions), once
you turn it on, reads whole sessions for the user's corrections and abandoned
approaches, and proposes rules, tasks, routines and notes. You or the agent
decide what to adopt. A drafted rule only warns until it is promoted
(automatic promotion is off unless you turn it on), and `jouet unwind`
proposes retiring rules that aren't helping.

A [routine](https://jouet.jdx.dev/routines) is a procedure the agent calls
once instead of working through each step: a short, tested Luau program,
something like a skill that runs. `joue land` waits for CI and merges a PR
once it is ready.

[`jouet audit`](https://jouet.jdx.dev/commands/audit) reads your Claude Code
and Codex transcripts and shows where the tokens and time went, and
[`jouet stats`](https://jouet.jdx.dev/commands/stats) estimates the tokens
jouet saved, against your own history. Those are estimates, not benchmark
results; the [comparative benchmarks](https://jouet.jdx.dev/benchmarks) are
still being built.

## An example

An agent shouldn't need a Python program to change one string:

```sh
python3 - <<'EOF'
from pathlib import Path
p = Path("src/main.rs")
s = p.read_text()
assert s.count("let x = 1;") == 1
p.write_text(s.replace("let x = 1;", "let x = 2;"))
EOF
```

With jouet:

```sh
jouet edit replace src/main.rs --old 'let x = 1;' --new 'let x = 2;'
```

The old text has to match exactly once. With several files or replacements,
jouet checks all of them before it writes anything, so a stale match leaves
every file alone and shows the closest match instead.
[See a multi-file failure →](https://jouet.jdx.dev/why#editing-two-files)

## Commands

**Code and data**

| Command | Instead of |
|---|---|
| [`jouet edit`](https://jouet.jdx.dev/commands/edit) | heredoc edits with a hand-written `assert s.count(old) == 1` |
| [`jouet search`](https://jouet.jdx.dev/commands/search) | `rg` printing every match; grouped by function, budgeted, and skipping what the session already saw |
| [`jouet read`](https://jouet.jdx.dev/commands/read) | reading a whole file to find one definition; `--map` gives a repository map, on the code graph |
| [`jouet graph`](https://jouet.jdx.dev/commands/graph) | repeated searches for callers, dependencies and the tests a change reaches |
| [`jouet diag`](https://jouet.jdx.dev/commands/diag) | a full lint run to check the files you just changed |
| [`jouet docs`](https://jouet.jdx.dev/commands/docs) | recalling a dependency's API from memory; its signatures and docs at the locked version |
| [`jouet deps`](https://jouet.jdx.dev/commands/deps) | adding a dependency from memory; whether it exists, its latest version, licence and OSV advisories |
| [`jouet db`](https://jouet.jdx.dev/commands/db) | `sqlite3` and `psql -c` printing whole tables; schemas and read-only queries, cut to a limit |
| [`jouet http`](https://jouet.jdx.dev/commands/http) | `curl` piped into `jq`; the status and only the JSONPaths asked for, with credentials redacted |

**Running and waiting**

| Command | Instead of |
|---|---|
| [`jouet test`](https://jouet.jdx.dev/commands/test) | re-running the whole suite after every edit; `--affected` runs only the tests a change reaches |
| [`jouet wait`](https://jouet.jdx.dev/commands/wait) | `sleep` and `until` loops around `gh pr checks`, log files, ports, URLs and processes |
| [`jouet job`](https://jouet.jdx.dev/commands/job) | `cmd &`, `nohup`, sleep-and-curl loops and `pkill` |
| [`jouet ci run`](https://jouet.jdx.dev/commands/ci) / [`explain`](https://jouet.jdx.dev/commands/ci#jouet-ci-explain) | pushing to see whether CI passes, and rerunning a failure without knowing whether it is flaky |
| [`jouet task`](https://jouet.jdx.dev/commands/task) | retyping the same command sequence every session; drafts a mise task for it |

**Pull requests**

| Command | Instead of |
|---|---|
| [`jouet pr status`](https://jouet.jdx.dev/commands/pr-status) | `for` loops over `gh pr view` |
| [`jouet pr threads`](https://jouet.jdx.dev/commands/pr-threads) | hand-written `reviewThreads` GraphQL, replies and resolves |
| [`jouet pr next`](https://jouet.jdx.dev/commands/pr-next) / [`land`](https://jouet.jdx.dev/commands/pr-land) | reading a PR's whole status to decide what it needs, then merging by hand |
| [`jouet stack`](https://jouet.jdx.dev/commands/stack) | trusting that each layer of a PR stack still contains the one below |
| [`jouet review-package`](https://jouet.jdx.dev/commands/review-package) | pointing a reviewer at a branch and the commands to run |

**Git and sessions**

| Command | Instead of |
|---|---|
| [`jouet commit`](https://jouet.jdx.dev/commands/commit) | `git add -A && git commit` in a checkout other sessions share |
| [`jouet secrets scan`](https://jouet.jdx.dev/commands/secrets) | committing an API key or token; gitleaks' rules over files, staged lines or outgoing commits |
| [`jouet handoff` / `pickup`](https://jouet.jdx.dev/commands/handoff) | hand-written notes on where the branch, PR and processes stand |
| [`jouet sessions`](https://jouet.jdx.dev/commands/sessions) / [`claim`](https://jouet.jdx.dev/commands/claim) | two sessions editing the same worktree without knowing it |

**Measuring and learning**

| Command | Instead of |
|---|---|
| [`jouet cost`](https://jouet.jdx.dev/commands/cost) | guessing how long a build, test or lint takes before running it again |
| [`jouet wind`](https://jouet.jdx.dev/winding) | hitting the same failure, or the same correction, session after session |
| [`jouet audit`](https://jouet.jdx.dev/commands/audit) | reading your own transcripts to see where tokens and time go |
| [`jouet stats`](https://jouet.jdx.dev/commands/stats) | guessing whether jouet's commands and rules are paying off |

Every command follows the same
[conventions](https://jouet.jdx.dev/conventions): no prompts and no TTY,
output that adapts to whoever is reading, `--json` that prints exactly one
document, and one table of exit codes. The
[CLI reference](https://jouet.jdx.dev/cli/) lists every flag.

To change what jouet does in your project, see
[agent setup and hooks](https://jouet.jdx.dev/setup),
[rules](https://jouet.jdx.dev/rules) and
[routines](https://jouet.jdx.dev/routines).

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup. Report
suspected vulnerabilities through the private process in
[SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
