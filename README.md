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
coding agents. It keeps track of what an agent's work costs and what keeps
going wrong, and hands that to later sessions.

An agent doesn't know a build takes minutes until it has waited for one, so it
builds after one edit and again after the next. Told up front, it can finish
its edits and build once. A command that fails in every session can get a rule
that names what to run instead, and a procedure the agent works through by
hand each time can become a [routine](https://jouet.jdx.dev/routines) it calls
once: a short Luau program, something like a skill that runs.

> [!CAUTION]
> **jouet is in early development.** How much it helps depends on your
> language, test runner and agent, and your setup may need some configuration,
> or a fix in jouet, before you see any savings. Commands, flags, exit codes
> and JSON output may change incompatibly between releases until 1.0.

## Install

```sh
mise use -g jouet
jouet setup codex   # or: claude, cursor, gemini, copilot
```

The short name needs mise 2026.10.25 or later; with an older mise, run
`mise use -g github:jdx/jouet`. Or use Cargo:

```sh
cargo install jouet --locked
```

Release archives for Linux, macOS (Apple Silicon) and Windows are on the
[releases page](https://github.com/jdx/jouet/releases), each with a signed
[packslip](https://packslip.dev). The glibc Linux archives need glibc 2.39 or newer; on older
distributions use the static musl archive, which mise picks automatically.
`jouet completion bash` (or `zsh`, `fish`, `powershell`) prints shell
completions, which mise loads for you when it installs jouet.
[All installation options →](https://jouet.jdx.dev/getting-started)

## What it keeps track of

`jouet setup` installs hooks and a short primer the agent reads at session
start. The hooks time each build, test and lint, and
[`jouet cost`](https://jouet.jdx.dev/commands/cost) lists the timings. The
primer names the slowest checks, so the agent can batch its edits before a
long run, or pick a narrower check while it iterates.

[`jouet audit`](https://jouet.jdx.dev/commands/audit) reads your Claude Code
and Codex transcripts and shows where the tokens and time went.
[`jouet wind`](https://jouet.jdx.dev/winding) looks across sessions for a
command that keeps failing, long sleeps, or the same commands run over and
over, and drafts a rule or a mise task for each.
[Session review](https://jouet.jdx.dev/winding#reviewing-whole-sessions), once
you turn it on, reads whole sessions for the user's corrections and abandoned
approaches, and proposes rules, tasks, routines and notes. You or the agent
decide what to adopt. A drafted rule only warns until it is promoted
(automatic promotion is off unless you turn it on), and `jouet unwind`
proposes retiring rules that aren't helping.

[`jouet stats`](https://jouet.jdx.dev/commands/stats) estimates the tokens
jouet saved, against your own history. Those are estimates, not benchmark
results; the [comparative benchmarks](https://jouet.jdx.dev/benchmarks) are
still being built.

## Commands for the chores

The rest of jouet is commands for the work itself: a
[code graph](https://jouet.jdx.dev/commands/graph), searches and reads that
stay within a budget and skip what the session has already seen, edits that
check before they write, and waits that do the polling in one call. When an
agent hand-rolls one of these anyway, a hook names the command.

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

| Command | Replaces |
|---|---|
| [`jouet cost`](https://jouet.jdx.dev/commands/cost) | guessing how long a build, test or lint will take before running it again |
| [`jouet wind`](https://jouet.jdx.dev/winding) | hitting the same failure, or the same correction, session after session |
| [`jouet edit`](https://jouet.jdx.dev/commands/edit) | heredoc edits with a hand-written `assert s.count(old) == 1` |
| [`jouet search`](https://jouet.jdx.dev/commands/search) | `rg` printing every match; grouped by function, budgeted, and skipping what the session already saw |
| [`jouet read --map`](https://jouet.jdx.dev/commands/read) | reading whole files to learn a codebase; Aider's repo map, on the code graph |
| [`jouet graph`](https://jouet.jdx.dev/commands/graph) | repeated searches to find callers, dependencies and the tests a change reaches |
| [`jouet docs`](https://jouet.jdx.dev/commands/docs) | recalling a dependency's API from memory or reading it out of the cargo registry and `node_modules`; its signatures and docs at the locked version |
| [`jouet deps`](https://jouet.jdx.dev/commands/deps) | adding a dependency from memory: whether it exists, its latest version, licence and deprecation, and the OSV advisories on that version or on everything locked |
| [`jouet db`](https://jouet.jdx.dev/commands/db) | `sqlite3` and `psql -c` printing whole tables; a SQLite or Postgres schema in a few lines, and read-only queries cut to 50 rows, local databases only |
| [`jouet http`](https://jouet.jdx.dev/commands/http) | `curl` piped into `jq` or a Python one-liner; the status and only the JSONPaths asked for, within a budget, with hurl-style `--expect` checks and credentials redacted |
| [`jouet wait`](https://jouet.jdx.dev/commands/wait) | `sleep` and `until` loops around `gh pr checks`, log files, ports, URLs and processes |
| [`jouet pr status`](https://jouet.jdx.dev/commands/pr-status) | `for` loops over `gh pr view` |
| [`jouet pr threads`](https://jouet.jdx.dev/commands/pr-threads) | hand-written `reviewThreads` GraphQL, replies and resolves |
| [`jouet audit`](https://jouet.jdx.dev/commands/audit) | reading your own transcripts to see where tokens and time go |
| [`jouet stats`](https://jouet.jdx.dev/commands/stats) | guessing whether jouet's commands and rules are paying off |
| [`jouet test --affected`](https://jouet.jdx.dev/commands/test) | re-running the whole test suite after every edit while iterating |
| [`jouet commit`](https://jouet.jdx.dev/commands/commit) | `git add -A && git commit` in a checkout other sessions share |
| [`jouet secrets scan`](https://jouet.jdx.dev/commands/secrets) | committing or pushing an API key, token or private key; gitleaks' rules over files, the staged lines or outgoing commits |
| [`jouet review-package`](https://jouet.jdx.dev/commands/review-package) | pointing a reviewer at a branch and the commands to run |
| [`jouet handoff` / `pickup`](https://jouet.jdx.dev/commands/handoff) | hand-written notes on where the branch, PR and processes stand |

Commands follow shared
[conventions](https://jouet.jdx.dev/conventions): no prompts and no TTY,
output that adapts to whoever is reading, `--json` that prints exactly one
document, and one table of exit codes.

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
