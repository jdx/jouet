<p align="center">
  <img src="docs/public/logo.svg" alt="The jouet logo, a coral wind-up tin robot with a brass key on top" width="160">
</p>

<h1 align="center">jouet</h1>

<p align="center">
  <strong>Spend fewer tokens. Get there faster.</strong><br>
  Tools that do the repetitive work and help your coding agent improve over time.
</p>

<p align="center">
  <a href="https://jouet.jdx.dev/getting-started">Get started</a> ·
  <a href="https://jouet.jdx.dev">Documentation</a> ·
  <a href="https://jouet.jdx.dev/why">Why jouet</a> ·
  <a href="https://github.com/jdx/jouet/releases">Releases</a>
</p>

jouet (pronounced "zhweh", French for "toy") is a command-line toolkit that
helps coding agents use fewer tokens and finish work faster. It gives them
a code graph to navigate, checked edits instead of improvised scripts, and
commands that handle a whole wait or workflow in one call. Hooks steer your
agent toward those tools, and jouet learns from repeated mistakes to propose
improvements for future sessions.

- **Less context to read.** Searches and reads remember what the session has
  already seen. A code graph answers who calls a function and builds a
  repository map within a token budget.
- **Fewer turns and retries.** One call waits for CI and returns the failure
  log. A simple edit takes one command. A test run reports the failures an
  agent needs, with the full output saved to a log.
- **Reusable work, as code.** Like skills with executable workflows:
  [routines](https://jouet.jdx.dev/routines) let an agent run a tested Luau
  program over jouet's primitives instead of reasoning through every step
  again.
- **Improvement across sessions.** jouet notices repeated failures and
  costly habits, then proposes rules, tasks and workflow improvements for
  you or your agent to adopt. [How self-winding works →](https://jouet.jdx.dev/winding)

> [!CAUTION]
> **jouet is early, and benefits will vary.** Your language, framework, test
> runner or agent harness may need configuration or changes to jouet itself
> before it works well for your project. You may not see savings right away.
> Broad ecosystem support is still being worked out before 1.0; commands,
> flags, exit codes and JSON output may change incompatibly between releases.

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

## A simple edit should be a simple command

An agent should not need to write a Python program to change one string:

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

The text must match exactly once. Across multiple files and replacements,
jouet checks every edit before writing any file. A stale match writes
nothing and shows the closest match, so the agent has enough information
to correct it. [See a multi-file failure →](https://jouet.jdx.dev/why#editing-two-files)

The same idea runs through the toolkit: let code handle the mechanics,
and give the model the result it needs to make the next decision.

## How it improves

`jouet setup` installs a short session primer and hooks. When an agent reaches
for a Python edit script, a sleep-poll loop or raw review-thread GraphQL, a
rule points it to the jouet command instead. The agent can discover commands
through `jouet agent-help` as it needs them.

The hooks also keep a local ledger of command shapes and outcomes.
[`jouet wind`](https://jouet.jdx.dev/winding) uses it to spot recurring
failures, long sleeps and repeated command sequences. It drafts rules in
warn mode and flags sequences worth turning into tasks. Session review can
propose routines and project guidance too. You or your agent decide what
to adopt; automatic rule promotion is opt-in.

[`jouet audit`](https://jouet.jdx.dev/commands/audit) shows where your
Claude Code and Codex sessions spend tokens and time.
[`jouet stats`](https://jouet.jdx.dev/commands/stats) records jouet usage
and estimates token savings against your own history. These estimates are
not benchmark results; [comparative benchmarks](https://jouet.jdx.dev/benchmarks)
are still being developed.

## Commands

| Command | Replaces |
|---|---|
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

See [agent setup and hooks](https://jouet.jdx.dev/setup),
[rules](https://jouet.jdx.dev/rules) and
[routines](https://jouet.jdx.dev/routines) to adapt jouet to your workflow.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup. Report
suspected vulnerabilities through the private process in
[SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
