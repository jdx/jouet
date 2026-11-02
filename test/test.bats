#!/usr/bin/env bats

# `jouet test`: the full suite, or with --affected only the tests a change
# affects. The fixture projects in test/fixtures/affected run under the real
# vitest, jest and pytest, installed once into target/affected-fixtures (node
# from mise.toml, python3 from the system). Without them a test is skipped,
# except in CI, where it fails.

setup_file() {
	export DEPS="$BATS_TEST_DIRNAME/../target/affected-fixtures"
	# Each test has a home of its own, where rustup would find no toolchain.
	export REAL_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" REAL_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
	mkdir -p "$DEPS"
}

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE
	FIXTURES="$PROJECT_ROOT/test/fixtures/affected"
	init_repo
}

@test "trusted project pytest options apply to affected runs only" {
	cp -R "$FIXTURES/pytest/." .
	mkdir -p .jouet .venv/bin
	printf '[pytest]\naffected_args = ["-m", "not download"]\n' >.jouet/test.toml
	printf '#!/bin/sh\nexit 1\n' >.venv/bin/pytest
	printf '#!/bin/sh\nexit 1\n' >.venv/bin/python
	chmod +x .venv/bin/pytest .venv/bin/python
	printf '.venv/\n' >.gitignore
	git add .jouet/test.toml .gitignore pyproject.toml src tests
	git commit -qm base
	git checkout -qb work
	printf '\n# change\n' >>src/calc/ops.py

	run --separate-stderr jouet test --affected --since main --dry-run
	assert_failure 4
	assert_stderr --partial 'not trusted'

	run --separate-stderr jouet trust --yes
	assert_success
	assert_line --partial 'affected_args'
	run --separate-stderr jouet test --affected --since main --dry-run --json
	assert_success
	run jq -r '.suites[0] | [.command[], .note] | join(" ")' <<<"$output"
	assert_output --partial '-m not download'
	assert_output --partial 'restrict this affected run'

	# Testmon must collect with the same filter as the eventual pytest run.
	printf '#!/bin/sh\nexit 0\n' >.venv/bin/python
	cat >.venv/bin/pytest <<'SH'
#!/bin/sh
printf '%s\n' "$@" >"$PYTEST_ARGS_LOG"
printf 'tests/test_ops.py::test_add\n'
SH
	export PYTEST_ARGS_LOG="$BATS_TEST_TMPDIR/pytest-args"
	run --separate-stderr jouet test --affected --since main --dry-run --json
	assert_success
	run cat "$PYTEST_ARGS_LOG"
	assert_line -- '-m'
	assert_line -- 'not download'
	assert_line -- '--collect-only'

	# A filter can select nothing without proving no test depends on the change.
	printf '#!/bin/sh\nexit 5\n' >.venv/bin/pytest
	run --separate-stderr jouet test --affected --since main --dry-run --json
	assert_success
	run jq -r '.suites[0].reason' <<<"$output"
	assert_output 'no tests selected for the changed files'

	run --separate-stderr jouet test --dry-run --json
	assert_success
	run jq -r '.suites[0].command | join(" ")' <<<"$output"
	refute_output --partial 'not download'

	# A change jouet cannot safely narrow still runs the full pytest suite.
	mkdir -p .github/workflows
	printf 'name: CI\n' >.github/workflows/test.yml
	run --separate-stderr jouet test --affected --since main --dry-run --json
	assert_success
	run jq -r '.suites[0] | [.decision, (.command | join(" "))] | join(" ")' <<<"$output"
	assert_output --partial 'full '
	refute_output --partial 'not download'

	printf '\n# changed\n' >>.jouet/test.toml
	run --separate-stderr jouet test --affected --since main --dry-run
	assert_failure 4
}

# Skips the test when TOOL cannot be had, or fails it in CI, where it must be.
need() {
	if [[ -n "${CI:-}" ]]; then
		fail "$1 is needed in CI: $2"
	fi
	skip "$1: $2"
}

# Runs the rest of its arguments holding the lock for directory $1, so tests
# run in parallel install a fixture once rather than over each other and
# leave it half written. `mkdir` is the lock, which macOS has and `flock` is
# not. The holder writes its process ID in it: a lock whose holder is gone,
# such as a killed run, is taken over, and after ten minutes of waiting on a
# live one this fails rather than install beside it.
locked() {
	local lock="$1.lock" i=0 status holder
	shift
	until mkdir "$lock" 2>/dev/null; do
		holder=$(cat "$lock/pid" 2>/dev/null)
		# One waiter at a time takes over a lock whose holder is gone, and
		# reads the holder again first: another may have taken it over and
		# locked it anew already.
		if [[ -n $holder ]] && ! kill -0 "$holder" 2>/dev/null && mkdir "$lock.takeover" 2>/dev/null; then
			if [[ $(cat "$lock/pid" 2>/dev/null) == "$holder" ]]; then
				rm -rf "$lock"
			fi
			rmdir "$lock.takeover"
			continue
		fi
		((i++ < 6000)) || return 1
		sleep 0.1
	done
	echo "$$" >"$lock/pid"
	"$@"
	status=$?
	rm -rf "$lock"
	return "$status"
}

