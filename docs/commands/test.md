---
description: Run the full test suite, only the tests a change affects, or any test or build command, and get each failure with its error and a log of the whole output.
---

# jouet test

`jouet test --affected` runs only the tests your changes affect, so an agent iterating on a change waits seconds instead of minutes. It is one command for every ecosystem: adapters hand the selection to the test runner's own machinery (vitest's and jest's module graphs, pytest-testmon's coverage data), or to jouet's code graph where there is none, and jouet adds the same things everywhere: the changed set, the cases where only the whole suite is safe, an explanation of each pick, and a record that the run was partial.

`jouet test` without `--affected` runs the full suite, and `jouet test --cmd -- CMD` runs any other test, e2e or build command. Every run keeps its whole output in a log and prints, for an agent, [only what it needs](#what-a-run-prints): one line when it passes, each failure with its error when it fails.

```text
jouet test [--affected] [--since REF] [--adapter NAME] [--dry-run] [--budget BYTES] [-- ARGS...]
jouet test --cmd [--budget BYTES] -- CMD...
```

```sh
$ jouet test --affected
changes since origin/main (merge base 1a2b3c4d5e6f): 2 files
vitest (vitest's module graph): 2 test files
  test/calc.test.js: imports src/calc.js → src/sum.js (changed)
  test/sum.test.js: imports src/sum.js (changed)
  ignored: README.md (documentation)
  run: node_modules/.bin/vitest run --changed 1a2b3c4d5e6f
vitest: passed in 1.2s: 5 passed (full log: ~/.local/state/jouet/logs/test-1793449700864-936641-0.log)
This was a partial run. Run the full suite (`jouet test`) before you finish or push.
```

`--dry-run` prints the same explanation and runs nothing. The text lists the first 20 tests with their reasons and counts the rest; `--json` lists them all. Words after `--` go to the test runner. The flags are listed in the [CLI reference](/cli/test).

## What a run prints

An agent that runs a suite and cuts its output with `head`, `tail` or `grep` often cuts off the failure it needed, and runs the whole suite again with another filter. `jouet test` runs it once:

- **The whole output goes to a log**, stdout and stderr together, under jouet's log directory (`$JOUET_LOG_DIR`, else `~/.local/state/jouet/logs`), kept for a week. Its path is printed on every run: read more from it with `grep` or `jouet read` rather than running the tests again.
- **A pass is one line**, with the counts the runner printed.
- **A failure shows each failing test** by name, with the lines that explain it: the assertion or panic, the diff, the compiler error with its code frame. A long one keeps its first and last lines and the error and diff lines between. Failures are shown in the order the output has them until `--budget` bytes (default 4000; `0` for no limit), then only their names, then a count, so nothing is left out without saying so.

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

Failures are recognised, by their lines rather than by the command, in the output of cargo test and nextest, rustc (errors by file and line; warnings are counted), tsc, go test and go build, pytest, unittest, jest, vitest, TAP, which bats prints, and mise's e2e runner (`E2E test NAME failed`, with the failed assertion). One task can run several of them, as `mise run test` may run cargo's tests and then bats, and mise's `[task]` line prefixes are taken off. For any other output, a failing run shows the end of it, with earlier lines that look like errors.

This is for agents, and for plain display (pipes, redirects, CI). A person at a terminal sees the output as it runs, then the line with the counts and the log's path. `--json` puts the counts and failures in the document (see [JSON](#json)).

## Any command: `--cmd`

`jouet test --cmd -- CMD` runs CMD, directly rather than through a shell, in the current directory, with the same log and digest: one test, an e2e suite, a build.

```sh
$ jouet test --cmd -- cargo build
cmd: failed (exit 101) in 0.1s
FAIL src/lib.rs:1:45: error[E0425]: cannot find value `undefined_var` in this scope
    |
  1 | pub fn add(a: i32, b: i32) -> i32 { a + b + undefined_var }
    |                                             ^^^^^^^^^^^^^ not found in this scope
full log: ~/.local/state/jouet/logs/test-1793449979466-951516-0.log
```

The built-in `piped-test-output` rule warns (it never denies) when an agent pipes a test, build or e2e run into `head`, `tail`, `grep`, `rg` or `sed -n`, or writes its output to a file and greps that file in the same call, and suggests the `jouet test --cmd` command for it, or plain `jouet test` when the command is the whole suite (`cargo test`, `python3 -m unittest`, `pytest`, `go test ./...`, `npm test`, `mise run test`, ...). It leaves lints to the `scoped-lint` rule, and stays quiet for `--help` and `--list`, and for output written to a file the call does not read back. With `jouet setup`'s hooks, `jouet test --cmd -- CMD` is recorded as a run of CMD: `jouet test --cmd -- cargo build` is a build, and `jouet test --cmd -- cargo test --workspace` a full test run. A command jouet does not recognise, such as `./run-tests.sh`, is recorded as nothing, as it is when run directly.

## The safety model

Affected runs speed up iteration. They never replace the full suite:

- **Recorded as partial.** With `jouet setup`'s hooks installed, the built-in `checks` rule records `jouet test` as a full test run and `jouet test --affected` as a partial one, with `--affected` as its selector. A partial pass never clears a failed full run, the stop hook asks for the full suite when only partial runs followed a change, and the `push-untested` rule and `jouet pr land` want a full pass at the exact tree. See [finishing on a failing check](/setup#finishing-on-a-failing-check).
- **The whole suite when in doubt.** A change jouet cannot trace to tests runs everything, and says why.
- **Nothing for docs.** A change to documentation alone runs no tests, and says so.
- **Steering, not rewriting.** The `affected-tests` rule warns (it never denies) when an agent runs the whole suite again a few minutes after the last whole-suite run, in a repository with an adapter. It never fires after an affected run, because the next full run is the one that has to happen, nor after a whole-suite run whose output was cut with `head`, `tail`, `grep`, `rg` or `sed`: the agent did not see that run, and running it again whole is what `piped-test-output` asks for.

## The changed set

The changes are everything that differs from the merge base of `HEAD` and `--since` (by default the branch's upstream, else the remote's default branch, else a local `main` or `master`), committed or not, plus untracked files git does not ignore. A rename counts as a deletion and an addition.

With an upstream that is the branch's own remote branch, the changed set is what you have not pushed yet. The full suite passed before the last push, because jouet's hooks ask for that, so the affected run covers what changed since.

## What each changed file means

Each changed file is classified, for each adapter, in this order:

| Change | Effect |
|---|---|
| Documentation: `*.md`, `*.rst`, `*.txt` (not `requirements*.txt`), `LICENSE`, `CHANGELOG`, `.gitignore`, `.github/ISSUE_TEMPLATE/` and the like | ignored |
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

jouet looks for adapters in the current directory, then each directory above it up to the repository root, and uses the first directory that has any. Every adapter found there runs, one after the other (a directory with both `package.json` and `pyproject.toml` runs vitest and pytest), unless `--adapter` picks one. To test one package of a monorepo, run it from that package's directory.

## Adapters

### vitest

- **Found by** a `vitest.config.*` or `vitest.workspace.*` file, or `vitest` in `package.json`'s dependencies. Scripts are not read: renovate, for one, has a script named `jest` that runs vitest.
- **Selects with** `vitest list --changed <merge base> --filesOnly`, and runs `vitest run --changed <merge base>`: vitest's module graph, which follows every import vitest resolves, aliases and workspace packages included.
- **Traces** `.js`, `.jsx`, `.ts`, `.tsx`, `.mjs`, `.cjs`, `.mts`, `.cts`, `.vue`, `.svelte`, `.json` and style files.
- **The whole suite for** `package.json`, `vitest.config.*`, `vitest.workspace.*`, `vite.config.*`, `tsconfig*.json`, Babel, SWC and PostCSS configuration, `.env*` files, setup files (`setup*`, `*.setup.*`, `global-setup`) and `__mocks__/`.
- **Limits:** a test that reads a file at run time without importing it, such as a fixture loaded with `fs.readFileSync`, is not in the module graph. A deleted file runs the whole suite, because the graph cannot follow a file that is gone.
- **Runner:** `node_modules/.bin/vitest` in the project or a directory above it. Yarn Plug'n'Play installs, which have no `node_modules`, are not supported yet.

### jest

- **Found by** a `jest.config.*` file, a `jest` key in `package.json`, or `jest` in its dependencies.
- **Selects with** `jest --listTests --findRelatedTests <changed files>`, and runs `jest --findRelatedTests <changed files>`: jest's dependency graph (its haste map).
- **Traces** the same files as vitest.
- **The whole suite for** `package.json`, `jest.config.*`, `jest.preset.*`, `tsconfig*.json`, Babel and SWC configuration, `.env*` files, setup files and `__mocks__/`.
- **Limits:** as for vitest.

### pytest

- **Found by** `pytest.ini` or `conftest.py`, `[tool:pytest]` in `setup.cfg`, `[pytest]` in `tox.ini`, or `pytest` in `pyproject.toml` or a `requirements` file.
- **Runner:** `pytest` from a `.venv` or `venv` in the project or the repository root, the active virtualenv, `PATH`, or `python3 -m pytest`.
- **With pytest-testmon installed** (`import testmon` works in pytest's Python), it runs `pytest --testmon`, which selects by the coverage each test recorded on earlier runs. That is the most precise selection jouet offers: it sees calls, not only imports. testmon compares with its own last run, kept in `.testmondata`, rather than with the merge base; with no `.testmondata` yet, the first run runs everything once to record coverage. jouet sets `COVERAGE_CORE=ctrace` for testmon, unless you set `COVERAGE_CORE` yourself, because coverage's default core from Python 3.14 records no per-test contexts and testmon then misses tests that reach a change through another module.
- **Without testmon**, jouet builds a static import graph, labelled best-effort: it reads every `import` and `from ... import` in the repository's Python files (inside functions and `if TYPE_CHECKING:` too), resolves them against the project, its `src/`, pytest's `pythonpath` and each test's own directory, and runs the test files (by pytest's `python_files`, `test_*.py` and `*_test.py` by default) that import a changed module, directly or through other modules. Importing `a.b.c` also depends on `a/__init__.py` and `a/b/__init__.py`. A deleted module selects the tests that still import it.
- **Traces** `.py` and `.pyi`.
- **The whole suite for** any `conftest.py`, `pyproject.toml`, `setup.py`, `setup.cfg`, `pytest.ini`, `tox.ini`, `noxfile.py` and `.coveragerc`.
- **Limits of the import graph:** dynamic imports (`importlib.import_module`, `__import__`), plugins and entry points, and data files are invisible to it. Over-selection is possible: a module name found under two roots selects the tests of both.

### unittest

- **Found by** a test file (`test*.py`) in the directory, its `tests/` or its `test/` that imports `unittest`, where pytest is not found: a pytest project runs its unittest tests through pytest.
- **Runner:** `python -m unittest` with the Python of a `.venv` or `venv` in the project or the repository root, or `python3`. The full suite is `python3 -m unittest discover`, with `-s tests` when `tests/` is not a package, since discovery from the project would not enter it.
- **Selection:** the same best-effort static import graph as pytest's without testmon, with unittest's `test*.py` as the test files; it runs `python3 -m unittest` on the selected files.
- **Traces** `.py` and `.pyi`.
- **The whole suite for** `pyproject.toml`, `setup.py` and `setup.cfg`.

### graph

The fallback for languages without a native adapter, built on jouet's [code graph](/commands/graph).

- **Found by** `go.mod` (Go), a `Gemfile` (Ruby), `pom.xml` or `build.gradle` (Java). It runs only where no native adapter covers the language.
- **Selection:** the graph is brought up to date, then followed backwards from each changed file, through imports, calls, references and inheritance, to the test files that reach it, by codegraph's test conventions (`*_test.go`, `spec/` and `*_spec.rb`, `test/`, `*Test.java` and the like). Each pick shows the chain of files that reaches it.
- **Runner:** `go test` on the selected tests' packages; `bundle exec rspec` on their files (with `.rspec` or a `spec/` directory), else minitest through `bundle exec ruby -Itest -Ilib`; `mvn test -Dtest=…` or `gradle test --tests …` on their classes, through `mvnw` or `gradlew` when the project has one.
- **The whole suite for** `go.mod`, the `Gemfile`, `.rspec`, a spec or test helper, the Maven or Gradle build, test data, a deleted file (the graph cannot follow a file that is gone), and a file the graph could not index.

### Planned

These slot into the same adapter trait and are follow-ups:

- **go:** `go list -deps` gives the package graph, on top of Go's own test cache.
- **cargo:** narrowing what is built (`-p`) rather than which tests run, with `determinator`. In Rust, compiling dominates, so selecting tests saves little.
- **Ruby:** best-effort, from coverage maps in the style of crystalball, or file naming conventions.

## The full suite

`jouet test` without `--affected` runs the repository's test task, found at the repository root: a mise `test` task (`mise run test`), which jouet asks mise about (`mise tasks info test`), so includes, globs and file tasks count as mise reads them, and reads from the config files when mise cannot answer; a package.json `test` script (with the package manager its lockfile names, and not `npm init`'s placeholder that only fails); `Cargo.toml` (`cargo test --workspace`), a justfile or Makefile `test` target, or `go.mod` (`go test ./...`). Without one, it runs the whole suite of each adapter found. Words after `--` are passed on (after a `--` of its own for `npm test`).

