---
description: Run the full test suite, or only the tests a change affects, chosen by each ecosystem's own tool and explained test by test.
---

# jouet test

`jouet test --affected` runs only the tests your changes affect, so an agent iterating on a change waits seconds instead of minutes. It is one command for every ecosystem: adapters hand the selection to the test runner's own machinery (vitest's and jest's module graphs, pytest-testmon's coverage data), and jouet adds the same things everywhere: the changed set, the cases where only the whole suite is safe, an explanation of each pick, and a record that the run was partial.

`jouet test` without `--affected` runs the full suite.

```text
jouet test [--affected] [--since REF] [--adapter NAME] [--dry-run] [-- ARGS...]
```

```sh
$ jouet test --affected
changes since origin/main (merge base 1a2b3c4d5e6f): 2 files
vitest (vitest's module graph): 2 test files
  test/calc.test.js: imports src/calc.js → src/sum.js (changed)
  test/sum.test.js: imports src/sum.js (changed)
  ignored: README.md (documentation)
  run: node_modules/.bin/vitest run --changed 1a2b3c4d5e6f
...vitest's own output...
vitest: passed in 1.2s
This was a partial run. Run the full suite (`jouet test`) before you finish or push.
```

`--dry-run` prints the same explanation and runs nothing. The text lists the first 20 tests with their reasons and counts the rest; `--json` lists them all. Words after `--` go to the test runner. The flags are listed in the [CLI reference](/cli/test).

## The safety model

Affected runs speed up iteration. They never replace the full suite:

- **Recorded as partial.** With `jouet setup`'s hooks installed, the built-in `checks` rule records `jouet test` as a full test run and `jouet test --affected` as a partial one, with `--affected` as its selector. A partial pass never clears a failed full run, the stop hook asks for the full suite when only partial runs followed a change, and the `push-untested` rule and `jouet pr land` want a full pass at the exact tree. See [finishing on a failing check](/setup#finishing-on-a-failing-check).
- **The whole suite when in doubt.** A change jouet cannot trace to tests runs everything, and says why.
- **Nothing for docs.** A change to documentation alone runs no tests, and says so.
- **Steering, not rewriting.** The `affected-tests` rule warns (it never denies) when an agent runs the whole suite again a few minutes after the last whole-suite run, in a repository with an adapter. It never fires after an affected run, because the next full run is the one that has to happen.

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

### Planned

These slot into the same adapter trait and are follow-ups:

- **go:** `go list -deps` gives the package graph, on top of Go's own test cache.
- **cargo:** narrowing what is built (`-p`) rather than which tests run, with `determinator`. In Rust, compiling dominates, so selecting tests saves little.
- **Ruby:** best-effort, from coverage maps in the style of crystalball, or file naming conventions.

## The full suite

`jouet test` without `--affected` runs the repository's test task, found at the repository root as `jouet run`'s recipes find it: a mise `test` task (`mise run test`), a package.json `test` script (with the package manager its lockfile names), `Cargo.toml` (`cargo test --workspace`), a justfile or Makefile `test` target, or `go.mod` (`go test ./...`). Without one, it runs the whole suite of each adapter found. Words after `--` are passed on (after a `--` of its own for `npm test`).

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
| `1` | the tests failed (the runner exited non-zero) |
| `2` | a usage error: `--since` or `--adapter` without `--affected`, an unknown adapter or ref, or pytest's own usage error |
| `4` | no adapter or test task was found, the runner is not installed, or not in a git repository |

With several adapters, every suite runs, and the exit is `1` if any failed.

## JSON

`--json` prints one document after the runs, and sends the runners' output to stderr:

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
      "exit_code": 0,
      "duration_ms": 1204
    }
  ],
  "ok": true
}
```

`decision` is `affected`, `full` or `nothing`, with a `reason` for the last two. `via` is the chain of imports from a changed file to the test; it is absent when the runner picked a test through something jouet's own scan does not follow (a path alias, a package), and a `note` says how many. `env` lists variables jouet set for the command, such as `COVERAGE_CORE`.

## Adding an adapter

An adapter is a type implementing the `Adapter` trait in `src/affected/mod.rs`:

- `name`, `precision` and `ecosystem` (which packages it can see into);
- `traces(path)`: the files it can follow to tests, and `traces_deletions()`;
- `trigger(path)`: the files whose change means its whole suite, such as its configuration;
- `full(...)` and `select(...)`: the command for the whole suite, and the tests the changed files affect with the command that runs them, delegating to the runner's own selection where it has one;
- optionally `env()` and `exit(code)`, to map the runner's exit codes onto jouet's.

Detection is a function of the directory's files only, so the hooks' `jouet.test.adapters()` probe can ask too. Add the name to `NAMES`, `detect` and `load`, a fixture project in `test/fixtures/affected/`, selection tests in `src/affected/tests.rs`, and a bats test that runs the real tool.
