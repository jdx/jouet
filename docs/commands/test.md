---
description: Run the full test suite, only the tests a change affects, or any test or build command, and get each failure with its error and a log of the whole output.
---

# jouet test

`jouet test --affected` selects tests related to your changes, which can reduce the work needed while iterating. It works the same way in every ecosystem it supports. An adapter hands the selection to the test runner's own machinery (vitest's and jest's module graphs, pytest-testmon's coverage data, cargo's crate graph), or to jouet's code graph where the runner has none. jouet adds the same things everywhere: the changed set, the cases where only the whole suite is safe, an explanation of each pick, and a record that the run was partial.

```sh
$ jouet test --affected
changes since origin/main (merge base 1a2b3c4d5e6f): 2 files
vitest (vitest's module graph): 2 test files
  test/calc.test.js: imports src/calc.js → src/sum.js (changed)
  test/sum.test.js: imports src/sum.js (changed)
  ignored: README.md (documentation)
  run: node_modules/.bin/vitest run --changed 1a2b3c4d5e6f
vitest: passed in 1.2s: 5 passed (full log: ~/.local/state/jouet/logs/test-1793449700864-936641-0.log)
This was a partial run; the full suite was not verified.
```

Each picked test is listed with the chain of imports that links it to a changed file. Files that cannot affect a test are listed as ignored, and the plan ends with the command it runs. `--dry-run` prints the same plan and runs nothing. The text lists the first 20 tests with their reasons and counts the rest; `--json` lists them all.