### Failures that were there before: `--baseline`

A full suite can fail for reasons the change has nothing to do with: a test that needs a tool this machine lacks, or one already broken on the default branch. Fixing the environment to make it pass costs far more than the change did. `jouet test --baseline` settles it instead:

```console
$ jouet test --baseline
unittest: failed (exit 1) in 0.1s: 1 failed, 1 passed
FAIL test_env (test_a.T.test_env)
  …
baseline: the failure also fails at main (302afa77cd40), so none is from this change; counted as passing: test_env (test_a.T.test_env)
```

When the run fails, it runs each failed suite again in a temporary worktree of the merge base with the upstream or default branch (`--since REF` names another), with any path into the checkout in the command pointed at the worktree, and compares the failures by name. If every one also fails there, the run passes (exit 0), and it names them; the stop gate and `push-untested` count it as a passing full run. A failure the merge base does not have still fails it (exit 1), listed as new. The worktree is removed afterwards. It needs a runner whose failures jouet recognises by name, costs one more run of the failed suites, and cannot be combined with `--affected`, whose tests depend on the changes.

### mise tasks

If your project runs its tests through mise, keep `test` as the full suite, and add an optional task for the affected run:

```toml
[tasks."test:affected"]
description = "Run only the tests the current changes affect (partial: run `test` before pushing)"
run = "jouet test --affected"
```

