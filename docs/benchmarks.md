---
description: How jouet is measured: the same agent tasks run with and without its hooks, compared on tokens, requests, time and success.
---

# Benchmarks

jouet's savings figures so far are estimates. [`jouet audit`](/commands/audit) attributes past requests to chores, and [`jouet stats`](/commands/stats) prices jouet's runs by your own history. Neither shows what happens when the same agent does the same work with jouet and without it. The benchmark harness in `bench/` does that: it runs coding agents headlessly on a fixed set of tasks, once with jouet set up and once without, and compares what each run cost and whether it succeeded.

It is a maintainer tool, not part of the `jouet` binary, and it spends real model tokens. Its results decide which built-in rules ship and calibrate `jouet stats`.

## Method

### Tasks

A task is a small fixture repository, a prompt and a success check, chosen from the chores jouet targets:

| task | chore | the rules it exercises | what success means |
|---|---|---|---|
| `smoke` | fix one function | `checks`, `git-add-all` | the tests pass and the fix is committed. The cheapest task, for checking the harness works |
| `rename-setting` | an edit across seven files | `python-edit`, `git-add-all` | the setting and flag are renamed everywhere but the changelog, the tests pass, and the commit leaves out another session's untracked file |
| `fix-tests` | a test-fix loop | `checks`, `git-add-all` | four bugs behind six failing tests are fixed without touching the tests, including on inputs the tests do not cover |
| `ci-wait` | waiting for CI | `sleep-poll`, `checks` | a PR's checks stay pending for 50 seconds, then fail on a test that only runs in CI; the fix, made from the CI log, passes as CI runs it |
| `review-threads` | review-thread triage | `review-threads`, `checks` | two requested changes are made, all three open threads get replies, the two fixed ones are resolved and the question stays open |