# The fixture NAME's npm dependencies, installed once at their pinned versions.
npm_deps() {
	command -v npm >/dev/null || need npm "not installed"
	locked "$DEPS/$1" npm_install "$1" || need "$1" "npm install failed"
}

npm_install() {
	local dir="$DEPS/$1"
	[[ -x "$dir/node_modules/.bin/$1" ]] && return 0
	mkdir -p "$dir"
	cp "$FIXTURES/$1/package.json" "$dir/"
	(cd "$dir" && npm install --no-audit --no-fund --silent >/dev/null 2>&1)
}

# A virtualenv NAME with pytest, plus the packages given.
venv() {
	command -v python3 >/dev/null || need python3 "not installed"
	locked "$DEPS/$1" venv_install "$@" || need pytest "installing it into a virtualenv failed"
}

venv_install() {
	local dir="$DEPS/$1"
	shift
	[[ -x "$dir/bin/pytest" ]] && return 0
	rm -rf "$dir"
	{ python3 -m venv "$dir" && "$dir/bin/pip" install -q pytest==9.2.0 "$@"; } >/dev/null 2>&1
}

# Commits fixture NAME on main as the work directory's project, with its
# dependencies linked in and ignored, and starts a branch to change it on.
project() {
	cp -R "$FIXTURES/$1/." .
	printf '.venv\nnode_modules\n.testmondata*\n.pytest_cache\n__pycache__\n' >.gitignore
	git add -A
	git commit -q -m "the $1 fixture"
	git checkout -q -b work
}

js_project() {
	npm_deps "$1"
	project "$1"
	ln -s "$DEPS/$1/node_modules" node_modules
}

# A pytest project in a virtualenv with pytest, and testmon when $1 is
# "testmon".
py_project() {
	if [[ ${1:-} == testmon ]]; then
		venv venv-testmon pytest-testmon==2.2.0
		project pytest
		ln -s "$DEPS/venv-testmon" .venv
	else
		venv venv-pytest
		project pytest
		ln -s "$DEPS/venv-pytest" .venv
	fi
}