Without `--affected`, `jouet test` runs the [full suite](#the-full-suite), and `jouet test --cmd -- CMD` runs [any other](#any-command-cmd) test, e2e or build command. Every run keeps its whole output in a log and prints, for an agent, [only what it needs](#what-a-run-prints): one line when it passes, each failure with its error when it fails.

```text
jouet test [--affected] [--since REF] [--adapter NAME] [--baseline] [--fresh] [--dry-run] [--budget BYTES] [-- ARGS...]
jouet test --cmd [--baseline] [--fresh] [--since REF] [--budget BYTES] -- CMD...
```

Words after `--` go to the test runner, or with `--cmd` are the command. `--since` and `--adapter` choose what `--affected` selects, so they need it; `--since` also names the base for `--baseline`. Each flag is described in the [CLI reference](/cli/test).

## How `--affected` picks tests

jouet first works out the [changed set](#the-changed-set): every file that differs from the merge base with the upstream or default branch, committed or not, plus untracked files. It then decides, for each adapter, what [each changed file means](#what-each-changed-file-means). Documentation is ignored. A lockfile, tool versions, CI configuration or the runner's own configuration means the whole suite. A file the adapter can trace goes to the adapter, which turns it into tests, with the runner's own selection where it has one. Any other file in the project, such as a JSON fixture a test reads at run time, means the whole suite too.

### Supported test runners

| Adapter | For | Picks tests with |
|---|---|---|
| [`vitest`](#vitest) | JavaScript and TypeScript | vitest's module graph |
| [`jest`](#jest) | JavaScript and TypeScript | jest's dependency graph |
| [`pytest`](#pytest) | Python | pytest-testmon's coverage data when it is installed, else a static import graph |
| [`unittest`](#unittest) | Python without pytest | a static import graph |
| [`cargo`](#cargo) | Rust | the crates that changed and every workspace crate that depends on them |
| [`graph`](#graph) | Go, Ruby and Java | jouet's code graph |

jouet uses every adapter it finds in the [nearest directory that has any](#where-it-runs); `--adapter NAME` keeps one of them. Native adapters for Go and Ruby are [planned](#planned).

### The whole suite, or nothing

When jouet cannot trace a change to tests, it runs the whole suite and says why. Here the change edits the README and adds `data/cases.json`, a fixture that no import reaches:

```output
$ jouet test --affected --dry-run
changes since main (merge base fa8c0f1d3e37): 2 files
unittest (a static import graph (best-effort: dynamic imports are invisible to it)): the whole suite, because unittest cannot trace data/cases.json to the tests that use it
  ignored: README.md (documentation)
  would run: python3 -m unittest discover -s tests
```

A change to documentation alone runs no tests, and says so:

```output
$ jouet test --affected
changes since main (merge base fa8c0f1d3e37): 1 file
unittest (a static import graph (best-effort: dynamic imports are invisible to it)): nothing to run: only documentation changed
```

Nothing runs either when nothing has changed since the base, when every change is in another ecosystem's package, or when the adapter selects no tests for the changed files. Each of these exits `0` and names its reason. In `--json`, a suite's `decision` is `affected`, `full` or `nothing`, with a `reason` for the last two.

## The changed set

The changes are everything that differs from the merge base of `HEAD` and the base ref, committed or not, plus untracked files git does not ignore. A rename counts as a deletion and an addition.

The base ref is `--since REF` when it is given. Otherwise it is the branch's upstream, else the remote's default branch, else a local `main` or `master`. An upstream with the branch's own name, as `git push -u` sets it, is skipped: once the branch is pushed, it holds the change itself, so the default branch is used instead. With none of these, the base is `HEAD`, and only uncommitted and untracked files count. A `--since` ref that does not exist, or shares no history with `HEAD`, is a usage error (exit `2`).

## What each changed file means

Each changed file is classified, for each adapter, in this order:

| Change | Effect |
|---|---|
| Documentation: `*.md`, `*.rst`, `*.txt` (not `requirements*.txt`), `LICENSE`, `CHANGELOG`, `.gitignore`, `.github/ISSUE_TEMPLATE/` and the like, except under `fixtures/`, `testdata/`, `snapshots/` or `__snapshots__/`, which tests read | ignored |
| A lockfile (`package-lock.json`, `pnpm-lock.yaml`, `yarn.lock`, `bun.lock`, `uv.lock`, `poetry.lock`, `requirements*.txt`, `Cargo.lock`, `go.sum`, ...) | the whole suite |
| Tool versions or settings (`mise.toml`, `mise.lock`, `.tool-versions`, `.nvmrc`, `.python-version`, `.npmrc`, ...) | the whole suite |
| CI configuration (`.github/workflows/`, `.gitlab-ci.yml`, `.circleci/`, `.buildkite/`, ...) | the whole suite |
| The project's own manifest or test configuration (see each adapter) | the whole suite |
| A file the adapter traces (see each adapter) | passed to the adapter |
| A file another adapter in the same directory traces | ignored by this one |
| Any other file in the project, such as a JSON fixture read at run time | the whole suite: the adapter cannot trace it |
| A file in another package of the same ecosystem | as if it were in the project |
| A file in another package of another ecosystem (a Go service beside a JavaScript app) | ignored |
| A file in no package at all | the whole suite |

A package is a directory with a manifest: `package.json`, `pyproject.toml`, `setup.py`, `setup.cfg`, `Cargo.toml`, `go.mod`, `Gemfile`, `pom.xml`, `build.gradle` or `Package.swift`.

## Where it runs

jouet looks for adapters in the current directory, then each directory above it up to the repository root, and uses the first directory that has any. Every adapter found there runs, one after the other (a directory with both `package.json` and `pyproject.toml` runs vitest and pytest), unless `--adapter` picks one. To test one package of a monorepo, run `jouet test --affected` from that package's directory.

When the root has only a graph adapter and changed files belong to a detected nested project with a native adapter, jouet stops and names those projects instead of running only the root suite. Run affected checks from each relevant project and the root checks as needed. `--adapter graph` explicitly selects only the root adapter. The check relies on test project files present in the worktree and does not detect nested graph-only projects, since those may be modules the root runner covers. It does not discover every possible nested suite.

## Adapters

### vitest

- **Found by** a `vitest.config.*` or `vitest.workspace.*` file, or `vitest` in `package.json`'s dependencies. Scripts are not read: renovate, for one, has a script named `jest` that runs vitest.
- **Selects with** `vitest list --changed <merge base> --filesOnly`, and runs `vitest run --changed <merge base>`. That is vitest's module graph, which follows every import vitest resolves, aliases and workspace packages included.
- **Traces** `.js`, `.jsx`, `.ts`, `.tsx`, `.mjs`, `.cjs`, `.mts`, `.cts`, `.vue`, `.svelte`, `.json` and style files (`.css`, `.scss`, `.sass`, `.less`).
- **The whole suite for** `package.json`, `vitest.config.*`, `vitest.workspace.*`, `vitest.projects.*`, `vite.config.*`, `tsconfig*.json`, `jsconfig.*`, Babel, SWC and PostCSS configuration, `.env*` files, setup files (`setup*`, `*.setup.*`, `global-setup`) and `__mocks__/`.
- **Limits:** a test that reads a file at run time without importing it, such as a fixture loaded with `fs.readFileSync`, is not in the module graph. A deleted file runs the whole suite, because the graph cannot follow a file that is gone.
- **Runner:** `node_modules/.bin/vitest` in the project or a directory above it. Yarn Plug'n'Play installs, which have no `node_modules`, are not supported yet.

For a project with a non-default Vitest configuration, run from that package and pass it to the runner, for example `jouet test --affected -- --config vitest.acceptance.config.ts`. jouet uses the same configuration when listing and running the affected tests. A change to that configuration runs its whole suite. This limits the run to that configuration; other suites are not verified. For Vitest and Jest affected selections, jouet refuses the plan if a changed, conventionally named test file is absent from every selected JavaScript runner. Check the runner configuration or run the missing tests directly with `jouet test --cmd`. Planning, including `--dry-run`, may ask whole-suite runners to list their files when changed tests are not already covered. Listing stops once those tests are covered; a listing failure prevents the plan only if coverage remains unestablished. This check does not cover nonconventional test names.

### jest

- **Found by** a `jest.config.*` file, a `jest` key in `package.json`, or `jest` in its dependencies.
- **Selects with** `jest --listTests --findRelatedTests <changed files>`, and runs `jest --findRelatedTests <changed files>`: jest's dependency graph (its haste map).
- **Traces** the same files as vitest.
- **The whole suite for** `package.json`, `jest.config.*`, `jest.preset.*`, `tsconfig*.json`, `jsconfig.*`, Babel, SWC and PostCSS configuration, `.env*` files, setup files and `__mocks__/`.
- **Limits** and **runner:** as for vitest, with `node_modules/.bin/jest`. An explicit `--config` or `-c` also applies to both selection and execution, and a change to that config runs its whole suite.

### pytest

- **Found by** `pytest.ini` or `conftest.py`, `[tool:pytest]` in `setup.cfg`, `[pytest]` in `tox.ini`, or `pytest` in `pyproject.toml`, `requirements.txt` or `requirements-dev.txt`.
- **Runner:** `pytest` from a `.venv` or `venv` in the project or the repository root, the active virtualenv, `PATH`, or `python3 -m pytest`.
- **With pytest-testmon installed** (`import testmon` works in pytest's Python), it runs `pytest --testmon`, which selects by the coverage each test recorded on earlier runs. That is the most precise selection jouet offers, since it sees calls as well as imports. testmon compares with its own last run, kept in `.testmondata`, rather than with the merge base; with no `.testmondata` yet, the first run runs everything once to record coverage. jouet sets `COVERAGE_CORE=ctrace` for testmon, unless you set `COVERAGE_CORE` yourself: from Python 3.14, coverage's default core records no per-test contexts, and testmon then misses tests that reach a change through another module.
- **Without testmon**, jouet builds a static import graph, labelled best-effort. It reads every `import` and `from ... import` in the repository's Python files (inside functions and `if TYPE_CHECKING:` too), and resolves them against the project, its `src/`, pytest's `pythonpath` and each test's own directory. It then runs the test files (by pytest's `python_files`, `test_*.py` and `*_test.py` by default) that import a changed module, directly or through other modules. Importing `a.b.c` also depends on `a/__init__.py` and `a/b/__init__.py`. A deleted module selects the tests that still import it.
- **Traces** `.py` and `.pyi`.
- **The whole suite for** any `conftest.py`, `pyproject.toml`, `setup.py`, `setup.cfg`, `pytest.ini`, `tox.ini`, `noxfile.py` and `.coveragerc`.
- **Limits of the import graph:** dynamic imports (`importlib.import_module`, `__import__`), plugins and entry points, and data files are invisible to it. It can select too much: a module name found under two roots selects the tests of both.

#### Project options for affected pytest runs

A repository can give pytest extra arguments for affected runs in `.jouet/test.toml` at its root. This one leaves out tests marked `download`:

```toml
[pytest]
affected_args = ["-m", "not download"]
```

Use it only where leaving those tests out of an affected run is right for the project. The arguments go to the selected run and to pytest-testmon's collection. They do not go to `jouet test`, to `--cmd`, or to the whole-suite run that `--affected` falls back to when it cannot safely narrow the change. A note in the output says the arguments restrict the run, the command shown includes them, and the run is still recorded as partial.

The file is included in the trust hash, so review the repository's `.jouet` files before a person runs `jouet trust`. While the file is untrusted, or has changed since it was trusted, an affected run with a pytest suite stops before running any test (exit `4`).

### unittest

- **Found by** a test file (`test*.py`) in the directory, its `tests/` or its `test/` that imports `unittest`, where pytest is not found: a pytest project runs its unittest tests through pytest.
- **Runner:** `python -m unittest` with the Python of a `.venv` or `venv` in the project or the repository root, or `python3`. The full suite is `python3 -m unittest discover`, with `-s tests` (or `-s test`) when that directory is not a package, since discovery from the project would not enter it.
- **Selection:** the same best-effort static import graph as pytest's without testmon, with unittest's `test*.py` as the test files. It runs `python3 -m unittest` on the selected files.
- **Traces** `.py` and `.pyi`.
- **The whole suite for** `pyproject.toml`, `setup.py` and `setup.cfg`.

### cargo

A Rust workspace's tests, by crate. In Rust, compiling dominates, so the saving is in what is built.

- **Found by** a `Cargo.toml` with a `[package]` or a `[workspace]`.
- **Selection:** `cargo metadata --no-deps --offline` gives the workspace's members and their path dependencies on each other. A changed file belongs to the member whose directory holds it (the deepest, when one crate sits in another's directory). The crates picked are the changed ones and every member that depends on one, directly or through others, dev-dependencies included. A file outside every member, such as a shared fixture directory, belongs to the members whose Rust files name it, or a directory holding it, in a string with a `/`. A string is read from the crate's directory (`join("../corpus/complete")`) and from the root, or, in `include_str!`, `include_bytes!` or `include!`, from the file's own directory; a placeholder (`"cases/{name}.json"`) stands for the rest of the name. Each pick shows the chain of crates that reaches a changed one. When `cargo metadata` fails, the precision line says why, and a change to any file it would have traced runs the whole suite.
- **Runner:** `cargo test -p A -p B ...`, with the words after `--` passed on.
- **Other languages in a crate:** a changed file of a language cargo builds no tests from (`.py`, `.bats`, `.js`, `.ts`, `.go`, `.rb` and the like) still picks its crate, but a note names it, since `cargo test` leaves its own tests alone: `note: cargo runs no tests of test/stack.bats, bench/jbench/costs.py: run their own tests too`. Other files, such as `.luau` or `.toml`, can reach a test through `include_str!` and stay the crate's.
- **Traces** every file inside a member's directory except its `target/`, since a test can read a fixture with `include_str!`.
- **The whole suite for** the workspace manifest, `.cargo/config.toml` (or `.cargo/config`), `rust-toolchain(.toml)` and `Cargo.lock`, and a file outside every member that no member names. The whole suite is `cargo test`, with `--workspace` in a workspace.

### graph

The fallback for languages without a native adapter, built on jouet's [code graph](/commands/graph).

- **Found by** `go.mod` (Go), a `Gemfile` (Ruby), `pom.xml` or `build.gradle(.kts)` (Java). It runs only where no native adapter covers the language.
- **Selection:** the graph is brought up to date, then followed backwards from each changed file, through imports, calls, references and inheritance, to the test files that reach it, by codegraph's test conventions (`*_test.go`, `spec/` and `*_spec.rb`, `test/`, `*Test.java` and the like). For Go, only files ending in `_test.go` select test packages; other Go files in `testdata` can still connect a dependency path. Tests in a changed Go file's own package are included even when the graph misses a link. Graph picks show the chain of files that reaches them.
- **Runner:** `go test` on the selected tests' packages; `bundle exec rspec` on their files (with `.rspec` or a `spec/` directory), else minitest through `bundle exec ruby -Itest -Ilib`; `mvn test -Dtest=…` or `gradle test --tests …` on their classes, through `mvnw` or `gradlew` when the project has one. The whole suite is `go test ./...`, `bundle exec rspec` or `bundle exec rake test`, or `mvn test` or `gradle test`.
- **Go package preflight:** before running or displaying an affected plan, jouet asks `go list` whether the selected packages can be loaded with the active build settings. It does not download modules or a toolchain. If a package is excluded by build constraints, belongs to another module, or cannot be validated with installed dependencies, it stops with a precondition error and names the packages; choose the relevant tests or fix the environment. Explicit `-tags`, module and overlay options after `--` are used for the preflight, except `-mod=mod`, which is refused because planning could change module files. This checks that selected packages can load, not that the graph found every affected test.
- **The whole suite for** `go.mod`, the `Gemfile`, `.rspec`, a spec or test helper, the Maven or Gradle build, test data, a deleted file (the graph cannot follow a file that is gone), and a file the graph could not index. For Go, an unparseable or unindexed Go file anywhere in the project also runs the whole suite: it could hide a path from a changed file to a test.

### Planned

These fit the same adapter trait and are follow-ups:

- **go:** `go list -deps` gives the package graph, on top of Go's own test cache.
- **Ruby:** best-effort, from coverage maps in the style of crystalball, or file naming conventions.

## The full suite

`jouet test` without `--affected` runs the repository's test task, found at the repository root. It takes the first of these:

- a mise `test` task (`mise run test`). jouet asks mise about it (`mise tasks info test`), so includes, globs and file tasks count as mise reads them, and reads the config files when mise cannot answer;
- a package.json `test` script, run with the package manager its lockfile names (`pnpm`, `yarn`, `bun`, else `npm`), unless it is `npm init`'s placeholder that only fails;
- `Cargo.toml` (`cargo test --workspace`);
- a justfile or Makefile `test` target;
- `go.mod` (`go test ./...`).

Without one, it runs the whole suite of each adapter found. Words after `--` are passed on, after a `--` of its own for `npm test`.

### The same command on the same files: `--fresh`

An agent often runs a check again on files that have not changed since it last ran: to compare a slow failure with `--baseline`, after a stop-gate block, or to read the result once more. By default `jouet test` runs the command each time. A project can opt in to reusing a result by setting `[test] reuse_s` or `JOUET_TEST_REUSE_S` to a positive number of seconds. With reuse enabled, a matching run is answered from the last one:

```output
$ jouet test --baseline --cmd -- mise run check
reused: `mise run check` ran on these same files 3m ago (115s, exit 1); --fresh runs it again
```

The output, the log, the [baseline](#failures-that-were-there-before-baseline) comparison and what the stop gate records are those of the earlier run, and `--json` marks the suite `"reused": true`.

A run is answered this way only when every one of these is the same: the contents of the working tree (tracked and untracked files, less those git ignores), HEAD, the command, the directory it runs in, and the environment. A run is kept for this only when it left the working tree as it found it. It is kept for `[test] reuse_s` seconds (`JOUET_TEST_REUSE_S`, 0 by default).

Ignored files are not part of the key. A reused result can therefore be stale after an ignored input changes, and a reused command does not recreate deleted ignored outputs. Commands can also depend on network or service state or be flaky. Opt in only when those limits are acceptable; `--fresh` runs the command and keeps its result for the next run.

### Failures that were there before: `--baseline`

A full suite can fail for reasons the change has nothing to do with: a test that needs a tool this machine lacks, or one already broken on the default branch. Fixing the environment to make it pass costs far more than the change did. So a failing run is compared with the merge base instead: on its own for the full suite, and with `--baseline` for any run.

```console
$ jouet test --baseline
unittest: failed (exit 1) in 0.1s: 1 failed, 1 passed
FAIL test_env (test_a.T.test_env)
  …
baseline: the failure also fails at main (302afa77cd40) with matching failure details; counted as passing: test_env (test_a.T.test_env)
```

When the run fails, jouet runs each failed suite again in a temporary worktree of the merge base with the upstream or default branch, and compares the names and captured failure details. `--since REF` names another branch or commit. An upstream of the branch's own name, as `git push -u` sets, holds the change itself, so the default branch is used instead. Any path into the checkout in the command is pointed at the worktree, and the worktree is removed afterwards. On Unix, jouet first initializes submodules from the current checkout when it has the exact pinned commits, copying Git objects into the temporary worktree and preserving the configured remotes. For other submodules it uses Git's normal initialization and URLs.

- If every failure also fails at the merge base with matching, unambiguous details, the run passes (exit `0`) and names them. The stop gate counts the coverage actually run; an affected run remains partial.
- Matching names with missing, ambiguous or different details remain unresolved (exit `1`). A Ruff finding at a different location in the same file with the same rule also remains unresolved: it may have shifted or may be a new occurrence. When the baseline was run again, the output names its log; JSON includes its path in `baseline.logs`. A reused baseline may not have a log to report.
- A complete, explicitly echoed `remark --use remark-validate-links --frail --quiet` failure ending in pnpm's `ELIFECYCLE` trailer can also be compared by file, location, and full warning details. Incomplete output or other visible failure markers remain unresolved.
- A direct, non-mutating `ruff check` with complete default-format diagnostics can be compared by rule, location, and full diagnostic details. Other formats, incomplete or mixed output, and commands that change files remain unresolved.
- A failure the merge base does not report still fails the run (exit `1`) and is labeled `NEW FAILURE`. This says the current run failed and no matching failure was reported at the merge base; inspect the logs when you need to establish why.
- When the run at the merge base fails without naming any failing test, as when a tool it needs is missing from that checkout, nothing is compared and the run still fails.
- When pnpm reports missing `node_modules`, or a project-local executable in it cannot be found in the merge-base checkout, jouet points to the optional baseline setup below; it does not install dependencies automatically.
- When a multi-test merge-base run reports only failures, with no passes or skips, the result is not compared: a setup problem may have prevented the tests from running. This also applies to a kept baseline.
- When a single failing command's failure names are missing or cannot be matched safely, explicit `--baseline` runs the command at the merge base. If both fail, jouet reports that it cannot compare the failures and retains the nonzero result; the stop gate asks for an explanation once. If the merge base passes, the failure remains unresolved. When multiple failing commands include unnamed failures, jouet does not run the comparison.
- When the merge base ran fewer than half as many tests, a failure it does not list may never have run there, so it is listed as not compared rather than new.
- When Nextest reports that it stopped after a failure and did not run all selected tests, jouet keeps the run failed and does not compare it at the merge base. Rerun with `--no-fail-fast` so the remaining tests run. In JSON the suite includes `nextest_incomplete: true`. This recognizes Nextest’s explicit warning; other runners can stop early without a warning jouet recognizes.

When every failure is a Go test that `go test` named with its package, jouet first runs only those tests at the merge base, as `go test -run '^(TestA|TestB)$' PKG...`, even for a `--cmd` run too slow to compare on its own, such as a `mise run check` that lints and tests everything. That takes seconds where the whole command can take minutes. If each failure fails there with the same details, it was there before the change. Any other result goes on to the comparison above, so only a run of the whole command can call a failure new. A project whose tests need the setup below skips this step.

The comparison needs a runner whose failures jouet recognises by name, and costs one more run of the failed suites. With `--affected --baseline`, jouet selects the tests once from the change, then replays those exact commands at the merge base; it does not select again there. For Vitest and Jest, the run names the selected files directly, so their Git- and module-graph-based selectors cannot choose different files at the merge base. It runs a fresh comparison rather than using a cached result. A missing test, missing dependencies in the isolated checkout, or an uncomparable result does not prove a failure was pre-existing and does not clear the run. A successful comparison clears the matching affected failure at the same work tree; it does not clear a full-suite failure or an affected failure recorded at a different tree.

The comparison uses full captured failure blocks, not the shortened display, and normalizes checkout paths and Rust thread IDs. Other variable output can prevent a match. Identical output is evidence, not proof that the underlying cause is the same; inspect the logs when that distinction matters. Name-only cached failures from older versions are not reused.

For a project whose tests need dependencies installed in each checkout, configure an optional setup command in `.jouet/test.toml`:

```toml
[baseline]
setup = ["npm", "ci"]
```

After reviewing the configuration, a person runs `jouet trust`. When a comparison actually runs, jouet executes this argument list once in the root of its temporary merge-base checkout, before the test commands. It invokes the program directly, without a shell, and does not run setup in the current worktree. The program can run project scripts and access the network as the user; trust it accordingly. A missing program or unsuccessful setup leaves the failures uncomparable, with its output in the reported log. A configured setup uses a fresh comparison, rather than a cached result.

**What the merge base did is kept** for 14 days in `$XDG_STATE_HOME/jouet/known/`, by repository (every worktree of it shares it), merge base, exact command arguments, working directory and environment. A later failure with the same invocation and environment at the same merge base is compared with what is kept, without running the merge base again. Environment values are included in a digest rather than stored in the cache. Changing an inherited or job-specific environment variable requires a new comparison, including changes to `PWD` or agent session IDs between worktrees or sessions. A command can read any of these values, so they are not ignored merely because they often change. Cached results made before this key included the environment are not reused. If the kept run could not name any failures, passing `--baseline` explicitly retries it, for example after installing a missing tool.

**Without `--baseline`:**

- `jouet test`, the full suite, runs a failing suite at the merge base on its own, once per merge base and command.
- `jouet test --cmd` runs a failing run that took under a minute at the merge base on its own too, once per merge base and command, so a failure that was already there is said without the agent asking. A slower failing run is compared with what is kept for its command, and runs nothing at the merge base, because a test written to fail, or a build broken by the change, would cost a second long run; its result names the `--baseline` command that compares it. Run that once to keep what the merge base does, as for a cross-compile that needs a tool this machine lacks.
- `--affected` and `--dry-run` compare nothing. If an affected run names failing tests, jouet suggests the equivalent `--baseline` command; it does not run that comparison automatically. When a failing runner is under `node_modules` and no trusted baseline setup is configured, the hint also says the isolated checkout may need dependencies. The hint is omitted when automatic baseline comparison is configured off.
- `[test] baseline = "off"` in the config file, or `JOUET_TEST_BASELINE=off`, compares only with `--baseline`. `[test] baseline_cmd_under_ms` (`JOUET_TEST_BASELINE_CMD_UNDER_MS`, 60000 by default) is how short a failing `--cmd` run must be to run at the merge base on its own: raise it for a project whose focused tests are slow but whose failures are often already there.

When the merge base could not run the command either, the [stop hook](/setup#finishing-on-a-failing-check) asks about those failures once, and not again until the command passes, since nothing on this machine can tell more.

When the merge base is `HEAD` and nothing has changed since it, as after committing on `main` itself with no upstream, a run there would test the same code. `--baseline` then refuses before running anything (exit `2`), and without it nothing is compared. Name the commit the change started from with `--since`, such as `jouet test --baseline --since HEAD~1` for a change of one commit.

## Any command: `--cmd`

`jouet test --cmd -- CMD` runs CMD in the current directory, directly rather than through a shell, with the same log and digest as a test run. Use it for one test, an e2e suite or a build:

```sh
$ jouet test --cmd -- cargo build
cmd: failed (exit 101) in 0.1s
FAIL src/lib.rs:1:45: error[E0425]: cannot find value `undefined_var` in this scope
    |
  1 | pub fn add(a: i32, b: i32) -> i32 { a + b + undefined_var }
    |                                             ^^^^^^^^^^^^^ not found in this scope
full log: ~/.local/state/jouet/logs/test-1793449979466-951516-0.log
```

The built-in `piped-test-output` rule points agents here. It warns the first time in a session, and denies after that, when an agent pipes a test, build or e2e run other than `jouet test` into `head`, `tail`, `grep`, `rg` or `sed -n`, or writes its output to a file and greps that file in the same call. It suggests the `jouet test --cmd` command for it, or plain `jouet test` when the command is the whole suite (`cargo test`, `python3 -m unittest`, `pytest`, `go test ./...`, `npm test`, `mise run test`, ...). It leaves lints to the `scoped-lint` rule, and stays quiet for `--help` and `--list`, and for output written to a file the call does not read back. It denies only once it has already warned in the session, counted from the hooks' ledger, because a warning alone was followed once and then forgotten.

With `jouet setup`'s hooks, `jouet test --cmd -- CMD` is recorded as a run of CMD: `jouet test --cmd -- cargo build` is a build, and `jouet test --cmd -- cargo test --workspace` a full test run. A command jouet does not recognise, such as `./run-tests.sh`, is recorded as nothing, as it is when run directly.

## What a run prints

An agent that runs a suite and cuts its output with `head`, `tail` or `grep` often cuts off the failure it needed, and runs the whole suite again with another filter. `jouet test` runs it once:

- **The whole output goes to a log**, stdout and stderr together, under jouet's log directory (`$JOUET_LOG_DIR`, else `~/.local/state/jouet/logs`), kept for a week. Its path is printed on every run. Read more from it with `grep` or `jouet read` rather than running the tests again.
- **A pass is one line**, with the counts the runner printed. A run of up to eight tests whose runner names each one (cargo test without `-q`, and TAP, so bats) also lists their names, as in `cmd: passed in 0.9s: 2 passed (edit::tests::editor_with_args, edit::tests::visual_fallback)`, so an agent that added a test sees it ran without reading the log. A larger run names, instead, the passing tests the change names in a line it adds since `HEAD` (a new `@test "..."`, a new `fn name()`), as in `cmd: passed in 2.2s: 19 passed, among them the changed edit command accepts an editor command with arguments`: up to eight, matched as whole words, and none when more match. Output streamed to a person, who saw the names go by, leaves them out.
- **A failure shows each failing test** by name, with the lines that explain it: the assertion or panic, the diff, the compiler error with its code frame. A long one keeps its first and last lines and the error and diff lines between. Failures are shown in the order the output has them until `--budget` bytes (default 4000; `0` for no limit), then only their names, then a count, so nothing is left out without saying so. No runner takes `--budget`, so one written after `--` is jouet's too: jouet takes it out of the command and says so on stderr.

```sh
$ jouet test --cmd -- cargo test
cmd: failed (exit 101) in 0.1s: 3 failed, 40 passed
FAIL tests::adds_negative_numbers
  checking negatives

  thread 'tests::adds_negative_numbers' (936708) panicked at src/lib.rs:9:74:
  assertion `left == right` failed: sum of negatives
    left: -5
   right: -6
FAIL tests::parses_padded_input
  thread 'tests::parses_padded_input' (936709) panicked at src/lib.rs:10:64:
  parse failed: Err("invalid digit found in string")
FAIL tests::unwraps_none
  thread 'tests::unwraps_none' (936750) panicked at src/lib.rs:11:62:
  a value was configured
full log: ~/.local/state/jouet/logs/test-1793449700778-936641-0.log
```

That digest is for agents, and for plain display (pipes, redirects, CI). A person at a terminal sees the output as it runs, then the line with the counts and the log's path. `--json` puts the counts and failures in the document (see [JSON](#json)).

### Recognised output

Failures are recognised by their lines, whatever command printed them, in the output of:

- cargo test and nextest;
- rustc, with errors by file and line (warnings are counted);
- a cargo build script that failed, with its stderr;
- `cargo fmt --check` and rustfmt, each place by file and line, with the diff;
- tsc;
- go test and go build, including a package that fails with no failed test of its own, as when `TestMain` gives up, named by the package. For `go test -json`, named tests and their output are associated by package and test; incomplete or ambiguous event sequences are not used as matching baseline details;
- pytest, unittest, RSpec, jest and vitest;
- Playwright's list reporter for `.test` and `.spec` files, including its numbered failure blocks;
- TAP, which bats prints;
- any [format you declare](#your-own-runners-formats).

One task can run several of them, as `mise run test` may run cargo's tests and then bats, and mise's `[task]` line prefixes are taken off. For any other output, a failing run shows the end of it, with earlier lines that look like errors.

Playwright's generic test timeouts are named in the digest but remain unresolved in baseline comparisons: the same timeout headline can have different causes. Vitest unhandled source type errors are named in a failed run when every reported unhandled error has a location; they are display-only. Runs reporting unhandled Vitest errors are not compared with a baseline, even if they also name failing tests. If unhandled errors cannot all be named, the digest includes their output and points to the full log, even when tests also failed.

A run that passed without running a test says so when the output shows it: TAP's `1..0`, libtest running no test in any binary, or pytest, RSpec, jest, Playwright or go saying they ran none. With `--cmd`, if the command is recognised as a test and its output shows no test ran, jouet exits `4` instead of accepting the runner's successful exit; check the filter or path. For Go, including `go test -json`, a run where every package reports `[no test files]` is empty; packages without test files do not make a mixed run empty. An unrecognised command retains its own exit code, and an empty `--affected` plan still succeeds. A Rust backtrace, std's or color-eyre's, is folded into one `... N backtrace lines skipped` line, so a panic's 40 frames do not push the message above them out of what is shown.

If a recognized test command exits zero but its output reports failed tests, jouet exits `4` and shows the failures. This can happen when a script runs several checks and returns only the last one's status. The JSON `exit_code` remains the command's actual exit code, while `ok` is false. A merge-base run with the same contradiction cannot establish a baseline pass.

The checks rule recognizes direct Playwright runs and explicit binary execution through `npx`, `bunx`, `pnpm exec`, `yarn exec`, `yarn workspace NAME exec`, and `npm exec --`. Playwright covers its own tests, not necessarily the project's unit tests, so its runs are partial; workspace runs are scoped to that workspace. Package scripts named `playwright` are not assumed to run the binary. The rule does not record interactive UI or debug modes, or `--pass-with-no-tests`: they can exit successfully without evidence that a test ran.

### Your own runners: formats

A runner jouet does not know, such as a project's own e2e script, can be described in a TOML file in a `formats/` directory: yours, `~/.config/jouet/formats/`, a trusted repository's `.jouet/formats/`, the system's, or a plugin's. Each file is one format, named after it (a plugin's after the plugin too, `acme/e2e`):

```toml
description = "The e2e runner's failed tests"
# A line that says a test failed; its `name` group, else its first, names it.
failed = ['^ERROR: E2E test (?<name>\S+) failed: exited with status code \d+$']
# A line that starts a test: a failure's lines are those after the last one.
# Without it, a failure has only the lines from an excerpt line on, or else
# the failed line itself.
start = ['^>>> E2E (\S+)$']
# Where the failure's excerpt starts: its last line that matches, else the first.
excerpt = ['^ERROR: E2E assertion failed: ']
# Lines left out of the excerpt.
drop = ['^\S+: \d+s$']
# The runner's own count of the tests it ran: its first group, else the whole
# line, is read for `3 passed`, `1 failed` and `2 skipped`.
summary = ['^E2E: ran \d+ tests \((.+)\)']

[[examples]]
output = """
>>> E2E cli/test_a
ERROR: E2E assertion failed: [mise ls] expected 'x'
cli/test_a: 1s
ERROR: E2E test cli/test_a failed: exited with status code 1
"""
failures = ["cli/test_a"]
```

With it, the failed test is named, with its assertion:

```text
cmd: failed (exit 1) in 0.0s: 1 failed
FAIL cli/test_a
  ERROR: E2E assertion failed: [mise ls] expected 'x'
```

With `summary`, a run that passed says how many tests it ran (`cmd: passed in 12.0s: 3 passed`) rather than only that it passed, so there is no need to read the log to see that the tests you meant to run did. Without it, a format counts only the tests it names as failed.

Each pattern key takes a list of regular expressions ([Rust's syntax](https://docs.rs/regex/latest/regex/#syntax)), matched against each line once colour and mise's `[task]` prefixes are taken off.

A format's examples are checked when it loads. A format does not load when its examples do not name the failures they list, when a pattern does not compile, or when it has a key jouet does not know; `jouet test` says why on stderr and runs without it. `jouet test --dry-run` lists the formats that load (`failure formats: ...`), and `--json` reports them as `formats`. Failures named by a format count for [`--baseline`](#failures-that-were-there-before-baseline) like any other.

## Partial runs and the stop gate {#the-safety-model}

An affected run is for iterating on a change, and jouet keeps it apart from a run of the whole suite. The output ends with `This was a partial run; the full suite was not verified.`, and the hooks record it as partial.

- **Full and partial runs.** With `jouet setup`'s hooks installed, the built-in `checks` rule records `jouet test` as a full test run and `jouet test --affected` as a partial one, with `--affected` as its selector. A partial pass never clears a failed full run. After a change, a passing affected run at the files as they are now is all the stop hook asks for, so check that the rest of the suite runs when the project needs it. See [finishing on a failing check](/setup#finishing-on-a-failing-check).
- **Its own result.** A pipe, as in `jouet test | tail`, hides a run's exit status from the hooks, and Codex sends hooks none at all. So `jouet test` records each run itself, for the stop gate and in the ledger. A dry run records nothing.
- **A run still going.** While it runs, `jouet test` marks itself in the session's state, and the stop gate does not hold up an agent that is waiting for it on what earlier test runs found.
- **The `affected-tests` rule** warns (it never denies) when an agent runs the whole suite again within 30 minutes of the last whole-suite run, in a repository with an adapter. It never fires after an affected run, nor after a whole-suite run whose output was cut with `head`, `tail`, `grep`, `rg` or `sed`: the agent did not see that run, and running it again whole is what `piped-test-output` asks for.

### A mise task for affected runs {#mise-tasks}

If your project runs its tests through mise, keep `test` as the full suite, and add an optional task for the affected run:

```toml
[tasks."test:affected"]
description = "Run only the tests the current changes affect (partial: run `test` before pushing)"
run = "jouet test --affected"
```

`mise run test:affected` is recorded as a partial run by its task name, as `jouet test --affected` is.

## Timings

`jouet test` times each command it runs that the `checks` rule recognises as one build, test or lint, and keeps the timing in a git note on `HEAD` under `refs/notes/jouet-cost`, where [`jouet cost`](/commands/cost) reads it. `[cost] record = false` in the config file, or `JOUET_COST_RECORD=0`, stops this. `[cost] sequences = true` (`JOUET_COST_SEQUENCES=1`) also records local, session-linked timings for successive `jouet test` checks, an experimental option described on that page.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | the tests passed, every failure also fails at the merge base, or nothing needed to run |
| `1` | the tests failed (the runner exited non-zero), the `--cmd` command exited non-zero, or the runner could not list the tests a change affects |
| `2` | a usage error: `--adapter` without `--affected`, `--since` without `--affected` or `--baseline`, `--cmd` without a command or with `--affected`, `--budget` after `--` without a number, an unknown adapter, an unknown ref or one with no merge base, `--baseline` where the merge base is `HEAD` with nothing changed, or pytest's or unittest's own usage error |
| `4` | a recognised `--cmd` test ran no tests, no adapter or test task was found, the runner or the `--cmd` program is not installed, not in a git repository, or `.jouet/test.toml` is untrusted or has changed since it was trusted |

With several adapters, every suite runs, and the exit is `1` if any failed.

## JSON

`--json` prints one document after the runs; the runners' output goes only to each suite's `log`:

```json
{
  "affected": true,
  "base": { "ref": "origin/main", "merge_base": "1a2b3c4d5e6f..." },
  "changed": 2,
  "suites": [
    {
      "adapter": "vitest",
      "precision": "vitest's module graph",
      "dir": ".",
      "decision": "affected",
      "tests": [
        { "path": "test/calc.test.js", "via": ["src/sum.js", "src/calc.js", "test/calc.test.js"] }
      ],
      "files": [
        { "path": "README.md", "status": "modified", "effect": "ignored", "reason": "documentation" },
        { "path": "src/sum.js", "status": "modified", "effect": "source" }
      ],
      "command": ["node_modules/.bin/vitest", "run", "--changed", "1a2b3c4d5e6f"],
      "exit_code": 1,
      "duration_ms": 1204,
      "log": "/home/me/.local/state/jouet/logs/test-1793449700778-936641-0.log",
      "passed": 4,
      "failed": 1,
      "failures": [
        {
          "name": "test/calc.test.js [ test/calc.test.js ]",
          "lines": [
            "Error: Cannot find module './missing.js' imported from /work/test/calc.test.js",
            " ❯ test/calc.test.js:1:1",
            "      1| import { nope } from './missing.js'",
            "       | ^",
            "      2|"
          ]
        }
      ]
    }
  ],
  "ok": false
}
```

- **Per suite:** `decision` is `affected`, `full` or `nothing`, with a `reason` for the last two. Each of `files` has an `effect` of `source`, `ignored` or `full`, with a `reason` for the last two. `via` is the chain of imports from a changed file to the test; it is absent when the runner picked a test through something jouet's own scan does not follow (a path alias, a package), and a `note` says how many. `env` lists variables jouet set for the command, such as `COVERAGE_CORE`. With `--cmd`, the one suite's `adapter` is `cmd`; for the repository's test task, it is `task`.
- **After a run**, each suite has its `log` and what jouet read in it: `passed`, `failed` and `skipped`, summed over every summary line the runner printed, and `warnings` (rustc's); `failures`, each with its `name` and `lines`, as many as fit `--budget`; `more_failures`, the names of those after; and `failures_omitted`, how many did not fit even by name.
- **For the whole run:** `dry_run` is `true` for a dry run, `formats` names the failure formats that loaded, and `baseline` holds a comparison with the merge base: its `base`, and the failures that are `pre_existing`, `new`, or `unknown` (not reached there).

A count of `0` is left out, as are empty lists.

## Adding an adapter

An adapter is a type implementing the `Adapter` trait in `src/affected/mod.rs`:

- `name` and `precision`, and `covers(ecosystem)`: which packages it can see into (a native adapter covers one language, a graph-based fallback several);
- `traces(path)`: the files it can follow to tests, and `traces_deletions()`;
- `trigger(path)`: the files whose change means its whole suite, such as its configuration;
- `full(...)` and `select(...)`: the command for the whole suite, and the tests the changed files affect with the command that runs them, delegating to the runner's own selection where it has one;
- optionally `env()` and `exit(code)`, to map the runner's exit codes onto jouet's.

Then add an entry to `ADAPTERS`: its name, a detection function of the directory's files only (so the hooks' `jouet.test.adapters()` probe can ask too), and how to load it. Native adapters come first; a fallback goes last. Add a fixture project in `test/fixtures/affected/`, selection tests in `src/affected/tests.rs`, and a bats test that runs the real tool.