Each task lives in `bench/tasks/<name>/`: `task.toml` (prompt, timeout, rules), `repo/` (committed on `main`), an optional `pr/` (committed on the PR branch), an optional `github.json` (the fake GitHub's fixture), `check.py` (exit 0 means success) and `solve.py` (a reference solution). Commits use a fixed author and date, so a task's SHAs are the same everywhere.

### Arms

- **plain:** the harness as installed, with no jouet on `PATH`.
- **jouet:** `jouet` on `PATH`, and `jouet setup claude` or `jouet setup codex` run in the run's home, which installs the hooks, the skill and the session-start primer that points the agent at `jouet agent-help`.

The prompt is the same in both arms. Adding an arm for rtk, or for jouet with only some rules, means adding an entry to `ARMS` in `bench/jbench/arms.py` with its setup step.

### Isolation

Every run starts from nothing, in a temporary directory outside any checkout (so no parent `CLAUDE.md` or `AGENTS.md` leaks in):

- a fresh copy of the task's repository;
- its own `HOME`, `CLAUDE_CONFIG_DIR` or `CODEX_HOME`, XDG directories and `TMPDIR`;
- a short `PATH` whose first entry is a `bin/` directory holding the harness, `git`, `python3`, `node`, `gh` and a few other tools, plus `jouet` in the jouet arm.

The user's own `~/.claude`, `~/.codex` and jouet ledger are never read or written. The one exception is authentication. An API key in the environment (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`; `OPENAI_API_KEY` or `CODEX_API_KEY`) is passed through. Without one, the harness's credentials file (`.credentials.json` or `auth.json`) is linked into the run's config directory, so a token refreshed during a run is written back where the harness will look next time. An agent in a run can read that file, as it could anyway.

### Running the agents

- **Claude Code:** `claude -p --output-format stream-json --verbose --model M --effort E --permission-mode bypassPermissions --strict-mcp-config --max-budget-usd N`. The stream carries each response's usage, and its last event has the totals, turn count and cost.
- **Codex:** `codex exec --json --model M -c model_reasoning_effort=E --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust`. Codex skips hooks nobody has trusted, and the throwaway `CODEX_HOME` has trusted none. Its usage arrives only at the end of the turn, so the per-run token cap is enforced from the session file Codex writes as it goes.

### A fake GitHub

The PR tasks talk to a local fake GitHub (`bench/jbench/fake_github.py`). Both arms reach the same fake through wrapper scripts, so the agent's environment carries no hint of it:

- `gh` gets `GH_HOST=github.localhost`, the one host `gh` talks to over plain HTTP, with the fake as its HTTP proxy, and `GH_REPO`;
- `jouet` gets `JOUET_GITHUB_API_URL`.

The fake executes real GraphQL queries (variables, aliases, fragments, connections, filters, the review-thread mutations) against the fixture, serves its REST responses, and switches any object to its final state a set number of seconds after the run starts, which is how checks stay pending and then fail. Every request is logged. A request the fake cannot answer gets a 404, as GitHub would answer it, and the report lists it, so a gap in a fixture is not mistaken for an agent's mistake.

`mise run test:bench` proves each task without running an agent:

- `check.py` fails on the untouched fixture and passes after `solve.py`;
- each task's probes run against the fake and get the exit codes they expect, with no request it cannot answer. Probes are the commands an agent would use: `jouet wait ci`, `jouet pr threads` (listing, replying and resolving), `gh pr checks`, `gh run view --log-failed`, `gh pr view` and `gh api graphql`.

### What can't be held constant

- **The model is not deterministic.** Neither CLI exposes a temperature or a seed. So each task runs several times per arm, and the arms are compared in pairs: round *k* of the plain arm against round *k* of the jouet arm. Within a round, the arm order is shuffled by the seed, so a slow hour or a provider incident falls on both arms rather than on one.
- **Model and harness versions change.** The summary records the CLI versions and models, and a comparison is only meaningful within one run of the suite.
- **GitHub state** is replaced by the fake, so PR tasks are the same every time. The fake's clock starts with the run, so how long the agent waits is part of what is measured.
- **The prompt cache** depends on what ran recently on the same account. Runs interleave arms, so both see the same cache conditions on average, and the report shows cached and uncached input separately.
- **The network** is not sandboxed. A prompt says not to push, and the fixture's remote points at a repository that does not exist.

### Metrics

Every metric comes from one run's isolated files:

- **Tokens:** input (all the context read, cached or not, which is how `jouet audit` counts), input not served from the cache, output, and Claude Code's reported cost. These come from the harness's own output.
- **Model requests, tool calls, failed calls and tool output bytes:** from `jouet audit --json` over the run's transcripts, the same readers `audit` and `stats` use. Requests are counted the same way for both harnesses.
- **Wall time:** from starting the agent to its exit.
- **Success:** the task's check, and the run finished within its timeout and budget.
- **Rule decisions:** from the run's jouet ledger, per rule and action.

Token and time metrics include failed runs; success rates are reported beside them. For each task, harness and arm, the report gives the median and quartiles of each metric. For each pair of arms, it gives the median difference (jouet minus plain), its range, how many rounds jouet used less, and an exact two-sided sign test. With five pairs, the smallest p-value that test can give is 0.0625, which is the honest answer for five pairs.

## Running it

It needs Python 3.11 or newer, a built `jouet`, and the `claude` or `codex` CLI, logged in or with an API key in the environment.

```sh
mise run bench:agents --dry-run              # the plan and the budget; spends nothing
mise run bench:agents:smoke                  # the smoke task, once per harness and arm
mise run bench:agents --harness claude --rounds 5
mise run bench:agents --tasks ci-wait,review-threads --harness codex
python3 bench/agents.py report bench/results/<id>   # rewrite a summary
```

| flag | default | what it does |
|---|---|---|
| `--harness` | `claude,codex` | which harnesses |
| `--tasks` | every task | which tasks |
| `--arms` | `plain,jouet` | which arms |
| `--rounds` | 3 | runs per task, harness and arm |
| `--seed` | the time | the arm order in each round |
| `--claude-model`, `--codex-model` | `claude-sonnet-5-5`, `gpt-6-sol` | a fixed model per harness |
| `--effort` | `medium` | reasoning effort, both harnesses |
| `--max-runs` | 40 | refuse a plan with more runs |
| `--max-tokens-per-run` | 4,000,000 | stop a run once its input and output tokens pass this |
| `--max-usd-per-run` | 3 | Claude Code's `--max-budget-usd` (Codex reports no cost) |
| `--max-total-tokens` | 60,000,000 | skip the remaining runs once the suite has spent this |
| `--timeout` | each task's | cap every task's timeout, in seconds |
| `--jouet` | `target/debug/jouet` | the jouet binary the jouet arm runs |
| `--keep-work` | off | keep each run's work tree and home, for debugging |

A run stopped by its timeout or a budget counts as a failure, and the report says which stopped it.

Results go to `bench/results/<timestamp>/`, which git ignores:

- `meta.json`: versions, models, seed, budget and commit;
- `runs.jsonl`: one record per run;
- `summary.json` and `summary.md`: the comparison;
- `runs/<task>.<harness>.<arm>.r<round>/`: the agent's event stream, its transcripts, `jouet audit`'s report, the check's output and, for PR tasks, every request the fake GitHub saw and its final state.

### In CI

The `bench agents` workflow never runs on its own. Someone dispatches it, and by default it is a dry run of the smoke task. Untick `dry_run` to run agents with the `BENCH_ANTHROPIC_API_KEY` and `BENCH_OPENAI_API_KEY` secrets. The summary goes to the job summary and the results directory to an artifact. `mise run test`, which CI runs, includes `test:bench` and never starts an agent.

The workflow installs the harnesses from `mise.bench.toml`, at the versions `mise.bench.lock` pins, so a result names exactly what it measured. That file is loaded only with `MISE_ENV=bench`, so it never changes the `claude` or `codex` you run day to day in the repository. Bump them with `MISE_ENV=bench mise lock`, and run them locally with `MISE_ENV=bench mise run bench:agents`.

## Why it is built this way

- **A mise task and a Python script, not `jouet bench`.** The harness is for maintainers. Shipping it in the binary would give every user a command that spends their tokens to measure jouet, plus fixtures and a fake server they do not need. mr-boxington's real-world benchmarks set the pattern: a standard-library-only Python script behind a mise task, writing JSON and Markdown.
- **Not tak.** tak times a command many times and gates on instruction counts. An agent run takes minutes, needs a per-run timeout and a fresh home, and its interesting numbers are tokens and requests, which tak does not measure. The harness times each run itself and keeps tak's habits: interleaved, seeded rounds, and medians with their spread.
- **`jouet audit`'s readers, not new ones.** The tool-call counts are read the way `audit` and `stats` read real sessions, so a gap between this benchmark and `stats`' estimates is a calibration finding, not two parsers disagreeing.

## Limits

- Five small tasks in Python repositories are not a survey of real work. They cover the chores jouet targets, so a result says how much jouet helps on those chores, not on a whole session.
- A few runs per arm only detect large effects. Read the spread and the pair counts before the medians.
- The fake GitHub answers what its fixtures hold. An agent that asks for something else gets a 404, which the report lists, and a different answer than GitHub would give.
- The primer and skill that `jouet setup` installs add context to every request. The benchmark measures that cost too, which is part of the point.