@test "--dry-run explains a vitest selection, test by test, and runs nothing" {
	js_project vitest
	printf 'export const sum = (a, b) => b + a\n' >src/sum.js
	printf 'More words.\n' >>README.md
	base="$(git rev-parse --short=12 main)"

	run --separate-stderr jouet test --affected --dry-run

	assert_success
	assert_output "changes since main (merge base $base): 2 files
vitest (vitest's module graph): 2 test files
  test/calc.test.js: imports src/calc.js → src/sum.js (changed)
  test/sum.test.js: imports src/sum.js (changed)
  ignored: README.md (documentation)
  would run: node_modules/.bin/vitest run --changed $base"
}

@test "vitest runs only the affected tests, and fails when they fail" {
	js_project vitest
	printf "export const shout = (s) => s + '?'\n" >src/fmt.js

	run --separate-stderr jouet test --affected

	assert_failure 1
	assert_line 'vitest (vitest'"'"'s module graph): 1 test file'
	assert_line '  test/fmt.test.js: imports src/fmt.js (changed)'
	assert_line --regexp '^vitest: failed \(exit 1\) in [0-9.]+s: 1 failed(, [0-9]+ passed)?$'
	assert_line --regexp '^FAIL test/fmt.test.js > '
	assert_line --regexp '^full log: .+\.log$'
	assert_line "This was a partial run; the full suite was not verified."
	assert_output --partial '1 failed'

	printf "export const shout = (s) => s + '!'\n" >src/fmt.js
	run --separate-stderr jouet test --affected --json
	assert_success
	run jq -c '[.affected, .ok, .suites[0].decision, [.suites[0].tests[].path], .suites[0].exit_code]' <<<"$output"
	assert_output '[true,true,"affected",["test/fmt.test.js"],0]'
}

@test "jest selects through its module graph and passes extra arguments on" {
	js_project jest
	printf 'exports.sum = (a, b) => b + a\n' >src/sum.js

	run --separate-stderr jouet test --affected --json -- --ci
	assert_success
	# The runner's output went to the log, so stdout holds only the document.
	assert_equal "$stderr" ""
	log="$(jq -r '.suites[0].log' <<<"$output")"
	assert_equal "$(grep -c '^Tests:' "$log")" 1
	run jq -c '[.suites[0].adapter, .suites[0].tests, .suites[0].command[1]]' <<<"$output"
	assert_output '["jest",[{"path":"test/calc.test.js","via":["src/sum.js","src/calc.js","test/calc.test.js"]},{"path":"test/sum.test.js","via":["src/sum.js","test/sum.test.js"]}],"--ci"]'
}

@test "pytest without testmon follows imports, and a conftest runs the whole suite" {
	py_project
	printf 'def add(a, b):\n    return b + a\n' >src/calc/core/arith.py

	run --separate-stderr jouet test --affected

	assert_success
	assert_line --partial 'pytest (a static import graph (best-effort'
	assert_line '  tests/test_arith.py: imports src/calc/core/arith.py (changed)'
	assert_line '  tests/test_ops.py: imports src/calc/ops.py → src/calc/core/arith.py (changed)'
	assert_line --partial '2 passed'
	assert_line --regexp '^pytest: passed in [0-9.]+s: 2 passed \(full log: .+\.log\)$'

	printf '\n# shared\n' >>tests/conftest.py
	run --separate-stderr jouet test --affected --dry-run
	assert_success
	assert_line --partial ': the whole suite, because a conftest changed (tests/conftest.py)'
	assert_line --regexp '^  would run: .*/pytest$'
}

@test "pytest-testmon selects by coverage once it has recorded a run" {
	unset COVERAGE_CORE
	py_project testmon

	printf 'def shout(s):\n    return s + "!!"[:1]\n' >src/calc/fmt.py
	run --separate-stderr jouet test --affected
	assert_success
	assert_line --partial "pytest (pytest-testmon's coverage data): 3 test files"
	assert_line --partial 'note: pytest-testmon has no .testmondata yet'

	printf 'def add(a, b):\n    return b + a\n' >src/calc/core/arith.py
	run --separate-stderr jouet test --affected --json
	assert_success
	# coverage's default core on Python 3.14 and later loses which test ran
	# what, so jouet asks for its C tracer.
	run jq -c '[[.suites[0].tests[].path], .suites[0].command[-1], .suites[0].env]' <<<"$output"
	assert_output '[["tests/test_arith.py","tests/test_ops.py"],"--testmon",{"COVERAGE_CORE":"ctrace"}]'
}

@test "a docs-only change runs nothing, and says so" {
	py_project
	printf 'More words.\n' >>README.md
	mkdir docs && printf '# Guide\n' >docs/guide.md

	run --separate-stderr jouet test --affected

	assert_success
	assert_output --partial ': nothing to run: only documentation changed'
	refute_output --partial 'partial run'
}

@test "a lockfile change runs the whole suite and says why" {
	js_project jest
	printf '{}\n' >package-lock.json

	run --separate-stderr jouet test --affected --dry-run --json

	assert_success
	run jq -c '[.suites[0].decision, .suites[0].reason, .suites[0].command]' <<<"$output"
	assert_output '["full","a lockfile changed (package-lock.json)",["node_modules/.bin/jest"]]'
}

@test "jouet test runs the mise test task as the full suite" {
	command -v mise >/dev/null || need mise "not installed"
	printf '[tasks.test]\nrun = "echo the whole suite"\n' >mise.toml
	export MISE_TRUSTED_CONFIG_PATHS="$PWD"

	run --separate-stderr jouet test --dry-run
	assert_success
	assert_output "task: the full suite (the repository's test task)
  would run: mise run test"

	run --separate-stderr jouet test --json
	assert_success
	log="$(jq -r '.suites[0].log' <<<"$output")"
	assert_equal "$(grep -c '^the whole suite$' "$log")" 1
	run jq -c '[.affected, .suites[0].decision, .suites[0].command, .suites[0].exit_code]' <<<"$output"
	assert_output '[false,"full",["mise","run","test"],0]'
}

@test "without a test task, jouet test runs each runner's whole suite" {
	py_project

	run --separate-stderr jouet test

	assert_success
	assert_line "pytest: the full suite (the runner's own, as there is no test task)"
	assert_line --partial '3 passed'
	refute_line --partial 'partial run'
}

@test "plain unittest runs whole through discover, and an affected run follows imports" {
	command -v python3 >/dev/null || need python3 "not installed"
	project unittest

	run --separate-stderr jouet test
	assert_success
	assert_line "unittest: the full suite (the runner's own, as there is no test task)"
	# tests/ is not a package, so discovery has to start there.
	assert_line '  run: python3 -m unittest discover -s tests'
	assert_line --regexp '^unittest: passed in [0-9.]+s: 2 passed \(full log: .+\)$'

	# A different size from the original: Python's bytecode cache checks the
	# source's size and its mtime in whole seconds.
	printf 'def add(a, b):\n    return a + b + 1\n' >calc/arith.py
	run --separate-stderr jouet test --affected
	assert_failure 1
	assert_line --partial 'unittest (a static import graph (best-effort'
	assert_line '  tests/test_arith.py: imports calc/arith.py (changed)'
	refute_line --partial 'tests/test_strings.py'
	assert_line --regexp '^unittest: failed \(exit 1\) in [0-9.]+s: 1 failed$'
	assert_line --regexp '^FAIL test_add '
}

@test "usage errors and missing runners exit 2 and 4" {
	run --separate-stderr jouet test --since main
	assert_failure 2
	assert_equal "$stderr" "jouet: --since and --adapter select tests, so they need --affected
hint: jouet test --affected --since main"

	git commit -q --allow-empty -m init
	run --separate-stderr jouet test --affected
	assert_failure 4
	assert_output ""
	[[ $stderr == "jouet: no test runner jouet can select for, from here up to "* ]]

	project pytest
	run --separate-stderr jouet test --affected --adapter rspec
	assert_failure 2
}

# A finished command in this work tree, as Claude Code's hooks report it.
ran() {
	local event=PostToolUse
	[[ $2 == 0 ]] || event=PostToolUseFailure
	jq -cn --arg c "$1" --arg cwd "$PWD" --arg e "$event" --arg code "$2" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: $e, tool_name: "Bash", tool_input: {command: $c}}
		+ (if $code == "0" then {} else {error: "Exit code \($code)"} end)' |
		jouet hook claude "$([[ $2 == 0 ]] && echo post-tool-use || echo post-tool-use-failure)"
}