`mise run test:affected` is recorded as a partial run by its task name, just as `jouet test --affected` is.

## Exit codes

| Code | Meaning |
|---|---|
| `0` | the tests passed, or nothing needed to run |
| `1` | the tests failed (the runner exited non-zero), or the `--cmd` command exited non-zero |
| `2` | a usage error: `--since` or `--adapter` without `--affected`, `--cmd` without a command or with `--affected`, an unknown adapter or ref, or pytest's or unittest's own usage error |
| `4` | no adapter or test task was found, the runner or the `--cmd` program is not installed, or not in a git repository |

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

After a run, each suite has its `log` and what jouet read in it: `passed`, `failed` and `skipped`, summed over every summary line the runner printed, and `warnings` (rustc's); `failures`, each with its `name` and `lines`, as many as fit `--budget`; `more_failures`, the names of those after; and `failures_omitted`, how many did not fit even by name. A count of `0` is left out, as are empty lists. With `--cmd`, the one suite's `adapter` is `cmd`.

`decision` is `affected`, `full` or `nothing`, with a `reason` for the last two. `via` is the chain of imports from a changed file to the test; it is absent when the runner picked a test through something jouet's own scan does not follow (a path alias, a package), and a `note` says how many. `env` lists variables jouet set for the command, such as `COVERAGE_CORE`.

## Adding an adapter

An adapter is a type implementing the `Adapter` trait in `src/affected/mod.rs`:

- `name` and `precision`, and `covers(ecosystem)`: which packages it can see into (a native adapter covers one language, a graph-based fallback several);
- `traces(path)`: the files it can follow to tests, and `traces_deletions()`;
- `trigger(path)`: the files whose change means its whole suite, such as its configuration;
- `full(...)` and `select(...)`: the command for the whole suite, and the tests the changed files affect with the command that runs them, delegating to the runner's own selection where it has one;
- optionally `env()` and `exit(code)`, to map the runner's exit codes onto jouet's.

Then add an entry to `ADAPTERS`: its name, a detection function of the directory's files only (so the hooks' `jouet.test.adapters()` probe can ask too), and how to load it. Native adapters come first; a fallback goes last. Add a fixture project in `test/fixtures/affected/`, selection tests in `src/affected/tests.rs`, and a bats test that runs the real tool.
