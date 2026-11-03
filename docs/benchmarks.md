---
description: How jouet is measured, by running the same agent tasks with and without its hooks and comparing tokens, requests, time and success.
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

### Tasks from real projects

Other tasks are real, already-fixed issues in real projects. Their `task.toml` names a local clone and a pinned commit before the fix (`[source]`) instead of a `repo/`. Prompts describe the original issue and ask for a fix, a test and a commit. The checker runs a hidden regression test and `solve.py` applies a reference fix. They set `opt_in = true`, so they run only when named with `--tasks`, and each needs its clone at the path `[source]` names.

Success means the agent exited normally and the task's checker passed; it does not by itself establish that every part of the prompt was fulfilled. Some real-project checkers report whether a fix was committed without making the commit a condition of success, and not all check that the agent added a test. Inspect the task's checker and run evidence when assessing accuracy.

Projects beyond jdx's own:

| task | project | issue | language and runner | needs on the host |
|---|---|---|---|---|
| `tornado-cancelled-handler` | tornado, a web framework | [tornadoweb/tornado#2990](https://github.com/tornadoweb/tornado/issues/2990) | Python, tornado's own unittest runner | Python 3 |
| `actual-running-balance` | Actual Budget, a local-first finance webapp | [actualbudget/actual#8475](https://github.com/actualbudget/actual/issues/8475) | TypeScript, Playwright and Chromium | the benchmark's pinned Node, a local clone and prepared Yarn and browser caches, as described below |
| `cobra-completion-args` | Cobra, a Go CLI library | [spf13/cobra#2257](https://github.com/spf13/cobra/issues/2257) | Go, `go test` | the benchmark's Go tools, cached modules, and a local clone at `~/src/cobra-bench-source` |
| `rubocop-redundant-self` | RuboCop, a Ruby static analyzer | [rubocop/rubocop#16149](https://github.com/rubocop/rubocop/issues/16149) | Ruby, RSpec and RuboCop's CLI | Ruby 4.0.9, a clone at `~/src/rubocop-bench-source`, and a prepared bundle as described below |
| `requests-proxied-files` | Requests, an HTTP library | [psf/requests#7502](https://github.com/psf/requests/pull/7502) | Python, pytest and pytest-httpbin | Python 3.13 and a local Requests clone at `~/src/requests-bench-source` |
| `yq-signed-hex` | yq, a CLI | [mikefarah/yq#2748](https://github.com/mikefarah/yq/issues/2748) | Go, `go test` | the benchmark's pinned Go toolchain, and yq's modules in `~/go/pkg/mod` (`go mod download`) |
| `zod-int-bounds` | zod, a library | [colinhacks/zod#6550](https://github.com/colinhacks/zod/issues/6550) | TypeScript, vitest in a pnpm monorepo | the benchmark's pinned pnpm and Node/npm, and one `pnpm install` in the clone to fill the store |
| `commander-output-config` | commander.js, a CLI library | [tj/commander.js#2342](https://github.com/tj/commander.js/issues/2342) | JavaScript, jest and tsc | one `npm ci` in the clone to fill npm's cache |
| `click-short-help` | Click, a CLI library | [pallets/click#3865](https://github.com/pallets/click/pull/3865) | Python, pytest | Python 3.13 and the task's hash-locked pytest and build dependencies; Click is installed editable in each run |
| `django-group-by-transform` | Django, a web framework | [ticket #36292](https://code.djangoproject.com/ticket/36292) | Python, Django's `tests/runtests.py` on SQLite | Python 3.13, a Django clone at `~/src/django`, and the task's hash-locked wheels in `~/src/django-bench-wheels`, as described below |
| `zod-lazy-error-pr` | zod, a library | a whole PR session on [colinhacks/zod#6519](https://github.com/colinhacks/zod/pull/6519) | TypeScript, vitest in a pnpm monorepo | the benchmark's pinned pnpm and Node/npm, and one `pnpm install` in `~/src/zod` at the PR's base to fill the store |

Each `task.toml` notes which jouet levels the task can exercise: what `jouet test` does with the project out of the box, and where TOML config or a Luau rule or routine could help. A JavaScript project's dependencies are installed by the task's `setup` commands, offline from your cache, after the work tree is committed and before the agent's clock starts. The agents are not network-sandboxed and can override offline defaults. A setup that leaves files git does not ignore is refused. Tools the default `bin/` lacks, such as `go`, `npm` or a pinned `pnpm`, can be exposed through `[tools]`, by path or by a bare name found on your `PATH`.

The Zod task checks the reported number-schema issue for success. The upstream fix also addressed a pre-existing bigint bounds issue; its hidden test is reported separately in the check log and does not decide whether the requested task passed.

The opt-in Actual Budget task checks a running balance after two edits in a local browser, and requires a committed regression test and release note. The reference fix is adapted from [actualbudget/actual#8490](https://github.com/actualbudget/actual/pull/8490). It retains repository history, uses port 3001 for its local server, and refuses to reuse a server already on that port. Before running, prepare the pinned dependencies and Chromium once from the jouet checkout:

```sh
git clone https://github.com/actualbudget/actual.git ~/src/actual-bench-source
git -C ~/src/actual-bench-source checkout --detach 501bece3a1a556b9538359ca3284d065bac285eb
MISE_ENV=bench mise x -- sh -c 'cd "$HOME/src/actual-bench-source" && YARN_CACHE_FOLDER="$HOME/.cache/jouet-bench/yarn-cache" node .yarn/releases/yarn-4.13.0.cjs install --immutable'
MISE_ENV=bench mise x -- sh -c 'cd "$HOME/src/actual-bench-source" && PLAYWRIGHT_BROWSERS_PATH="$HOME/.cache/jouet-bench/playwright" node node_modules/@playwright/test/cli.js install --only-shell chromium'
```

Each trial installs its own `node_modules` from the prepared cache before timing. The agents are not network-sandboxed.

The opt-in Django task's traceback ends in the SQL compiler, but the cause is in how the query records its `values()` selection, so the agent has to read across the query and the compiler. Its check runs the fix's own aggregation tests and `aggregation_regress`. Each run creates a virtual environment with asgiref, sqlparse and tblib (without tblib, Django's parallel runner cannot report a failure). pip installs them with `PIP_NO_INDEX`, so prepare the wheels once:

```sh
git clone https://github.com/django/django.git ~/src/django
python3 -m pip download --only-binary=:all: --require-hashes -r bench/tasks/django-group-by-transform/requirements.lock -d ~/src/django-bench-wheels
```

The opt-in `zod-lazy-error-pr` task is a whole PR session in a TypeScript project, like `mise-uninstall-pr`. The agent starts at the PR's second push (442ff01). The fake GitHub serves that push's real CI failure and log, and pullfrog's real review thread. To pass, the agent has to fix the failing issue-schema tests, make a check that throws synchronously reject through `~standard.validate`, keep the PR's own lazy-error tests passing, then reply on the thread and resolve it. Fill pnpm's store once with `pnpm install` in a `~/src/zod` worktree at a6b4939.

The opt-in Requests task installs hash-locked test dependencies, the project's pinned Ruff version, and the checkout into an isolated Python environment before each run. Its independent checker prepares multipart requests without network access, exercising temporary files, dynamically proxied streams and ordinary file values.

The opt-in RuboCop task uses the parent of [the fix](https://github.com/rubocop/rubocop/pull/16151) and a checked-in Bundler lockfile. Prepare the clone and a gem bundle once; the harness copies that bundle into each run's ignored `vendor/` directory and checks it against the lockfile before starting the agent. Both arms have separate writable copies. The bundle contains native gems, so prepare it with the same Ruby and platform used for the trials:

```sh
git clone https://github.com/rubocop/rubocop.git ~/src/rubocop-bench-source
git -C ~/src/rubocop-bench-source worktree add --detach /tmp/rubocop-bench-prep 11e303ca043bde4b5b0bef2fbc344823c204f8ff
cp bench/tasks/rubocop-redundant-self/Gemfile.lock /tmp/rubocop-bench-prep/Gemfile.lock
(cd /tmp/rubocop-bench-prep && BUNDLE_PATH="$HOME/src/rubocop-bench-source/vendor/bundle" BUNDLE_APP_CONFIG=/tmp/rubocop-bench-config mise x ruby@4.0.9 -- bundle install)
```

Run the harness with `MISE_ENV=bench` so both arms get its pinned Ruby and Bundler. The independent checker invokes RuboCop on splat assignment targets and on genuinely redundant receivers; it also requires a committed regression spec, changelog entry, and clean tracked worktree. The check does not replace the project's full suite.

RuboCop's pinned configuration specs can fail when the checkout is under a hidden directory, including the harness's default `~/.cache`. The task refuses such a checkout during setup. Set `XDG_CACHE_HOME` to a non-hidden directory outside `/tmp` when running it, for example `XDG_CACHE_HOME="$HOME/src/jouet-bench-cache" MISE_ENV=bench mise run bench:agents --tasks rubocop-redundant-self`. The harness creates the directory if needed. This keeps the same checkout location policy for both arms.

The opt-in `jactionlint-dependabot-groups` task reproduces [jdx/jactionlint#313](https://github.com/jdx/jactionlint/issues/313) from the parent of its fix. It needs a local clone at `~/src/jactionlint`, Go 1.27.1, Go 1.27.3 and the project's modules in `~/go/pkg/mod`. Run it with `MISE_ENV=bench MISE_GO_VERSION=1.27.1 mise run bench:agents --tasks jactionlint-dependabot-groups`: the project uses Go 1.27.1 for formatting and staticcheck, and Go 1.27.3 for builds and tests. Before each timed run, mise installs the project's tools in that run's isolated home. Its independent check exercises the public parser with both valid grouped updates and genuine duplicates.

The opt-in `ruff-perf-iterable` task reproduces [astral-sh/ruff#29365](https://github.com/astral-sh/ruff/issues/29365) from the parent of its upstream fix. It needs a local Ruff Git repository at `~/src/ruff-bench-source` containing commit `83a3afc3156d73ae4296f3fa74ede28e4c72bfcd`, Rust 1.99.0, and the project's Cargo dependencies. Ruff's project instructions also require `uv` for its `prek` checks; verify it is available in both isolated arms, and report when a run cannot perform those checks. The independent check builds Ruff and exercises both conditional-iterable fixes and a named-expression case. It is excluded from the default suite because the project is large and each isolated run must build it.

### Tasks from SWE-bench

`bench/import_swebench.py` turns SWE-bench-format records into tasks of the same kind. It reads a JSONL file (a Hugging Face dataset's `to_json` export), a JSON list or a page of the dataset viewer's `/rows` API, with SWE-bench's fields (`instance_id`, `repo`, `base_commit`, `problem_statement`, `patch`, `test_patch`, `FAIL_TO_PASS`, `PASS_TO_PASS`). SWE-bench Verified, SWE-bench Multilingual and SWE-PolyBench use those names; Multi-SWE-bench's own (`org`, `number`, `base.sha`, `fix_patch`, `resolved_issues`, `f2p_tests`, `p2p_tests`) are mapped onto them. Each instance becomes an opt-in task named `swebench-<repo>-<number>`:

- `[source]` is a clone under `~/src/swebench/<owner>__<repo>` at `base_commit`, cloned once if missing;
- the prompt is `problem_statement` and the line the other real-project tasks end with;
- `hidden/` holds the test files as `test_patch` leaves them, and `fix.patch` is `patch`;
- `check.py` puts `hidden/` in place, runs the instance's test modules with the repository's runner, and passes when every `FAIL_TO_PASS` test passes and no `PASS_TO_PASS` test fails. A `PASS_TO_PASS` test the environment skips, or that the log does not name, is listed in the check's output but does not fail it. The logs are read as SWE-bench reads them: Django's runner, sympy's `bin/test` and pytest's `-rA` summary. `swebench.json` holds the instance's tests and the command;
- `solve.py` applies `fix.patch` and commits it.

```sh
python3 bench/import_swebench.py import verified.jsonl --dataset "SWE-bench Verified" --ids django__django-11400,sympy__sympy-16597
MISE_ENV=bench mise x -- python3 bench/import_swebench.py prime bench/tasks/swebench-*
python3 bench/agents.py check-tasks --tasks swebench-django-11400
```

`prime` is the only step that uses the network. For each task it builds a Python environment under `~/src/swebench/envs/<instance_id>` with the benchmark's `uv`. It uses the instance's Python version, installs the repository at its base commit for its dependencies, adds the test extras, and makes the environment read-only. Packages are limited to those released within six months after the instance's `created_at`. The build tools are limited to those released before 2024, and atomicwrites, whose old releases were deleted from PyPI, is exempt. uv downloads the Python versions it lacks. A run links `python3` and `python` to that environment in both arms and puts the work tree first on `PYTHONPATH`, so the agent's edits are what is tested, with no install in the run.

The repositories the importer knows (`spec_for`) are Django, sympy, Sphinx, pylint and pytest, all pure Python. Python versions follow SWE-bench's harness, except that Django 3.x runs on 3.8, not 3.6, because uv's Python builds start at 3.8. A repository it does not know needs `--test-cmd` (and `--python` for a Python project), and its check then requires only that the command exits 0. That is how SWE-bench Multilingual's or Multi-SWE-bench's other languages import; their toolchains are not primed. Repositories with compiled extensions, such as astropy, scikit-learn and matplotlib, do not work with `PYTHONPATH`, since the work tree has no built extensions.

The first batch has ten SWE-bench Verified instances, chosen from the dataset's two longest difficulty labels (`1-4 hours`, `>4 hours`) for gold patches that span several files or need reading across modules:

| task | instance | gold patch | why |
|---|---|---|---|
| `swebench-django-11400` | Ordering of `RelatedFieldListFilter` choices | 3 files | admin filters, model fields and reverse relations |
| `swebench-django-11885` | Combine fast-delete queries | 2 files, 213 lines | the deletion collector and the admin's use of it |
| `swebench-django-13344` | A coroutine reaches the first middleware's `process_response()` | 3 files | the session, cache and security middlewares |
| `swebench-django-14631` | `BaseForm` should read values through `BoundField` | 2 files | forms and bound fields |
| `swebench-django-15629` | `db_collation` does not reach foreign keys | 4 files | the schema editors and related fields |
| `swebench-django-16263` | Strip unused annotations from count queries | 4 files | expressions, `Q`, the SQL query and `WHERE` |
| `swebench-sympy-16597` | `a.is_even` does not imply `a.is_finite` | 6 files | assumptions, core, printing and tensors |
| `swebench-sympy-14248` | `MatrixSymbol` differences print as sums | 3 files | the str, pretty and LaTeX printers |
| `swebench-sphinx-9461` | `@classmethod` `@property` methods go undocumented | 3 files | the Python domain, autodoc and `util.inspect` |
| `swebench-pytest-5787` | Exception serialization loses chained exceptions | 1 file, 260 lines | report serialization, read against `_pytest._code` |

SWE-bench is MIT-licensed, and Django, sympy and Sphinx are BSD-licensed and pytest is MIT-licensed, so their tasks are committed. pylint is GPL-2.0, so its instances (`pylint-dev__pylint-4551` was a candidate) are left to be generated locally. The dataset itself is not committed. Download it as JSONL, for example from the dataset viewer's `/rows` API for `princeton-nlp/SWE-bench_Verified`.

The models under test may have seen these issues and fixes in training. Both arms use the same model, but jouet may change how an agent uses what it remembers, so pairing does not eliminate this confounder. Success and cost differences on these tasks may not generalize to unfamiliar work.

### Arms

- **plain:** the harness as installed, with no jouet on `PATH`.
- **jouet:** `jouet` on `PATH`, and `jouet setup claude` or `jouet setup codex` run in the run's home, which installs the hooks and the session-start primer that points the agent at `jouet agent-help`.

Codex trials use a dedicated configuration whose hooks a person has reviewed. Create a new home and review the hooks before running a Codex jouet arm:

```sh
python3 bench/agents.py prepare-codex --home "$HOME/.cache/jouet-bench/reviewed-codex"
HOME="$HOME/.cache/jouet-bench/reviewed-codex" CODEX_HOME="$HOME/.cache/jouet-bench/reviewed-codex/.codex" codex -c 'cli_auth_credentials_store="ephemeral"'
# In Codex, open /hooks and review and trust the jouet hooks. Then exit.
python3 bench/agents.py run --harness codex --codex-reviewed-home "$HOME/.cache/jouet-bench/reviewed-codex/.codex"
```

The review command uses an ephemeral credential store; it does not need `codex logout`. The harness uses the dedicated file credential store, lends credentials for each run, and returns any refresh before removing its copy. It refuses concurrent use of the dedicated home and checks that Codex reports the exact hooks for this jouet build as enabled and trusted. A changed hook needs a new review. The harness checks for a recorded session-start event and excludes a run if it did not occur. Other enabled hooks cause the preflight to fail.

Codex runs use ephemeral sessions. The harness records live cumulative token counts and selected agent events, but not every tool call or output. `jouet audit` cannot inspect a session transcript, so audit-derived request and tool metrics are unavailable for these runs.

The prompt is the same in both arms. `--jouet-env KEY=VALUE` sets a variable in the jouet arm's agent environment after `jouet setup`. For example, `--jouet-env JOUET_HOOKS_PRIME=0` measures sessions without the primer, and without the skill that setup would install in its place. Adding an arm for rtk, or for jouet with only some rules, means adding an entry to `ARMS` in `bench/jbench/arms.py` with its setup step.

### The jouet-init arm

`--arms plain,jouet,jouet-init` adds a third arm: jouet as in the `jouet` arm, in the project as the [jouet-init skill](/project-setup) configured it, which is how a user who ran `jouet setup` and then `/jouet-init` works. The skill runs once per task, starting commit, jouet build, harness and model, in an isolated home like any run, with the prompt `Use the jouet-init skill to configure jouet for this project.`. What it changed, committed or not, is kept as a patch with what the run cost under `~/.cache/jouet-bench/init/`, and reused by every later `jouet-init` run. Each run commits the patch to the task's baseline and replays any task commits on top, so the configuration is already present in the comparison against `origin/main`. If the patch cannot be applied to the baseline or the task commits cannot be replayed, the trial fails before the agent starts. The run also executes `jouet trust --yes` in its fresh home, as the user does once after the skill.

The skill's run is not counted: a project commits the configuration once and keeps it, as it keeps its other tooling config. The report lists the files configured. When both `jouet` and `jouet-init` ran, it also compares their task metrics directly, with paired deltas restricted to rounds where both passed. The arm medians include finished failed runs, as with the other diagnostics. When the pilot includes `jouet-init`, the main report compares it with `plain`; otherwise it compares `jouet` with `plain`. The initialized arm includes the skill’s project changes, while the plain arm does not, so that comparison measures the configured jouet experience rather than isolating jouet on an identical project tree.

### Isolation

Every run starts from nothing, in a temporary directory outside any checkout (so no parent `CLAUDE.md` or `AGENTS.md` leaks in):

- a fresh copy of the task's repository;
- its own `HOME`, XDG directories and `TMPDIR`, plus a fresh `CLAUDE_CONFIG_DIR` or, for a plain Codex run, `CODEX_HOME`. Codex jouet runs share the reviewed `CODEX_HOME` and use ephemeral sessions;
- a short `PATH` including a `bin/` directory holding the harness, `git`, `python3`, `node`, `gh` and a few other tools, plus `jouet` in the jouet arm. GNU `parallel`, `uv` and `uvx`, when installed on the host, are available to both arms for projects whose checks use them.

A source task normally starts from a new single commit containing the pinned tree. To let an agent inspect the actual ancestors, set `history = true` in `[source]` and `remote = false` at the top level of `task.toml`. The harness copies the pinned commit and its ancestors into each independent run, without retaining the source's remote, tags or other refs. With `upstream = true`, it creates a local bare origin with pushing disabled. History mode requires a complete, non-shallow source clone and does not initialize submodules.

For a source task without history whose `.gitmodules` uses relative URLs, set `submodule_base_url` in `[source]` to the original repository URL. Setup temporarily uses it to resolve and fetch the pinned submodules, so this step needs network access when the submodules are remote. The temporary origin, submodule origins, resolved URLs and fetch metadata are removed before the trial; the task's own configured origin is then set up normally. For local paths or `file:` URLs, explicitly permit Git's file protocol for the benchmark process, for example `GIT_ALLOW_PROTOCOL=file:https` when both local and HTTPS URLs are needed. History mode does not accept this option.

For task definitions that should stay outside this checkout, `run` and `check-tasks` accept repeatable `--tasks-dir PATH` arguments alongside the bundled task directory. A run using an external task directory must specify `--out` outside the jouet checkout, and its task and cache directories, including the `jouet-init` cache when that arm is selected, must also be outside the checkout. The same applies to directories in `JOUET_BENCH_TASKS`. For example:

```sh
MISE_ENV=bench mise run bench:agents --tasks-dir "$HOME/bench-tasks" --tasks example --out "$HOME/bench-results/example"
```

These worktrees are not security sandboxes: agents and task programs can access host files and the network. Results, agent transcripts, cost notes and any retained worktrees can contain task data; choose locations and task programs accordingly.

A task may set `python_requirements` to a hash-locked pip requirements file in its task directory. Before timing each agent, the harness creates a fresh virtual environment under that run's home, installs the pinned requirements with hash verification, and prepends its `bin` to the isolated `PATH` for both the agent and the checker. The environments are separate; only the pip download cache is shared across the suite. Setup failures are reported as harness errors, rather than task failures. Each run records the Python version and requirements-file digest, and keeps setup output in `environment.log`.

A task may also specify `isolated_setup` as lists of command arguments. The harness runs each command directly in the run's environment before timing, with output in `environment.log`; failures or changes to tracked or nonignored checkout files make the setup fail. `{workdir}` names the run's checkout and `{taskdir}` the benchmark task's directory. Ignored dependencies may be installed in the checkout. Only use commands appropriate for the task, since they run project tools and can access the network.

The isolated home's login profiles restore that `PATH` after system profiles run. On macOS, `path_helper` otherwise puts system tools ahead of the run's tools and can expose host-installed programs to the plain arm.

The user's own `~/.claude`, `~/.codex` and jouet ledger are never read or written. The one exception is authentication:

- An API key or token in the environment (`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`; `OPENAI_API_KEY` or `CODEX_API_KEY`) is passed through. This is the dependable way to run a long suite. For Claude Code, `claude setup-token` prints a long-lived token for `CLAUDE_CODE_OAUTH_TOKEN`, and nothing then needs refreshing.
- Without one, each run gets a copy of the harness's credentials file (`.credentials.json` or `auth.json`), readable only by you. A run whose access token has expired refreshes it, and that spends the old refresh token. So after the run, a copy that changed is written back over your file, atomically and under a lock. If your file changed during the run too, the newer of the two is kept. The run's record lists what was written back.

A link to the file would not survive this: the harnesses replace the file when they refresh, which turns the link into a file of its own, and the run directory is deleted afterwards. That logs you out of your own CLI.

When a run fails because the harness cannot log in at all, the suite stops instead of recording a failure for every remaining run. An agent in a run can read the credentials file, as it could anyway.

### Running the agents

- **Claude Code:** `claude -p --output-format stream-json --verbose --model M --effort E --permission-mode bypassPermissions --strict-mcp-config --settings '{"syncClaudeAiSkills": false}' --max-budget-usd N`. The stream carries each response's usage, and its last event has the totals, turn count and cost. Skill syncing is off because a claude.ai login otherwise downloads the account's skills in the background, and their listing, about 3.8k tokens, joins every request after it lands. It landed in longer runs and not in shorter ones, so it counted against whichever arm made more requests. Runs before this change include it.
- **Codex:** `codex exec --json --model M -c model_reasoning_effort=E --dangerously-bypass-approvals-and-sandbox --dangerously-bypass-hook-trust`. Codex skips hooks nobody has trusted, and the throwaway `CODEX_HOME` has trusted none. Its usage arrives only at the end of the turn, so the per-run token cap is enforced from the session file Codex writes as it goes.

### A fake GitHub

The PR tasks talk to a local fake GitHub (`bench/jbench/fake_github.py`). Both arms reach the same fake through wrapper scripts, so the agent's environment carries no hint of it:

- `gh` gets `GH_HOST=github.localhost`, the one host `gh` talks to over plain HTTP, with the fake as its HTTP proxy, and `GH_REPO`;
- `jouet` gets `JOUET_GITHUB_API_URL`.

The fake executes real GraphQL queries (variables, aliases, fragments, connections, filters, the review-thread mutations) against the fixture, serves its REST responses, and switches any object to its final state a set number of seconds after the run starts, which is how checks stay pending and then fail. Every request is logged. A request the fake cannot answer gets a 404, as GitHub would answer it, and the report lists it, so a gap in a fixture is not mistaken for an agent's mistake.

`mise run test:bench` proves each task without running an agent:

- `check.py` fails on the untouched fixture and passes after `solve.py`;
- each task's probes run against the fake and get the exit codes they expect, with no request it cannot answer. Probes are the commands an agent would use: `jouet wait ci`, `jouet pr next`, `jouet pr threads` (listing, replying and resolving), `gh pr checks`, `gh run view --log-failed`, `gh pr view` and `gh api graphql`. A probe's `before` commands run first against the same fake, so a listing after a reply is checked too.

### What can't be held constant

- **The model is not deterministic.** Neither CLI exposes a temperature or a seed. So each task runs several times per arm, and the arms are compared in pairs: round *k* of the plain arm against round *k* of the jouet arm. Within a round, the arm order is shuffled by the seed, so a slow hour or a provider incident falls on both arms rather than on one.
- **Model and harness versions change.** The summary records the CLI versions and models, and a comparison is only meaningful within one run of the suite.
- **GitHub state** is replaced by the fake, so PR tasks are the same every time. The fake's clock starts with the run, so how long the agent waits is part of what is measured.
- **The prompt cache** depends on what ran recently on the same account. Runs interleave arms, so both see the same cache conditions on average, and the report shows cached and uncached input separately.
- **The network** is not sandboxed. A prompt says not to push, and the fixture's remote points at a repository that does not exist.
- **macOS process cleanup** uses `lsof` to find detached processes whose working directory remains in the run. A process that detaches and leaves that directory, or whose path contains control characters escaped by `lsof`, may survive cleanup. Discovery errors are recorded with leftover processes.

### Metrics

Every metric comes from one run's isolated files:

- **Tokens:** input (all the context read, cached or not, which is how `jouet audit` counts), input not served from the cache, output, and Claude Code's reported cost. These come from the harness's own output.
- **Model requests, tool calls, failed calls and tool output bytes:** from `jouet audit --json` over the run's transcripts, the same readers `audit` and `stats` use. Requests are counted the same way for both harnesses.
- **Wall time:** from starting the agent to its exit.
- **Success:** the task's check passes, the agent exits successfully, and the run finishes within its timeout and budget. Older records scored only the check and run limits; regenerating their reports does not change those recorded outcomes.
- **Rule decisions:** from the run's jouet ledger, per rule and action.

The headline token and time differences use only pairs where both arms passed, and show the number of those pairs alongside each arm's success rate. The detailed diagnostic metrics also include failed and stopped runs; they are useful for investigating failures but do not compare completed tasks. The report counts paired outcomes (both pass, only plain passes, only jouet passes, or both fail). Harness errors are excluded from success rates and paired outcomes; unpaired runs are also excluded from paired outcomes. For example, a Codex model-capacity error before a run starts is a harness error, not a failed task. For each task, harness and arm, the detailed report gives the median and quartiles of each metric. For each pair of arms, it gives the median difference (jouet minus plain), its range, how many rounds jouet used less, and an exact two-sided sign test. With five pairs, the smallest p-value that test can give is 0.0625, which is the honest answer for five pairs. Both-pass comparisons are conditional on success and do not measure the cost of failures.

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
| `--tasks-dir` | none | another directory of tasks, searched after `bench/tasks/`; repeatable. `JOUET_BENCH_TASKS` adds more, as a path list |
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
| `--jouet` | `target/debug/jouet` | the jouet binary the jouet arm runs. It is copied once when the suite starts, so a rebuild during the suite neither changes nor removes what is measured |
| `--keep-work` | off | keep each run's work tree and home, for debugging; its result records `workdir` and `tmp_dir` |

A run stopped by its timeout or a budget counts as a failure, and the report says which stopped it.

Results go to `bench/results/<timestamp>/`, which git ignores:

- `meta.json`: versions, models, seed, budget, benchmark checkout commit and the SHA-256 of the jouet binary snapshot used by the suite. The commit is the benchmark checkout's HEAD; it does not capture uncommitted changes or establish which source built an externally supplied `--jouet` binary. Older records do not have the binary hash;
- `runs.jsonl`: one record per run, with the directory its task came from (`task_dir`);
- `summary.json` and `summary.md`: the comparison;
- `runs/<task>.<harness>.<arm>.r<round>/`: agent events, transcripts and `jouet audit` output where available, the check's output and, for PR tasks, every request the fake GitHub saw and its final state. Ephemeral Codex runs have only selected events and no transcript or audit report.

Each run's `changed_paths` lists paths whose contents or Git index entries changed between setup and the checker, including committed, staged and untracked changes. The summary shows up to ten paths per run and points to `runs.jsonl` when more were recorded; `summary.json` retains the full list. Staging an existing file can list it even when its contents did not change. This is evidence for a scope review, not a pass/fail judgment or a substitute for reviewing the diff. Ignored files and changes inside submodules are not included. `null` means the snapshots could not be compared; it does not mean nothing changed. The snapshots use a temporary Git index and do not change the agent's index.

### Tasks outside this repository

Some tasks cannot be published: their source is a private repository, or their fixture names one. Keep those in a directory of your own, laid out like `bench/tasks/`, one directory per task, and name it with `--tasks-dir` (repeatable) or `JOUET_BENCH_TASKS` (a path list, `:`-separated on Unix). Both `run` and `check-tasks` read the same directories. A task's name is its directory's name, and a name found in more than one directory is an error rather than an override.

```sh
python3 bench/agents.py check-tasks --tasks-dir ~/bench-private --tasks invoice-rounding
JOUET_BENCH_TASKS=~/bench-private mise run bench:agents --tasks invoice-rounding --harness claude --out ~/bench-results/invoice-rounding
```

A check or reference solution outside `bench/tasks/` can still use the shared helpers: `BENCH_CHECK_HELPERS` names the directory holding `_check.py`, so the script starts with `sys.path.insert(0, os.environ["BENCH_CHECK_HELPERS"])`. `meta.json` lists the directories a suite read (`task_dirs`), so a results directory says where its tasks came from. `mise run test:bench` reads only `bench/tasks/` unless `JOUET_BENCH_TASKS` is set.

### In CI

The `bench agents` workflow never runs on its own. Someone dispatches it, and by default it is a dry run of the smoke task. Untick `dry_run` to run agents with the `BENCH_ANTHROPIC_API_KEY` and `BENCH_OPENAI_API_KEY` secrets. The summary goes to the job summary and the results directory to an artifact. `mise run test`, which CI runs, includes `test:bench` and never starts an agent.

The workflow installs the harnesses, Python and GitHub CLI from `mise.bench.toml`, at the versions `mise.bench.lock` pins. That file is loaded only with `MISE_ENV=bench`, so it does not change the tools you run day to day in the repository. Install the benchmark tools with `MISE_ENV=bench mise install`, then run the suite with `MISE_ENV=bench mise run bench:agents`. Bump the versions deliberately with `MISE_ENV=bench mise lock --bump`.

## Why it is built this way

- **A mise task and a Python script, not `jouet bench`.** The harness is for maintainers. Shipping it in the binary would give every user a command that spends their tokens to measure jouet, plus fixtures and a fake server they do not need. mr-boxington's real-world benchmarks set the pattern: a standard-library-only Python script behind a mise task, writing JSON and Markdown.
- **Not tak.** tak times a command many times and gates on instruction counts. An agent run takes minutes, needs a per-run timeout and a fresh home, and its interesting numbers are tokens and requests, which tak does not measure. The harness times each run itself and keeps tak's habits: interleaved, seeded rounds, and medians with their spread.
- **`jouet audit`'s readers, not new ones.** The tool-call counts are read the way `audit` and `stats` read real sessions, so a gap between this benchmark and `stats`' estimates is a calibration finding, not two parsers disagreeing.

## Limits

- Five small tasks in Python repositories are not a survey of real work. They cover the chores jouet targets, so a result says how much jouet helps on those chores, not on a whole session.
- A few runs per arm only detect large effects. Read the spread and the pair counts before the medians.
- The fake GitHub answers what its fixtures hold. An agent that asks for something else gets a 404, which the report lists, and a different answer than GitHub would give.
- The primer that `jouet setup` installs adds context to every request. The benchmark measures that cost too, which is part of the point.