@test "the hooks record an affected run as partial, and a pass at the files now is enough to finish" {
	project pytest
	ran 'jouet test' 0
	printf 'def shout(s):\n    return s\n' >src/calc/fmt.py
	ran 'jouet test --affected' 0

	run --separate-stderr jq -c '.trees[].runs.test | [.command, .full, .selector]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '["jouet test --affected",false,"--affected"]'
	stop=$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}')
	run --separate-stderr jouet hook claude stop <<<"$stop"
	assert_output ""
	# A change after it, with no test run since, is asked about (not back
	# to the fixture's own text, which the full run passed at).
	printf 'def shout(s):\n    return s + "?"\n' >src/calc/fmt.py
	run --separate-stderr jouet hook claude stop <<<"$stop"
	run jq -r '.reason' <<<"$output"
	assert_output --partial "Files changed after the last test run passed (\`jouet test --affected\`)"
}

@test "running the whole suite again while iterating suggests --affected" {
	project pytest
	payload() {
		jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
	}

	# No test run yet: the first full run is not iterating.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload pytest)"
	assert_output ""

	ran 'pytest' 0
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload pytest)"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "\`jouet test --affected\` (pytest) runs only the tests your changes affect"
	assert_output --partial 'The full suite runs on CI after you push.'

	# After an affected run, a full run is the one that has to happen.
	ran 'jouet test --affected' 0
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload pytest)"
	assert_output ""
}

@test "the whole suite run again after its output was cut is not iterating" {
	project pytest
	payload() {
		jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
	}

	# The agent cut the run's output, and piped-test-output asked for the
	# whole run again: affected-tests leaves that run alone.
	ran 'pytest 2>&1 | tail -5' 0
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload 'jouet test')"
	assert_output ""

	# Once it saw a whole run, running it whole again is iterating.
	ran 'jouet test' 0
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload 'jouet test')"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "\`jouet test --affected\` (pytest) runs only the tests your changes affect"
}

# A script that prints a TAP run, as bats does: N tests, the ones in FAILING
# failing with two comment lines each, exiting 1 when any failed.
tap_script() {
	local n=$1 failing=$2
	{
		printf '#!/bin/sh\necho 1..%s\n' "$n"
		for i in $(seq 1 "$n"); do
			if [[ " $failing " == *" $i "* ]]; then
				printf 'echo "not ok %s case %s"\necho "# (in test file t.bats, line %s)"\necho "#   expected %s, got 0"\n' "$i" "$i" "$i" "$i"
			else
				printf 'echo "ok %s case %s"\n' "$i" "$i"
			fi
		done
		[[ -n $failing ]] && printf 'exit 1\n'
	} >run-tests
	chmod +x run-tests
}

@test "--cmd runs the command given, and a pass is one line with the log's path" {
	tap_script 3 ""

	run --separate-stderr jouet test --cmd -- ./run-tests

	assert_success
	assert_output --regexp '^cmd: passed in [0-9.]+s: 3 passed \(full log: .+\.log\)$'
	assert_equal "${#lines[@]}" 1
	assert_equal "$stderr" ""
	log="${output##*full log: }"
	log="${log%)}"
	assert_equal "$(cat "$log")" "$(./run-tests)"
}

@test "a failing run shows each failure up to the budget, then names the rest" {
	tap_script 6 "2 3 4 5 6"

	run --separate-stderr jouet test --cmd --budget 150 -- ./run-tests

	assert_failure 1
	assert_line --index 0 --regexp '^cmd: failed \(exit 1\) in [0-9.]+s: 5 failed, 1 passed$'
	assert_line --index 1 'FAIL case 2'
	assert_line --index 2 '  (in test file t.bats, line 2)'
	assert_line --index 3 '    expected 2, got 0'
	assert_line --index 4 'FAIL case 3'
	assert_line --index 7 '... 3 more failures: case 4, case 5, case 6'
	assert_line --index 8 --regexp '^full log: .+\.log$'

	run --separate-stderr jouet test --cmd --budget 150 --json -- ./run-tests
	assert_failure 1
	run jq -c '.suites[0] | [.adapter, .exit_code, .passed, .failed, [.failures[].name], .failures[0].lines, .more_failures, (.log | test("\\.log$"))]' <<<"$output"
	assert_output '["cmd",1,1,5,["case 2","case 3"],["(in test file t.bats, line 2)","  expected 2, got 0"],["case 4","case 5","case 6"],true]'
}

@test "output no failure is recognised in ends the failing run's digest" {
	# shellcheck disable=SC2016 # the script expands $i, not this shell
	printf '#!/bin/sh\nfor i in $(seq 1 40); do echo "step $i"; done\necho "linker died"\nexit 2\n' >build
	chmod +x build

	run --separate-stderr jouet test --cmd -- ./build

	assert_failure 1
	assert_line 'FAIL no failure recognised; the end of the output'
	assert_line '  ... 21 lines skipped'
	assert_line '  linker died'
}

@test "--cmd needs a command, cannot select, and a missing program exits 4" {
	run --separate-stderr jouet test --cmd
	assert_failure 2
	assert_equal "$stderr" "jouet: --cmd needs the command after \`--\`
hint: jouet test --cmd -- cargo test -p mycrate"

	run --separate-stderr jouet test --cmd --affected -- true
	assert_failure 2

	run --separate-stderr jouet test --cmd -- no-such-program-here
	assert_failure 4
	assert_stderr --partial 'running no-such-program-here'
}

@test "--budget after -- is refused, with the command it meant" {
	run --separate-stderr jouet test --cmd -- mise run test:e2e test_a --budget 600
	assert_failure 2
	assert_equal "$stderr" "jouet: \`--budget\` after \`--\` would go to the command, not to jouet: put it before \`--\`
hint: jouet test --budget 600 --cmd -- mise run test:e2e test_a"

	run --separate-stderr jouet test -- -k x --budget=100
	assert_failure 2
	assert_stderr --partial 'hint: jouet test --budget 100 -- -k x'
}

@test "a person at a terminal sees the output as it runs" {
	tap_script 2 ""

	run in_tty jouet test --cmd -- ./run-tests

	assert_success
	assert_output --partial 'ok 2 case 2'
	assert_output --regexp 'cmd: passed in [0-9.]+s: 2 passed \(full log: '
}

@test "the hooks steer a cut test run to --cmd, and record a --cmd build as a build" {
	payload() {
		jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
	}

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "cargo test -q -p app 2>&1 | grep -E 'panicked|FAILED' | head -20")"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "Run \`jouet test --cmd -- cargo test -q -p app\` instead of cutting its output with grep."
	assert_output --partial '[jouet rule: piped-test-output]'

	# Assignments stay in front, and what the shell would expand stays bare.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "RUST_LOG=debug cargo test --manifest-path \$DIR/Cargo.toml 'a b' | tail")"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "Run \`RUST_LOG=debug jouet test --cmd -- cargo test --manifest-path \$DIR/Cargo.toml \"a b\"\` instead"

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload 'cargo test > out.log 2>&1')"
	assert_output ""

	# The whole suite, cut: plain `jouet test` runs it.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload 'python3 -m unittest discover -v 2>&1 | tail -5')"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "Run \`jouet test\` instead of cutting its output with tail."

	ran 'jouet test --cmd -- cargo build' 1
	run --separate-stderr jq -c '.trees[].runs.build | [.command, .exit_code]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '["jouet test --cmd -- cargo build",1]'
}

@test "a test run piped into head does not count as passing, so a failure still holds up finishing" {
	project pytest
	ran 'pytest' 1
	# Six tests could have failed here: without pipefail the 0 is head's.
	ran 'jouet test --cmd -- pytest 2>&1 | head -3' 0
	run --separate-stderr jq -c '.trees[].runs.test | [.command, .exit_code]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '["pytest",1]'

	# With pipefail, the 0 is the tests' own.
	ran 'set -o pipefail; jouet test --cmd -- pytest 2>&1 | head -3' 0
	run --separate-stderr jq -c '.trees[].runs.test | [.command, .exit_code]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '["jouet test --cmd -- pytest",0]'
}

@test "--baseline passes a run whose every failure also fails at the merge base" {
	# The test file is rewritten within a second to the same size, which
	# Python's bytecode cache cannot tell from the old one.
	export PYTHONDONTWRITEBYTECODE=1
	tests() {
		mkdir -p tests
		printf 'import unittest\nclass T(unittest.TestCase):\n    def test_env(self):\n        self.fail("needs brew")\n    def test_math(self):\n        self.assertEqual(1 + 1, %s)\n' "$1" >tests/test_a.py
	}
	tests 2
	git add tests/test_a.py
	git commit -q -m base
	git checkout -q -b work
	echo change >notes.md
	git add notes.md
	git commit -q -m change

	# test_env failed before the change too: the run passes, naming it.
	run --separate-stderr jouet test --baseline
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	assert_output --partial 'test_env'

	# A failure the merge base does not have still fails the run.
	tests 3
	run --separate-stderr jouet test --baseline
	assert_failure 1
	assert_output --partial 'baseline: 1 of 2 failures is new, not failing at main'
	assert_output --partial 'test_math'
	# The merge base's worktree is gone again.
	run git worktree list
	assert_equal "${#lines[@]}" 1

	run --separate-stderr jouet test --baseline --affected
	assert_failure 2
}

@test "--baseline refuses before the run when the merge base is the work tree itself" {
	mkdir -p tests
	printf 'import unittest\nclass T(unittest.TestCase):\n    def test_env(self):\n        self.fail("needs brew")\n' >tests/test_a.py
	git add tests/test_a.py
	git commit -q -m base
	# Committed on main itself, with no upstream: the merge base with main is
	# HEAD, and a run there would be this same code.
	echo change >notes.md
	git add notes.md
	git commit -q -m change

	run --separate-stderr jouet test --baseline
	assert_failure 2
	assert_equal "$output" ""
	assert_stderr --partial "which is HEAD itself with nothing changed since"
	assert_stderr --partial "jouet test --baseline --since HEAD~1"
	run --separate-stderr jouet test --baseline --cmd -- python3 -m unittest discover -s tests -k "test_env or x"
	assert_failure 2
	assert_stderr --partial "jouet test --baseline --since HEAD~1 --cmd -- python3 -m unittest discover -s tests -k 'test_env or x'"

	# The commit before the change is a base that tells something.
	run --separate-stderr jouet test --baseline --since HEAD~1
	assert_success
	assert_output --partial "baseline: the failure also fails at HEAD~1"

	# An uncommitted change is a difference from HEAD, so HEAD is a base.
	echo more >>notes.md
	run --separate-stderr jouet test --baseline
	assert_success
	assert_output --partial "baseline: the failure also fails at main"
}

@test "the fixture lock lets one install run at a time, past a stale lock" {
	local fixture="$BATS_TEST_TMPDIR/fixture" log="$BATS_TEST_TMPDIR/held"
	# A lock a killed run left: its holder is gone.
	sh -c 'exit 0' &
	local dead=$!
	wait "$dead"
	mkdir "$fixture.lock"
	echo "$dead" >"$fixture.lock/pid"
	hold() {
		echo "start $1" >>"$log"
		sleep 0.3
		echo "end $1" >>"$log"
	}
	locked "$fixture" hold a &
	locked "$fixture" hold b &
	locked "$fixture" hold c &
	wait

	# Each install ran, and each ended before the next started.
	run awk 'NR % 2 == 1 { if ($1 != "start") bad = 1; who = $2 } NR % 2 == 0 { if ($1 != "end" || $2 != who) bad = 1 } END { exit bad || NR != 6 }' "$log"
	assert_success
	assert [ ! -e "$fixture.lock" ]
	assert [ ! -e "$fixture.lock.takeover" ]
}

@test "jouet test records its own run, so a piped one still counts" {
	mkdir -p tests
	printf 'import unittest\nclass T(unittest.TestCase):\n    def test_a(self):\n        self.fail("no")\n' >tests/test_a.py
	git add -A

	# head exits 0, and the hooks cannot see past it.
	run bash -c 'CLAUDE_CODE_SESSION_ID=s9 jouet test --cmd -- python3 -m unittest discover -s tests 2>&1 | head -1'
	assert_success
	run --separate-stderr jq -c '.trees[].runs.test | [.command, .exit_code, .full]' "$XDG_STATE_HOME/jouet/sessions/s9.json"
	assert_output '["jouet test --cmd -- python3 -m unittest discover -s tests",1,true]'
	# The ledger keeps it too, for pr land.
	run --separate-stderr jq -c 'select(.event == "test-run") | [.session, .exit_code, .decisions[0].action, .decisions[0].scope]' \
		"$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_output '["s9",1,"record:test","full"]'

	# Plain `jouet test` prints its plan first, so head exits before the run
	# ends; the run still finishes and records its result.
	run bash -c 'CLAUDE_CODE_SESSION_ID=s11 jouet test 2>&1 | head -1; echo "jouet=${PIPESTATUS[0]}"'
	assert_line 'jouet=1'
	run --separate-stderr jq -c '.trees[].runs.test | [.command, .exit_code]' "$XDG_STATE_HOME/jouet/sessions/s11.json"
	assert_output '["jouet test",1]'

	# A dry run ran nothing, so it records nothing.
	run bash -c 'CLAUDE_CODE_SESSION_ID=s10 jouet test --cmd --dry-run -- python3 -m unittest'
	assert [ ! -e "$XDG_STATE_HOME/jouet/sessions/s10.json" ]
}

@test "a passing --cmd run of the whole suite is a full run, so finishing is not held up" {
	ran 'jouet test --cmd -- python3 -m unittest' 0
	run --separate-stderr jq -c '.trees[].runs.test | [.command, .full]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '["jouet test --cmd -- python3 -m unittest",true]'

	# A command the rule does not know records nothing, as when run directly.
	ran 'jouet test --cmd -- ./run-tests.sh' 0
	run --separate-stderr jq -c '.trees[].runs.test.command' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '"jouet test --cmd -- python3 -m unittest"'
}

@test "a declared format names a runner's failures, and one that cannot work is said" {
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	mkdir -p "$XDG_CONFIG_HOME/jouet/formats"
	cat >"$XDG_CONFIG_HOME/jouet/formats/e2e.toml" <<'TOML'
failed = ['^ERROR: E2E test (?<name>\S+) failed']
start = ['^>>> E2E (\S+)$']
excerpt = ['^ERROR: E2E assertion failed: ']
TOML
	printf 'failed = ["^FAIL"]\n' >"$XDG_CONFIG_HOME/jouet/formats/broken.toml"
	printf '>>> E2E cli/test_a\nnoise\nERROR: E2E assertion failed: expected x\nERROR: E2E test cli/test_a failed\n' >out.txt
	git init -q repo
	cd repo

	run --separate-stderr jouet test --dry-run --cmd -- true
	assert_success
	assert_line 'failure formats: e2e'
	assert_stderr --partial 'broken.toml'
	assert_stderr --partial 'needs a group that names the test'

	run --separate-stderr jouet test --cmd -- sh -c 'cat ../out.txt; exit 1'
	assert_failure 1
	assert_line 'FAIL cli/test_a'
	assert_line '  ERROR: E2E assertion failed: expected x'
	refute_line '  noise'

	# A repository's formats need trusting first.
	rm "$XDG_CONFIG_HOME/jouet/formats/e2e.toml"
	mkdir -p .jouet/formats
	printf "failed = ['^ERROR: E2E test (?<name>\\\\S+) failed']\n" >.jouet/formats/e2e.toml
	run --separate-stderr jouet test --cmd -- sh -c 'cat ../out.txt; exit 1'
	refute_line 'FAIL cli/test_a'
}

@test "--baseline does not call failures new when the merge base's run names none, or reaches far fewer tests" {
	# At the merge base the runner dies before any test, as when a tool it
	# needs is missing from that checkout.
	export RETRY_MARKER="$BATS_TEST_TMPDIR/runner-ready"
	cat >run.sh <<'SCRIPT'
if [ -e "$RETRY_MARKER" ]; then
    echo 1..3
    echo "ok 1 a"
    echo "ok 2 b"
    echo "not ok 3 keychain"
else
    echo "mise ERROR No version is set for shim: fnox"
fi
exit 1
SCRIPT
	git add run.sh
	git commit -q -m base
	git checkout -q -b work
	printf 'echo 1..3\necho "ok 1 a"\necho "ok 2 b"\necho "not ok 3 keychain"\nexit 1\n' >run.sh
	git commit -q -am change

	run --separate-stderr jouet test --baseline --cmd -- sh run.sh
	assert_failure 1
	assert_output --partial 'baseline: the run at main failed (exit 1) without naming a failing test, so the failures could not be compared'
	refute_output --partial 'new'

	# Once the environment is fixed, an explicit retry replaces the cached
	# inconclusive result and can compare the same failures.
	touch "$RETRY_MARKER"
	run --separate-stderr jouet test --baseline --cmd -- sh run.sh
	assert_success
	assert_output --partial 'the failure also fails at main'

	# At the merge base one test ran, against three here: the failure may
	# never have run there.
	git checkout -q main
	printf 'echo 1..1\necho "not ok 1 other"\nexit 1\n' >run.sh
	git commit -q -am "base 2"
	git checkout -q work
	git merge -q -X ours main -m merge
	run --separate-stderr jouet test --baseline --cmd -- sh run.sh
	assert_failure 1
	assert_output --partial 'ran far fewer tests, so the failure could not be compared: keychain'
}

@test "cargo picks the changed crates and every crate that depends on them" {
	command -v cargo >/dev/null || need cargo "not installed"
	export RUSTUP_HOME="$REAL_RUSTUP_HOME" CARGO_HOME="$REAL_CARGO_HOME"
	printf '[workspace]\nmembers = ["crates/*"]\nresolver = "2"\n' >Cargo.toml
	for c in base mid top lone; do
		mkdir -p "crates/$c/src"
		printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2021"\n' "$c" >"crates/$c/Cargo.toml"
		printf 'pub fn f() {}\n' >"crates/$c/src/lib.rs"
	done
	printf '\n[dependencies]\nbase = { path = "../base" }\n' >>crates/mid/Cargo.toml
	printf '\n[dev-dependencies]\nmid = { path = "../mid" }\n' >>crates/top/Cargo.toml
	git add -A
	git commit -q -m init

	printf 'pub fn f() { let _ = 1; }\n' >crates/base/src/lib.rs
	run --separate-stderr jouet test --affected --dry-run
	assert_success
	assert_line "cargo (the workspace's crates: each changed crate and every crate that depends on it): 3 crates"
	assert_line '  base: changed'
	assert_line '  mid: depends on base (changed)'
	assert_line '  top: depends on mid → base (changed)'
	refute_line --partial 'lone'
	assert_line '  would run: cargo test -p base -p mid -p top'

	# The workspace manifest is every crate's.
	git checkout -q -- crates
	printf '# a comment\n' >>Cargo.toml
	run --separate-stderr jouet test --affected --dry-run
	assert_success
	assert_line --partial 'cargo test --workspace'
}

@test "a failing full suite is compared with the merge base on its own, once per merge base" {
	mkdir -p tests
	# Each run of the test, here or at the merge base, leaves a line.
	printf 'import unittest\nclass T(unittest.TestCase):\n    def test_env(self):\n        open("%s/runs", "a").write("x\\n")\n        self.fail("needs brew")\n' "$BATS_TEST_TMPDIR" >tests/test_a.py
	git add tests/test_a.py
	git commit -q -m base
	git checkout -q -b work
	echo change >notes.md
	git add notes.md
	git commit -q -m change

	# Without --baseline: the suite fails, runs at main, and passes.
	run --separate-stderr jouet test
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 2

	# Kept: the next failure compares with it, and main does not run again.
	echo more >>notes.md
	run --separate-stderr jouet test
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 3

	# The suite's own command, run through --cmd, shares what is kept.
	run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests
	assert_success
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 4
	# Another command has nothing kept: --cmd does not run main on its own,
	# and --baseline keeps what it finds for the next run.
	run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests -p 'test_*.py'
	assert_failure 1
	refute_output --partial 'baseline:'
	run --separate-stderr jouet test --baseline --cmd -- python3 -m unittest discover -s tests -p 'test_*.py'
	assert_success
	run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests -p 'test_*.py'
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'

	# Off, a failing run stays failed.
	JOUET_TEST_BASELINE=off run --separate-stderr jouet test
	assert_failure 1
	refute_output --partial 'baseline:'
}

@test "a program not on PATH names its versioned sibling" {
	mkdir -p bin
	printf '#!/bin/sh\nexit 0\n' >bin/frob3
	printf '#!/bin/sh\nexit 0\n' >bin/frob3.12
	chmod +x bin/frob3 bin/frob3.12
	PATH="$PWD/bin:$PATH" run jouet test --cmd -- frob -V
	assert_failure
	assert_output --partial 'running frob: No such file or directory'
	assert_output --regexp '.frob3. is on PATH'
}
