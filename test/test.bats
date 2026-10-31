#!/usr/bin/env bats

# `jouet test`: the full suite, or with --affected only the tests a change
# affects. The fixture projects in test/fixtures/affected run under the real
# vitest, jest and pytest, installed once into target/affected-fixtures (node
# from mise.toml, python3 from the system). Without them a test is skipped,
# except in CI, where it fails.

setup_file() {
	export DEPS="$BATS_TEST_DIRNAME/../target/affected-fixtures"
	mkdir -p "$DEPS"
}

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE
	FIXTURES="$PROJECT_ROOT/test/fixtures/affected"
	init_repo
}

# Skips the test when TOOL cannot be had, or fails it in CI, where it must be.
need() {
	if [[ -n "${CI:-}" ]]; then
		fail "$1 is needed in CI: $2"
	fi
	skip "$1: $2"
}

# The fixture NAME's npm dependencies, installed once at their pinned versions.
npm_deps() {
	command -v npm >/dev/null || need npm "not installed"
	local dir="$DEPS/$1"
	if [[ ! -x "$dir/node_modules/.bin/$1" ]]; then
		mkdir -p "$dir"
		cp "$FIXTURES/$1/package.json" "$dir/"
		(cd "$dir" && npm install --no-audit --no-fund --silent >/dev/null 2>&1) ||
			need "$1" "npm install failed"
	fi
}

# A virtualenv NAME with pytest, plus the packages given.
venv() {
	local dir="$DEPS/$1"
	shift
	if [[ ! -x "$dir/bin/pytest" ]]; then
		command -v python3 >/dev/null || need python3 "not installed"
		rm -rf "$dir"
		{ python3 -m venv "$dir" && "$dir/bin/pip" install -q pytest==9.2.0 "$@"; } >/dev/null 2>&1 ||
			need pytest "installing it into a virtualenv failed"
	fi
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
	assert_line --regexp '^vitest: failed \(exit 1\) in [0-9.]+s$'
	assert_line "This was a partial run. Run the full suite (\`jouet test\`) before you finish or push."
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
	# The runner's output went to stderr, so stdout holds only the document.
	assert_equal "$(grep -c '^Tests:' <<<"$stderr")" 1
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
	assert_line --regexp '^pytest: passed in [0-9.]+s$'

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
	assert_equal "$(grep -c '^the whole suite$' <<<"$stderr")" 1
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
	assert_stderr --partial 'Ran 2 tests'

	printf 'def add(a, b):\n    return a - b\n' >calc/arith.py
	run --separate-stderr jouet test --affected
	assert_failure 1
	assert_line --partial 'unittest (a static import graph (best-effort'
	assert_line '  tests/test_arith.py: imports calc/arith.py (changed)'
	refute_line --partial 'tests/test_strings.py'
	assert_stderr --partial 'Ran 1 test'
	assert_line --regexp '^unittest: failed \(exit 1\) in [0-9.]+s$'
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

@test "the hooks record an affected run as partial, so the full suite is still asked for" {
	project pytest
	ran 'jouet test' 0
	printf 'def shout(s):\n    return s\n' >src/calc/fmt.py
	ran 'jouet test --affected' 0

	run --separate-stderr jq -c '.runs.test | [.command, .full, .selector]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '["jouet test --affected",false,"--affected"]'
	stop=$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}')
	run --separate-stderr jouet hook claude stop <<<"$stop"
	run jq -r '.reason' <<<"$output"
	assert_output --partial "The tree has changed since the last full test run (\`jouet test\`)"
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
	assert_output --partial 'it still has to pass before you finish or push'

	# After an affected run, a full run is the one that has to happen.
	ran 'jouet test --affected' 0
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload pytest)"
	assert_output ""
}
