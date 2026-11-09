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

enable_result_reuse() {
	export JOUET_TEST_REUSE_S=3600
}

@test "baseline uses a local submodule for the pinned commit without changing its remote" {
	local source="$BATS_TEST_TMPDIR/source repo #1"
	local parent_origin="$BATS_TEST_TMPDIR/parent-origin"
	git remote add origin "$parent_origin"
	git init -q "$source"
	printf 'old\n' >"$source/value"
	git -C "$source" add value
	git -C "$source" commit -qm old
	local old_oid
	old_oid="$(git -C "$source" rev-parse HEAD)"
	git -c protocol.file.allow=always submodule add -q -- "$source" 'deps/local #1'
	printf '%s\n' "$source" >source-url
	printf 'old\n' >expected
	cat >check-submodule <<'SH'
#!/bin/sh
echo '1..1'
remote="$(git -C 'deps/local #1' remote get-url origin)"
expected_remote="$(cat source-url)"
if test "$(cat 'deps/local #1/value')" = "$(cat expected)" &&
   test ! -f "$(git -C 'deps/local #1' rev-parse --path-format=absolute --git-path objects/info/alternates)" &&
   { test "$remote" = "$expected_remote" || test "$remote" = "file://$expected_remote"; }; then
  echo 'not ok 1 expected failure'
  echo '# same failure details'
else
  echo 'not ok 1 wrong submodule'
  echo '# missing commit or wrong remote'
fi
exit 1
SH
	chmod +x check-submodule
	git add .gitmodules 'deps/local #1' source-url expected check-submodule
	git commit -qm base
	git checkout -qb work

	printf 'new\n' >"$source/value"
	git -C "$source" add value
	git -C "$source" commit -qm new
	git -C 'deps/local #1' fetch -q origin
	git -C 'deps/local #1' checkout -q FETCH_HEAD
	printf 'new\n' >expected
	git add 'deps/local #1' expected
	git commit -qm change
	mv "$source" "${source}.unavailable"

	# The base's older gitlink is present locally, but its remote is gone.
	GIT_ALLOW_PROTOCOL="file" run --separate-stderr jouet test --baseline --cmd -- ./check-submodule
	assert_success
	assert_output --partial 'baseline: the failure also fails'
	assert_equal "$(git -C 'deps/local #1' remote get-url origin)" "$source"
	assert_equal "$(cat 'deps/local #1/value')" new

	# A local update can create the checkout and still fail. Simulate that
	# after Git succeeds: the fallback must see the original remote, not the
	# temporary file URL.
	local real_git
	real_git="$(command -v git)"
	mkdir -p "$BATS_TEST_TMPDIR/git-wrapper"
	cat >"$BATS_TEST_TMPDIR/git-wrapper/git" <<'SH'
#!/bin/sh
case "$*" in
  *submodule.*.url=file://*)
    printf 'hit\n' >>"$GIT_WRAPPER_LOG"
    if [ "${GIT_WRAPPER_FAIL_BEFORE:-}" != 1 ]; then "$REAL_GIT" "$@"; fi
    exit 1
    ;;
  *) exec "$REAL_GIT" "$@" ;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/git-wrapper/git"
	REAL_GIT="$real_git" GIT_WRAPPER_LOG="$BATS_TEST_TMPDIR/wrapper-hit" PATH="$BATS_TEST_TMPDIR/git-wrapper:$PATH" \
		XDG_STATE_HOME="$BATS_TEST_TMPDIR/partial-state" GIT_ALLOW_PROTOCOL="file" \
		run --separate-stderr jouet test --baseline --cmd -- ./check-submodule
	assert [ -s "$BATS_TEST_TMPDIR/wrapper-hit" ]
	assert_success
	assert_output --partial 'baseline: the failure also fails'
	assert_equal "$(git -C 'deps/local #1' remote get-url origin)" "$source"

	# A shallow local checkout lacks the old gitlink. Restore its remote and
	# require the normal fallback to fetch the pinned commit.
	mv "${source}.unavailable" "$source"
	REAL_GIT="$real_git" GIT_WRAPPER_LOG="$BATS_TEST_TMPDIR/wrapper-hit" GIT_WRAPPER_FAIL_BEFORE=1 \
		PATH="$BATS_TEST_TMPDIR/git-wrapper:$PATH" XDG_STATE_HOME="$BATS_TEST_TMPDIR/before-state" \
		GIT_ALLOW_PROTOCOL="file" run --separate-stderr jouet test --baseline --cmd -- ./check-submodule
	assert_success
	assert_output --partial 'baseline: the failure also fails'
	assert_equal "$(git remote get-url origin)" "$parent_origin"

	mv 'deps/local #1' "$BATS_TEST_TMPDIR/saved-submodule"
	git clone -q --depth 1 "file://$source" 'deps/local #1'
	run git -C 'deps/local #1' cat-file -e "${old_oid}^{commit}"
	assert_failure
	XDG_STATE_HOME="$BATS_TEST_TMPDIR/fallback-state" GIT_ALLOW_PROTOCOL="file" \
		run --separate-stderr jouet test --baseline --cmd -- ./check-submodule
	assert_success
	assert_output --partial 'baseline: the failure also fails'
	assert_equal "$(git -C 'deps/local #1' remote get-url origin)" "file://$source"
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
		# A new holder may have created the directory but not written its PID
		# yet. That is not an error, including when Bats has errexit enabled.
		holder=$(cat "$lock/pid" 2>/dev/null || true)
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
	if "$@"; then
		status=0
	else
		status=$?
	fi
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
	(cd "$dir" && npm install --no-audit --no-fund --loglevel=error)
}

@test "a failed npm fixture install reports its error" {
	DEPS="$BATS_TEST_TMPDIR/npm-deps"
	cat >"$BATS_TEST_TMPDIR/bin/npm" <<'SH'
#!/bin/sh
echo 'fixture registry unavailable' >&2
exit 1
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/npm"
	run --separate-stderr npm_install jest
	assert_failure
	assert_equal "$stderr" 'fixture registry unavailable'
}

@test "a successful npm fixture install is reused" {
	DEPS="$BATS_TEST_TMPDIR/npm-deps"
	cat >"$BATS_TEST_TMPDIR/bin/npm" <<'SH'
#!/bin/sh
printf 'install\n' >>"$BATS_TEST_TMPDIR/npm-invocations"
mkdir -p node_modules/.bin
touch node_modules/.bin/jest
chmod +x node_modules/.bin/jest
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/npm"
	run --separate-stderr npm_install jest
	assert_success
	run --separate-stderr npm_install jest
	assert_success
	assert_equal "$(cat "$BATS_TEST_TMPDIR/npm-invocations")" 'install'
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

@test "affected JavaScript selection uses the requested runner config" {
	for runner in vitest jest; do
		mkdir "$runner"
		(
			cd "$runner"
			init_repo
			js_project "$runner"
			if [[ $runner == vitest ]]; then
				printf "export default { test: { include: ['never/**/*.test.js'] } }\n" >vitest.config.js
				printf "export default { test: { include: ['test/**/*.test.js'] } }\n" >alternate.config.js
			else
				printf "module.exports = { testMatch: ['**/never/**/*.test.js'] };\n" >jest.config.cjs
				printf "module.exports = { testMatch: ['**/test/**/*.test.js'] };\n" >alternate.config.cjs
			fi
			if [[ $runner == vitest ]]; then
				git add vitest.config.js alternate.config.js
			else
				git add jest.config.cjs alternate.config.cjs
			fi
			git commit -qm configs
			git branch -f main HEAD
			printf '\n// changed\n' >>src/fmt.js
			if [[ $runner == vitest ]]; then
				config=alternate.config.js
			else
				config=alternate.config.cjs
			fi
			for config_form in separate equals short; do
				case $config_form in
				separate) config_args=(--config "$config") ;;
				equals) config_args=("--config=$config") ;;
				short) config_args=(-c "$config") ;;
				esac
				run --separate-stderr jouet test --affected --since main --dry-run --json -- "${config_args[@]}"
				assert_success
				run jq -r '.suites[0].tests[].path' <<<"$output"
				assert_output 'test/fmt.test.js'
			done
			run --separate-stderr jouet test --baseline --affected --since main --dry-run --json -- --config "$config"
			assert_success
			run jq -e '.suites[0] | ((.tests | map(.path) | index("test/fmt.test.js")) != null) and ((.command | index("--config")) != null) and ((.command | index("test/fmt.test.js")) != null)' <<<"$output"
			assert_success
			run --separate-stderr jouet test --affected --since main --json -- --config "$config"
			assert_success
			run jq -r '.ok, .suites[0].tests[].path' <<<"$output"
			assert_output $'true\ntest/fmt.test.js'
			printf '\n// config change\n' >>"$config"
			run --separate-stderr jouet test --affected --since main --dry-run --json -- --config "$config"
			assert_success
			run jq -r '.suites[0] | [.decision, .reason] | join(" ")' <<<"$output"
			assert_output "full the selected test configuration changed ($config)"
		)
	done
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
	assert_output --partial 'the isolated checkout may need dependencies: review [baseline].setup'

	# A configured but untrusted setup cannot run at the merge base yet.
	mkdir -p .jouet
	printf '[baseline]\nsetup = ["npm", "ci"]\n' >.jouet/test.toml
	run --separate-stderr jouet test --affected
	assert_failure 1
	assert_output --partial 'the isolated checkout may need dependencies: review [baseline].setup'
	run --separate-stderr jouet trust --yes
	assert_success
	run --separate-stderr jouet test --affected
	assert_failure 1
	assert_output --partial 'jouet test --baseline --affected'
	refute_output --partial 'the isolated checkout may need dependencies'
	rm .jouet/test.toml

	printf "export const shout = (s) => s + '!'\n" >src/fmt.js
	run --separate-stderr jouet test --affected --json
	assert_success
	run jq -c '[.affected, .ok, .suites[0].decision, [.suites[0].tests[].path], .suites[0].exit_code]' <<<"$output"
	assert_output '[true,true,"affected",["test/fmt.test.js"],0]'
}

@test "affected vitest baseline replays the selected files at the merge base" {
	js_project vitest
	# The real runner is available in the isolated worktree through this fixture
	# symlink, just as it is in the changed worktree.
	git add -f node_modules
	printf "export const shout = (s) => s + '?'\n" >src/fmt.js
	git add src/fmt.js
	git commit -qm 'fixture with existing failure'
	git branch -f main HEAD
	printf '// change\n' >>src/fmt.js

	run --separate-stderr jouet test --baseline --affected --since main --json
	assert_success
	run jq -e '.baseline.pre_existing | length > 0' <<<"$output"
	assert_success
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

@test "affected jest baseline freezes the selected test paths" {
	js_project jest
	printf 'exports.sum = (a, b) => b + a\n' >src/sum.js
	run --separate-stderr jouet test --baseline --affected --since main --dry-run --json -- --ci
	assert_success
	run jq -e '.suites[0].command | index("--runTestsByPath") != null and index("--findRelatedTests") == null and index("--ci") != null' <<<"$output"
	assert_success

	# Jest applies this Git-relative selector before --runTestsByPath.
	run --separate-stderr jouet test --baseline --affected --since main --dry-run -- --changedSince main
	assert_failure 4
	assert_stderr --partial 'cannot be frozen'
	run --separate-stderr jouet test --baseline --affected --since main --dry-run -- --changed-since=main
	assert_failure 4
	assert_stderr --partial 'cannot be frozen'
	run --separate-stderr jouet test --baseline --affected --since main --dry-run -- -f
	assert_failure 4
	assert_stderr --partial 'cannot be frozen'
	run --separate-stderr jouet test --baseline --affected --since main --dry-run -- -of
	assert_failure 4
	assert_stderr --partial 'cannot be frozen'
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
	# to the fixture's own text, which the full run passed at), once it is
	# more than a small fix.
	{
		printf 'def shout(s):\n    return s + "?"\n'
		for i in $(seq 60); do printf 'X%d = %d\n' "$i" "$i"; done
	} >src/calc/fmt.py
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
	assert_output --partial 'An affected run does not verify the full suite.'

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
	# A small run names its tests, so the agent sees its own ran.
	assert_output --regexp '^cmd: passed in [0-9.]+s: 3 passed \(case 1, case 2, case 3\) \(full log: .+\.log\)$'
	assert_equal "${#lines[@]}" 1
	assert_equal "$stderr" ""
	log="${output##*full log: }"
	log="${log%)}"
	assert_equal "$(cat "$log")" "$(./run-tests)"
}

@test "--cmd names a Playwright list failure and shows its assertion" {
	mkdir bin
	cat >bin/playwright <<'SH'
#!/bin/sh
cat <<'OUTPUT'
Running 1 test using 1 worker

  1) [chromium] › e2e/balance.spec.ts:5:3 › account › updates balance ───
    Error: expect(locator).toHaveText(expected) failed
    Expected: "42.00"
    Received: "17.00"

  1 failed
    [chromium] › e2e/balance.spec.ts:5:3 › account › updates balance
OUTPUT
exit "${PLAYWRIGHT_EXIT:-1}"
SH
	chmod +x bin/playwright
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- playwright test
	assert_failure 1
	assert_output --partial 'FAIL [chromium] › e2e/balance.spec.ts:5:3 › account › updates balance'
	assert_output --partial 'Expected: "42.00"'
	refute_output --partial 'no failure recognised'
	PATH="$PWD/bin:$PATH" PLAYWRIGHT_EXIT=0 run --separate-stderr jouet test --cmd -- playwright test
	assert_failure 4
	assert_output --partial 'command exited 0 but reported failing tests'
	printf '#!/bin/sh\nprintf "Error: No tests found.\\n"\n' >bin/playwright
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- playwright test
	assert_failure 4
	assert_output --partial 'no test ran'
}

@test "--cmd recognizes Playwright through npx" {
	mkdir bin
	printf '#!/bin/sh\nprintf "Error: No tests found.\\n"\n' >bin/npx
	chmod +x bin/npx
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- npx playwright test
	assert_failure 4
	assert_output --partial 'no test ran'

	# Interactive mode may succeed without running tests.
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- npx playwright test --only-changed --ui
	assert_success
}

@test "--cmd reports gofmt previews that find unformatted files" {
	mkdir bin
	printf 'package p\nfunc f( ){ }\n' >example.go
	cat >bin/gofmt <<'SH'
#!/bin/sh
[ "${GOFMT_EMPTY:-}" = 1 ] && exit 0
case "$1" in
  -d) printf 'diff example.go.orig example.go\n--- example.go.orig\n+++ example.go\n@@ -1 +1 @@\n-old\n+new\n' ;;
  -l) printf '%s\n' "${GOFMT_LIST:-example.go}" ;;
  -w) printf 'example.go\n' ;;
esac
exit 0
SH
	chmod +x bin/gofmt
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -d example.go
	assert_failure 1
	assert_output --partial 'unformatted'
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- env gofmt -l example.go
	assert_failure 1
	assert_output --partial 'unformatted'
	local newline_name=$'line\nbreak.go'
	printf 'package p\n' >"$newline_name"
	PATH="$PWD/bin:$PATH" GOFMT_LIST="$newline_name" run --separate-stderr jouet test --cmd -- gofmt -l "$newline_name"
	assert_failure 1
	assert_output --partial 'unformatted'
	PATH="$PWD/bin:$PATH" GOFMT_LIST=$'diagnostic\nline\nbreak.go' run --separate-stderr jouet test --cmd -- gofmt -l .
	assert_failure 1
	mkdir sub
	local many="sub/"
	for _ in {1..12}; do many+=$'line\n'; done
	many+="end.go"
	printf 'package p\n' >"$many"
	PATH="$PWD/bin:$PATH" GOFMT_LIST=$'diagnostic\n'"$many" run --separate-stderr jouet test --cmd -- gofmt -l .
	assert_failure 1
	local extensionless=$'line\nbreak'
	printf 'package p\n' >"$extensionless"
	PATH="$PWD/bin:$PATH" GOFMT_LIST="$extensionless" run --separate-stderr jouet test --cmd -- gofmt -l "$extensionless"
	assert_failure 1
	PATH="$PWD/bin:$PATH" GOFMT_LIST='diagnostic only' run --separate-stderr jouet test --cmd -- gofmt -l .
	assert_success
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --json --cmd -- env gofmt -l example.go
	assert_failure 1
	assert_equal "$(jq -c '[.ok, .suites[0].exit_code, .suites[0].format_changes]' <<<"$output")" '[false,0,true]'
	git add example.go
	git commit -qm base
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --baseline --cmd -- gofmt -l example.go
	assert_failure 1
	assert_output --partial 'baseline: not compared'
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --json --baseline --cmd -- gofmt -l example.go
	assert_failure 1
	assert_equal "$(jq -c '[.ok, .suites[0].format_changes]' <<<"$output")" '[false,true]'
	[[ "$stderr" == *'baseline: not compared'* ]]
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -w example.go
	assert_success
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -d -w example.go
	assert_success
	PATH="$PWD/bin:$PATH" GOFMT_EMPTY=1 run --separate-stderr jouet test --cmd -- gofmt -d example.go
	assert_success
}

@test "--cmd follows Go flag syntax for gofmt previews" {
	mkdir bin
	printf 'package p\n' >example.go
	cat >bin/gofmt <<'SH'
#!/bin/sh
printf '%s\n' "${GOFMT_OUTPUT:-example.go}"
SH
	chmod +x bin/gofmt
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt --l example.go
	assert_failure 1
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -l=1 example.go
	assert_failure 1
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -l=false --l=True example.go
	assert_failure 1
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt --w=false -l=1 example.go
	assert_failure 1
	PATH="$PWD/bin:$PATH" GOFMT_OUTPUT='diff example.go.orig example.go' run --separate-stderr jouet test --cmd -- gofmt --d=1 example.go
	assert_failure 1
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -l -l=false example.go
	assert_success
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt --l -w=1 example.go
	assert_success
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt -r -l example.go
	assert_success
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- gofmt example.go -l
	assert_success
}

@test "--cmd refuses a recognised test run that selected no tests" {
	mkdir bin
	printf '#!/bin/sh\nprintf "running 0 tests\\ntest result: ok. 0 passed; 0 failed; 0 ignored; 3 filtered out\\n"\n' >bin/cargo
	chmod +x bin/cargo
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test missing_name
	assert_failure 4
	assert_output --partial 'no test ran'
	assert_output --partial '(exit 4)'
	# --bin runs only the binary's own tests: the lib's need --lib.
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test --bin app missing_name
	assert_failure 4
	assert_output --partial 'lib tests run with --lib'
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test --lib --bin app missing_name
	refute_output --partial 'lib tests run with --lib'

	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --json --cmd -- cargo test missing_name
	assert_failure 4
	assert_equal "$(jq -c '[.ok, .suites[0].exit_code, .suites[0].no_tests]' <<<"$output")" '[false,0,true]'

	# The output alone cannot turn an unrelated command into a failed check.
	cp bin/cargo ./other-command
	run --separate-stderr jouet test --cmd -- ./other-command
	assert_success

	printf '#!/bin/sh\nprintf "running 1 test\\ntest one ... ok\\ntest result: ok. 1 passed; 0 failed; 0 ignored\\n"\n' >bin/cargo
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test one
	assert_success
}

@test "--cmd refuses Go runs when every package has no test files" {
	mkdir bin
	cat >bin/go <<'SH'
#!/bin/sh
printf '?   \texample.com/a\t[no test files]\n'
if [ "${WITH_TESTS:-0}" = 1 ]; then
  printf 'ok  \texample.com/b\t0.123s\n'
else
  printf '?   \texample.com/b\t[no test files]\n'
fi
SH
	chmod +x bin/go
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- go test ./...
	assert_failure 4
	assert_output --partial 'no test ran'
	PATH="$PWD/bin:$PATH" WITH_TESTS=1 run --separate-stderr jouet test --cmd -- go test ./...
	assert_success
}

@test "--cmd refuses Go JSON runs when no test ran" {
	mkdir bin
	cat >bin/go <<'SH'
#!/bin/sh
printf '%s\n' '{"Action":"output","Package":"example.com/a","Output":"?   \texample.com/a\t[no test files]\n"}'
if [ "${WITH_TESTS:-0}" = 1 ]; then
  printf '%s\n' '{"Action":"output","Package":"example.com/b","Output":"ok  \texample.com/b\t0.120s\n"}'
fi
SH
	chmod +x bin/go
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- go test -json ./...
	assert_failure 4
	assert_output --partial 'no test ran'
	PATH="$PWD/bin:$PATH" WITH_TESTS=1 run --separate-stderr jouet test --cmd -- go test -json ./...
	assert_success
}

@test "--cmd reports RSpec failures and rejects contradictory success" {
	mkdir bin
	cat >bin/rspec <<'SH'
#!/bin/sh
cat <<'OUTPUT'
Failures:

  1) a meaningful example
     Failure/Error: expect(actual).to eq(expected)
       expected: 2
            got: 1
     # ./spec/example_spec.rb:12

Finished in 0.1 seconds
1 example, 1 failure
OUTPUT
exit "${RSPEC_EXIT:-1}"
SH
	chmod +x bin/rspec
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- rspec
	assert_failure 1
	assert_output --partial 'FAIL a meaningful example'
	assert_output --partial 'expected: 2'
	PATH="$PWD/bin:$PATH" RSPEC_EXIT=0 run --separate-stderr jouet test --cmd -- rspec
	assert_failure 4
	assert_output --partial 'command exited 0 but reported failing tests'
	printf '#!/bin/sh\nprintf "0 examples, 0 failures\\n"\n' >bin/rspec
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- rspec missing_spec
	assert_failure 4
	assert_output --partial 'no test ran'
	printf '#!/bin/sh\nprintf "0 examples, 0 failures, 1 error occurred outside of examples\\n"\n' >bin/rspec
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- rspec
	assert_failure 4
	assert_output --partial 'command exited 0 but reported failing tests'
	refute_output --partial 'no test ran'
}

@test "a successful test command that reports failures is not accepted as passing" {
	mkdir bin
	cat >bin/cargo <<'SH'
#!/bin/sh
printf 'running 2 tests\ntest broken ... FAILED\n---- broken stdout ----\nassertion failed\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n'
printf 'running 1 test\ntest later ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored\n'
exit 0
SH
	chmod +x bin/cargo
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test
	assert_failure 4
	assert_output --partial 'command exited 0 but reported failing tests'
	assert_output --partial 'FAIL broken'
	assert_output --partial '(exit 4)'
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --json --cmd -- cargo test
	assert_failure 4
	assert_equal "$(jq -c '[.ok, .suites[0].exit_code, .suites[0].failed]' <<<"$output")" '[false,0,1]'

	# A task script such as `make test` can hide the earlier runner's exit.
	cp bin/cargo bin/make
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- make test
	assert_failure 4
	assert_output --partial 'command exited 0 but reported failing tests'
	PATH="$PWD/bin:$PATH" run --separate-stderr sh -c 'jouet test --cmd -- cargo test && touch committed'
	assert_failure 4
	[ ! -e committed ]

	# Printing the same text is not a test command.
	cp bin/cargo ./other-command
	run --separate-stderr jouet test --cmd -- ./other-command
	assert_success
	printf '#!/bin/sh\nprintf "The word FAILED appears in a fixture.\\n"\n' >bin/cargo
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test
	assert_success
}

@test "a passing outer test can print a child Vitest error" {
	mkdir bin
	cat >bin/cargo <<'SH'
#!/bin/sh
printf 'running 1 test\n RUN  v4.1.5 /work\nVitest caught 1 unhandled error during the test run.\n⎯⎯⎯ Unhandled Source Error ⎯⎯⎯\nTypeCheckError: fixture\n ❯ src/checks.ts:4:2\n⎯⎯⎯\ntest child_output ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored\n'
exit 0
SH
	chmod +x bin/cargo
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --cmd -- cargo test -- --nocapture
	assert_success
}

@test "a contradictory merge-base test result is not recorded as a pass" {
	mkdir bin
	cat >bin/cargo <<'SH'
#!/bin/sh
printf 'running 2 tests\ntest broken ... FAILED\n---- broken stdout ----\nassertion failed\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n'
test "$(cat mode)" = base
SH
	chmod +x bin/cargo
	printf 'base\n' >mode
	git add mode
	git commit -qm base
	git checkout -qb work
	printf 'changed\n' >mode
	PATH="$PWD/bin:$PATH" run --separate-stderr jouet test --baseline --cmd -- cargo test
	assert_failure 1
	assert_output --partial 'exited 0 but reported failing tests; failures cannot be compared'
}

@test "a Vitest failure keeps an unhandled error beside a named test failure" {
	cat >run-tests <<'SH'
#!/bin/sh
printf ' RUN  v4.1.5 /repo\n FAIL  test/a.test.ts > fails\nAssertionError: wrong\n⎯⎯⎯ Unhandled Errors ⎯⎯⎯\nVitest caught 1 unhandled error during the test run.\n⎯⎯⎯ Unhandled Rejection ⎯⎯⎯\nError: connection lost\n Test Files  1 failed (1)\n Tests  1 failed (1)\n'
exit 1
SH
	chmod +x run-tests
	JOUET_TEST_BASELINE=off run --separate-stderr jouet test --cmd -- ./run-tests
	assert_failure 1
	assert_output --partial 'FAIL test/a.test.ts > fails'
	assert_output --partial 'Vitest unhandled errors (details in full log)'
	assert_output --partial 'Error: connection lost'
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

@test "--budget after -- is taken as jouet's, and the command never sees it" {
	tap_script 6 "2 3 4 5 6"
	mv run-tests tap
	printf '#!/bin/sh\necho "$*" > args\nexec ./tap\n' >run-tests
	chmod +x run-tests

	run --separate-stderr jouet test --cmd -- ./run-tests test_a --budget 150

	assert_failure 1
	assert_equal "$(cat args)" "test_a"
	assert_equal "$stderr" "jouet: took \`--budget 150\` after \`--\` as jouet's; it goes before \`--\`"
	assert_line --index 0 --regexp '^cmd: failed \(exit 1\) in [0-9.]+s: 5 failed, 1 passed$'
	assert_output --partial '... 3 more failures'

	run --separate-stderr jouet test --budget 4000 --cmd -- ./run-tests --budget=150 test_b
	assert_failure 1
	assert_equal "$(cat args)" "test_b"
	assert_output --partial '... 3 more failures'
}

@test "--budget after -- without a number is refused, with the command it meant" {
	run --separate-stderr jouet test --cmd -- mise run test:e2e test_a --budget
	assert_failure 2
	assert_equal "$stderr" "jouet: \`--budget\` after \`--\` would go to the command, not to jouet: put it before \`--\`
hint: jouet test --budget BYTES --cmd -- mise run test:e2e test_a"

	run --separate-stderr jouet test -- -k x --budget=lots
	assert_failure 2
	assert_stderr --partial 'hint: jouet test --budget lots -- -k x'
}

@test "a person at a terminal sees the output as it runs" {
	tap_script 2 ""

	run in_tty jouet test --cmd -- ./run-tests

	assert_success
	assert_output --partial 'ok 2 case 2'
	assert_output --regexp 'cmd: passed in [0-9.]+s: 2 passed \(full log: '
}

@test "the hooks steer a cut test run to --cmd, denying once warned, and record a --cmd build as a build" {
	payload() {
		jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
	}

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "cargo test -q -p app 2>&1 | grep -E 'panicked|FAILED' | head -20")"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "Run \`jouet test --cmd -- cargo test -q -p app\` instead of cutting its output with grep."
	assert_output --partial '[jouet rule: piped-test-output]'

	# Warned once in this session, the next cut run is denied. Assignments
	# stay in front, and what the shell would expand stays bare.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "RUST_LOG=debug cargo test --manifest-path \$DIR/Cargo.toml 'a b' | tail")"
	run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
	assert_line --index 0 deny
	assert_output --partial "already warned about cutting a run's output. Run \`RUST_LOG=debug jouet test --cmd -- cargo test --manifest-path \$DIR/Cargo.toml \"a b\"\` instead"
	assert_output --partial "Nothing in this call ran, so run its other commands again too."

	# Another session is only warned.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "cargo test | tail" | jq -c '.session_id = "s2"')"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "Run \`jouet test\` instead of cutting its output with tail."

	# So is a subagent of the warned session, which was never shown the
	# warning; once warned itself, it is denied.
	for verdict in warn deny; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload "cargo test | tail" | jq -c '.agent_id = "a7"')"
		run jq -r '.hookSpecificOutput.permissionDecision // "warn"' <<<"$output"
		assert_output "$verdict"
	done

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload 'cargo test > out.log 2>&1')"
	assert_output ""

	# The whole suite, cut: plain `jouet test` runs it.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload 'python3 -m unittest discover -v 2>&1 | tail -5')"
	run jq -r '.hookSpecificOutput.permissionDecisionReason' <<<"$output"
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

@test "a repeated check sees changes in ignored inputs by default" {
	printf '.state\n' >.gitignore
	cat >check-state <<'SH'
#!/bin/sh
test "$(cat .state)" = ok
SH
	chmod +x check-state
	git add .gitignore check-state
	git commit -qm base
	printf 'ok\n' >.state
	run --separate-stderr env -u JOUET_TEST_REUSE_S JOUET_TEST_BASELINE=off jouet test --cmd -- ./check-state
	assert_success
	printf 'bad\n' >.state
	run --separate-stderr env -u JOUET_TEST_REUSE_S JOUET_TEST_BASELINE=off jouet test --cmd -- ./check-state
	assert_failure 1
}

@test "a repeated build recreates ignored outputs by default" {
	printf 'dist/\n' >.gitignore
	printf '#!/bin/sh\nmkdir -p dist\necho built >dist/result\n' >build-output
	chmod +x build-output
	git add .gitignore build-output
	git commit -qm base
	run --separate-stderr env -u JOUET_TEST_REUSE_S jouet test --cmd -- ./build-output
	assert_success
	rm dist/result
	run --separate-stderr env -u JOUET_TEST_REUSE_S jouet test --cmd -- ./build-output
	assert_success
	assert [ -f dist/result ]
}

@test "--baseline runs an unnamed failure at the merge base without calling it a pass" {
	enable_result_reuse
	printf 'base\n' >tracked
	export RUNS_FILE="$BATS_TEST_TMPDIR/unnamed-runs"
	cat >fail-command <<'SH'
#!/bin/sh
echo run >>"$RUNS_FILE"
echo "the tool failed"
exit 1
SH
	chmod +x fail-command
	git add tracked fail-command
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >>tracked

	run --separate-stderr jouet test --baseline --cmd -- ./fail-command
	assert_failure 1
	assert_output --partial 'without naming a failing test, so the failures could not be compared'
	assert_output --partial 'it is not known whether they are the same'
	refute_output --partial 'baseline checkout reports missing node_modules'
	assert_equal "$(wc -l <"$RUNS_FILE" | tr -d ' ')" 2

	# The same command on the same files is answered from the first run;
	# only the merge base runs again.
	run --separate-stderr jouet test --baseline --json --cmd -- ./fail-command
	assert_failure 1
	assert_stderr --partial 'without naming a failing test, so the failures could not be compared'
	assert_equal "$(jq -r '.ok' <<<"$output")" false
	assert_equal "$(jq -r '.suites[0].reused' <<<"$output")" true
	assert_equal "$(wc -l <"$RUNS_FILE" | tr -d ' ')" 3
}

@test "a baseline missing pnpm dependencies suggests the opt-in setup" {
	cat >fail-command <<'SH'
#!/bin/sh
if [ ! -e .deps-ready ]; then
    echo 'WARN Local package.json exists, but node_modules missing, did you mean to install?'
else
    echo 'the current check failed'
fi
exit 1
SH
	chmod +x fail-command
	printf '.deps-ready\n' >.gitignore
	git add fail-command .gitignore
	git commit -qm base
	git checkout -qb work
	touch .deps-ready
	printf 'change\n' >change

	run --separate-stderr jouet test --baseline --cmd -- ./fail-command
	assert_failure 1
	assert_output --partial 'baseline checkout reports missing node_modules'
	assert_output --partial '[baseline].setup in .jouet/test.toml'
	assert_output --partial 'trust it before retrying'
}

@test "a baseline missing a project-local runner suggests the opt-in setup" {
	printf 'node_modules/\n' >.gitignore
	printf 'base\n' >tracked
	git add .gitignore tracked
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >>tracked
	mkdir -p node_modules/.bin
	printf '#!/bin/sh\necho "the current check failed"\nexit 1\n' >node_modules/.bin/vitest
	chmod +x node_modules/.bin/vitest

	run --separate-stderr jouet test --baseline --cmd -- ./node_modules/.bin/vitest
	assert_failure 1
	assert_output --partial 'baseline checkout has no project-local node_modules executable'
	assert_output --partial '[baseline].setup in .jouet/test.toml'
	assert_output --partial 'trust it before retrying'

	run --separate-stderr jouet test --baseline --cmd -- "$PWD/node_modules/.bin/vitest"
	assert_failure 1
	assert_output --partial 'baseline checkout has no project-local node_modules executable'
}

@test "--baseline does not pass an incomplete nextest run" {
	cp "$PROJECT_ROOT/src/testlog/fixtures/nextest.txt" nextest-output
	mkdir -p "$BATS_TEST_TMPDIR/bin"
	cat >"$BATS_TEST_TMPDIR/bin/cargo" <<'SH'
#!/bin/sh
cat nextest-output
exit 100
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/cargo"
	git add nextest-output
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >change

	run --separate-stderr env JOUET_SESSION_ID=s1 jouet test --baseline --cmd -- cargo nextest run
	assert_failure 1
	assert_output --partial 'tests were not run'
	run jq -c '[.trees[].failures.test // [] | length]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '[1]'
	run --separate-stderr jouet test --baseline --json --cmd -- cargo nextest run
	assert_failure 1
	run jq -e '.ok == false and .suites[0].nextest_incomplete == true and (has("baseline") | not)' <<<"$output"
	assert_success
	# Even an inconsistent zero exit cannot turn the incomplete report green.
	cat >"$BATS_TEST_TMPDIR/bin/cargo" <<'SH'
#!/bin/sh
cat nextest-output
exit 0
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/cargo"
	run --separate-stderr jouet test --cmd -- cargo nextest run
	assert_failure 4
	assert_output --partial 'tests were not run'
	assert_output --partial 'command exited 0 but nextest reported unrun tests'
	refute_output --partial 'cmd: passed in'
}

@test "--baseline keeps an unnamed failure unresolved when the merge base passes" {
	cat >check-command <<'SH'
#!/bin/sh
test "$(cat tracked)" = base
SH
	chmod +x check-command
	printf 'base\n' >tracked
	git add tracked check-command
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >tracked
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_failure 1
	assert_output --partial 'the command passed at'
	assert_output --partial 'current failure could not be named; it remains unresolved'
}

@test "an unnamed baseline failure is raised once and remains recorded" {
	mkdir -p "$BATS_TEST_TMPDIR/bin"
	printf '#!/bin/sh\necho tool failed\nexit 1\n' >"$BATS_TEST_TMPDIR/bin/pytest"
	chmod +x "$BATS_TEST_TMPDIR/bin/pytest"
	printf 'base\n' >tracked
	git add tracked
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >>tracked

	run --separate-stderr env JOUET_SESSION_ID=s1 PATH="$BATS_TEST_TMPDIR/bin:$PATH" jouet test --baseline --cmd -- pytest
	assert_failure 1
	stop=$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}')
	run --separate-stderr jouet hook claude stop <<<"$stop"
	run jq -r '.reason' <<<"$output"
	assert_output --partial 'the command also failed at the merge base'
	run --separate-stderr jouet hook claude stop <<<"$stop"
	assert_output ''
	run jq -c '[.trees[].failures.test // [] | length]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '[1]'
}

@test "--baseline does not equate failures when every base test failed in setup" {
	printf 'base\n' >tracked
	cat >check-command <<'SH'
#!/bin/sh
echo '1..3'
if [ "$(cat tracked)" = base ]; then
  printf 'not ok 1 target\n# setup failed\nnot ok 2 another\n# setup failed\nnot ok 3 third\n# setup failed\n'
else
  printf 'not ok 1 target\n# assertion failed\nok 2 another\nok 3 third\n'
fi
exit 1
SH
	chmod +x check-command
	git add tracked check-command
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >tracked

	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_failure 1
	assert_output --partial 'all 3 tests failed at'
	assert_output --partial 'failures cannot be compared safely'

	# The kept baseline has the same limitation on a later run.
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_failure 1
	assert_output --partial 'all 3 tests failed at'
}

@test "--baseline equates the same failures when both runs show only failures" {
	printf 'base\n' >tracked
	cat >check-command <<'SH'
#!/bin/sh
echo '1..2'
printf 'not ok 1 target\n# wrong answer\nnot ok 2 another\n# wrong answer\n'
exit 1
SH
	chmod +x check-command
	git add tracked check-command
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >tracked

	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_success
	refute_output --partial 'cannot be compared safely'
	assert_output --partial 'baseline: all 2 failures also fail at'
}

@test "--baseline compares complete Ruff diagnostics and rejects unexpected output" {
	printf 'original\n' >reason
	cat >ruff <<'SH'
#!/bin/sh
printf 'F821 Undefined name `x`\n --> a.py:1:7\n  |\n1 | %s\n  |       ^\n\n' "$(cat reason)"
test ! -f extra || cat extra
printf 'Found 1 error.\n'
if test -f unsafe; then
  printf '1 unsafe fix available with `--fix --unsafe-fixes`.\n'
else
  printf '[*] 1 fixable with the `--fix` option.\n'
fi
exit 1
SH
	chmod +x ruff
	git add reason ruff
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >changed
	git add changed
	git commit -qm change

	run --separate-stderr jouet test --baseline --cmd -- ./ruff check .
	assert_success
	assert_output --partial 'matching failure details'
	touch unsafe
	run --separate-stderr jouet test --baseline --cmd -- ./ruff check .
	assert_success
	assert_output --partial 'matching failure details'
	rm unsafe
	printf 'different\n' >reason
	run --separate-stderr jouet test --baseline --cmd -- ./ruff check .
	assert_failure 1
	assert_output --partial 'could not be compared'
	printf 'original\n' >reason
	printf 'error: could not read another file\n' >extra
	run --separate-stderr jouet test --baseline --cmd -- ./ruff check .
	assert_failure 1
	assert_output --partial 'could not be compared'
}

@test "--baseline leaves shifted Ruff diagnostics unresolved" {
	printf '1\n' >line
	printf 'F821\n' >rule
	cat >ruff <<'SH'
#!/bin/sh
line=$(cat line)
printf '%s Undefined name `x`\n --> a.py:%s:7\n  |\n%s | print(x)\n  |       ^\n\nFound 1 error.\n' "$(cat rule)" "$line" "$line"
exit 1
SH
	chmod +x ruff
	git add line rule ruff
	git commit -qm base
	git checkout -qb work
	printf '5\n' >line

	run --separate-stderr jouet test --baseline --cmd -- ./ruff check .
	assert_failure 1
	assert_output --partial 'could not be compared'
	refute_output --partial 'NEW FAILURE'

	printf 'F822\n' >rule
	run --separate-stderr jouet test --baseline --cmd -- ./ruff check .
	assert_failure 1
	assert_output --partial 'NEW FAILURE'
}

@test "--baseline compares complete remark link warnings with their details" {
	printf 'missing heading\n' >reason
	cat >check-command <<'SH'
#!/bin/sh
test ! -f earlier || cat earlier
printf '%s\n' '$ xargs -0 remark --use remark-validate-links --frail --quiet' \
  'docs/README.md' \
  "1:1-1:2 warning $(cat reason) missing-heading-in-file remark-validate-links:missing-heading-in-file" \
  '⚠ 1 warning' \
  '[ELIFECYCLE] Command failed with exit code 1.'
exit 1
SH
	chmod +x check-command
	cp check-command check-text
	cp check-command check-json
	git add reason check-command check-text check-json
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >changed
	git add changed
	git commit -qm change

	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_success
	assert_output --partial 'remark docs/README.md:1:1-1:2'
	printf 'different heading\n' >reason
	run --separate-stderr jouet test --baseline --cmd -- ./check-text
	assert_failure 1
	assert_output --partial 'could not be compared'
	assert_output --partial 'baseline log: '
	# The baseline log is also available to JSON consumers when the details differ.
	run --separate-stderr jouet test --baseline --json --cmd -- ./check-json
	assert_failure 1
	local baseline_log
	baseline_log=$(jq -er '.baseline.logs[0]' <<<"$output")
	[[ -f "$baseline_log" ]]
	run cat "$baseline_log"
	assert_output --partial 'missing heading'
	# Reusing the saved fingerprint does not promise a still-available log.
	run --separate-stderr jouet test --baseline --json --cmd -- ./check-json
	assert_failure 1
	run jq -e '.baseline | has("logs") | not' <<<"$output"
	assert_success
	printf 'missing heading\n' >reason
	printf 'Earlier task failed unexpectedly\n' >earlier
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_failure 1
	assert_output --partial 'could not be compared'
}

@test "--baseline does not count Go subtest names as separate failed tests" {
	enable_result_reuse
	export GO_RUNS_FILE="$BATS_TEST_TMPDIR/go-runs"
	cat >check-command <<'SH'
#!/bin/sh
echo run >>"$GO_RUNS_FILE"
printf '%s\n' \
  '--- PASS: TestOK (0.00s)' \
  '--- FAIL: TestTable (0.00s)' \
  '    --- FAIL: TestTable/one (0.00s)' \
  '        table_test.go:10: first case failed' \
  '    --- FAIL: TestTable/two (0.00s)' \
  '        table_test.go:11: second case failed' \
  'FAIL'
exit 1
SH
	chmod +x check-command
	git add check-command
	git commit -qm base
	git checkout -qb work
	printf 'change\n' >change
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_success
	assert_output --partial 'TestTable/one'
	assert_output --partial 'TestTable/two'
	assert_equal "$(wc -l <"$GO_RUNS_FILE" | tr -d ' ')" 2

	# A result kept by an older version lacks the passing/skip count. It
	# must be rerun rather than assumed to contain only failing tests.
	local known=("$XDG_STATE_HOME"/jouet/known/*.json)
	jq '(.entries[] | select(.outcome == "failed")) |= del(.non_failed)' "${known[0]}" >"$BATS_TEST_TMPDIR/old-known.json"
	cp "$BATS_TEST_TMPDIR/old-known.json" "${known[0]}"
	# The command itself is answered from the first run on these files.
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_success
	assert_equal "$(wc -l <"$GO_RUNS_FILE" | tr -d ' ')" 3

	# Cached failures from before detail fingerprints also require a rerun.
	jq '(.entries[] | select(.outcome == "failed")) |= del(.details)' "${known[0]}" >"$BATS_TEST_TMPDIR/old-known.json"
	cp "$BATS_TEST_TMPDIR/old-known.json" "${known[0]}"
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_success
	assert_equal "$(wc -l <"$GO_RUNS_FILE" | tr -d ' ')" 4
	jq '(.entries[] | select(.outcome == "failed")) |= del(.details_version)' "${known[0]}" >"$BATS_TEST_TMPDIR/old-known.json"
	cp "$BATS_TEST_TMPDIR/old-known.json" "${known[0]}"
	run --separate-stderr jouet test --baseline --cmd -- ./check-command
	assert_success
	assert_equal "$(wc -l <"$GO_RUNS_FILE" | tr -d ' ')" 5
}

@test "--baseline compares Go JSON failure details" {
	cat >check-go-json <<'SH'
#!/bin/sh
printf '%s\n' '{"Action":"run","Package":"example/a","Test":"TestBroken"}'
printf '{"Action":"output","Package":"example/a","Test":"TestBroken","Output":"%s\\n"}\n' "$(cat detail.txt)"
printf '%s\n' '{"Action":"fail","Package":"example/a","Test":"TestBroken"}'
printf '%s\n' '{"Action":"fail","Package":"example/a"}'
exit 1
SH
	chmod +x check-go-json
	printf 'expected original\n' >detail.txt
	git add check-go-json detail.txt
	git commit -qm base
	git checkout -qb work
	printf 'unrelated\n' >change.txt
	run --separate-stderr jouet test --baseline --cmd -- ./check-go-json
	assert_success
	assert_output --partial 'matching failure details'
	assert_output --partial 'TestBroken'

	printf 'different cause\n' >detail.txt
	run --separate-stderr jouet test --baseline --cmd -- ./check-go-json
	assert_failure 1
	assert_output --partial 'could not be compared'
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

	# A failure with the same test name but a different cause remains unresolved,
	# including when the merge-base result is already cached.
	perl -pi -e 's/needs brew/different cause/' tests/test_a.py
	run --separate-stderr jouet test --baseline
	assert_failure 1
	assert_output --partial 'could not be compared'
	assert_output --partial 'test_env'
	tests 2

	# A failure the merge base does not have still fails the run.
	tests 3
	run --separate-stderr jouet test --baseline
	assert_failure 1
	assert_output --partial 'baseline: NEW FAILURE: 1 of 2 failures is new in the current run; no matching failure was reported at main'
	assert_output --partial 'test_math'
	# The merge base's worktree is gone again.
	run git worktree list
	assert_equal "${#lines[@]}" 1

}

@test "affected baseline compares the selected tests and clears only that run" {
	export JOUET_TEST_BASELINE=off JOUET_SESSION_ID=s1
	mkdir -p tests
	: >tests/__init__.py
	cat >tests/test_a.py <<'PY'
import os
import unittest
class Tests(unittest.TestCase):
    def test_existing(self):
        if os.environ.get("AFFECTED_EXISTING", "1") == "1":
            self.fail("already broken")
    def test_changed(self):
        self.assertEqual(1 + 1, 2)
PY
	git add tests
	git commit -qm base
	git checkout -qb work
	printf '\n# a change\n' >>tests/test_a.py

	# A named failure in a partial run suggests the same selection with an
	# explicit baseline, preserving the base and runner arguments.
	run --separate-stderr env -u JOUET_TEST_BASELINE JOUET_SESSION_ID=hint PYTHONDONTWRITEBYTECODE=1 jouet test --affected --since main -- -v
	assert_failure 1
	assert_output --partial 'jouet test --baseline --affected --since main -- -v'
	refute_output --partial 'the isolated checkout may need dependencies'
	run --separate-stderr env -u JOUET_TEST_BASELINE JOUET_SESSION_ID=hint PYTHONDONTWRITEBYTECODE=1 jouet test --affected --since main --json -- -v
	assert_failure 1
	run jq -e '.affected and (.ok | not)' <<<"$output"
	assert_success

	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --affected --since main
	assert_failure 1
	refute_output --partial 'retries the selected tests'
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --affected --since main --json
	assert_success
	run jq -r '[.affected, .baseline.pre_existing[]] | @tsv' <<<"$output"
	assert_output --partial $'true\ttest_existing'
	run --separate-stderr jq -c '[.trees[].failures.test // [] | length]' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output '[0]'
	run --separate-stderr jq -r '.trees[].runs.test | [.full, .selector] | @tsv' "$XDG_STATE_HOME/jouet/sessions/s1.json"
	assert_output $'false\t--affected --since main'

	# The same command under a different environment must not reuse the
	# previous baseline, whose failure is no longer present at the base.
	cat >tests/test_a.py <<'PY'
import unittest
class Tests(unittest.TestCase):
    def test_existing(self):
        self.fail("new under this environment")
    def test_changed(self):
        self.assertEqual(1 + 1, 2)
PY
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 AFFECTED_EXISTING=0 jouet test --baseline --affected --since main
	assert_failure 1
	assert_output --partial 'test_existing'
	assert_output --partial 'is new'

	# A new failure in an existing selected test is not cleared by the old one.
	printf '\n    def test_new_failure(self):\n        self.fail("new failure")\n' >>tests/test_a.py
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --affected --since main
	assert_failure 1
	assert_output --partial 'test_new_failure'
	assert_output --partial 'is new'

	# A selected test module absent at the base is not treated as pre-existing.
	cat >tests/test_new.py <<'PY'
import unittest
class Added(unittest.TestCase):
    def test_added(self):
        self.fail("new module")
PY
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --affected --since main
	assert_failure 1
	assert_output --partial 'test_added'
}

@test "baseline setup requires trust and runs only in the isolated checkout" {
	export BASELINE_SETUP_TRACE="$BATS_TEST_TMPDIR/setup-trace"
	mkdir -p .jouet tests
	printf '[baseline]\nsetup = ["sh", "prepare.sh"]\n' >.jouet/test.toml
	printf '.baseline-ready\n' >.gitignore
	cat >prepare.sh <<'SH'
#!/bin/sh
printf '%s\n' "$PWD" >>"$BASELINE_SETUP_TRACE"
touch .baseline-ready
SH
	cat >tests/test_a.py <<'PY'
import os
import unittest
class Tests(unittest.TestCase):
    def test_existing(self):
        if not os.path.exists('.baseline-ready'):
            raise RuntimeError('setup did not run')
        self.fail('already broken')
PY
	git add .jouet/test.toml .gitignore prepare.sh tests/test_a.py
	git commit -qm base
	git checkout -qb work
	touch .baseline-ready
	printf '\n# change\n' >>tests/test_a.py

	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --since main --cmd -- python3 -m unittest discover -s tests
	assert_failure 1
	assert_output --partial 'not trusted'
	[ ! -e "$BASELINE_SETUP_TRACE" ]

	run --separate-stderr jouet trust --yes
	assert_success
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --since main --cmd -- python3 -m unittest discover -s tests
	assert_success
	assert_output --partial 'test_existing'
	run cat "$BASELINE_SETUP_TRACE"
	assert_success
	refute_output "$PWD"
	assert_output --partial 'jouet-baseline-'

	printf '[baseline]\nsetup = ["false"]\n' >.jouet/test.toml
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --since main --cmd -- python3 -m unittest discover -s tests
	assert_failure 1
	assert_output --partial 'not trusted'
	run --separate-stderr jouet trust --yes
	assert_success
	run --separate-stderr env PYTHONDONTWRITEBYTECODE=1 jouet test --baseline --since main --cmd -- python3 -m unittest discover -s tests
	assert_failure 1
	assert_output --partial 'baseline setup failed'
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
	local a=$!
	locked "$fixture" hold b &
	local b=$!
	locked "$fixture" hold c &
	local c=$!
	wait "$a" || fail "first fixture installation failed"
	wait "$b" || fail "second fixture installation failed"
	wait "$c" || fail "third fixture installation failed"

	# Each install ran, and each ended before the next started.
	run awk 'NR % 2 == 1 { if ($1 != "start") bad = 1; who = $2 } NR % 2 == 0 { if ($1 != "end" || $2 != who) bad = 1 } END { exit bad || NR != 6 }' "$log"
	assert_success
	assert [ ! -e "$fixture.lock" ]
	assert [ ! -e "$fixture.lock.takeover" ]
}

@test "a fixture lock without a PID is waited for" {
	local fixture="$BATS_TEST_TMPDIR/fixture" marker="$BATS_TEST_TMPDIR/ran" observed="$BATS_TEST_TMPDIR/observed" i
	cat() {
		if [[ ${1:-} == "$fixture.lock/pid" && ! -e $1 ]]; then
			touch "$observed"
		fi
		command cat "$@"
	}
	mkdir "$fixture.lock"
	locked "$fixture" touch "$marker" &
	local waiter=$!
	for ((i = 0; i < 50; i++)); do
		[[ -e $observed ]] && break
		sleep 0.1
	done
	assert [ -e "$observed" ]
	kill -0 "$waiter" || fail "waiter exited before the holder wrote its PID"
	rmdir "$fixture.lock"
	wait "$waiter" || fail "waiter failed after the lock was released"
	assert [ -e "$marker" ]
	run locked "$fixture" false
	assert_failure 1
	assert [ ! -e "$fixture.lock" ]
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

@test "a passing --cmd run of a command no rule knows counts as a partial test run, unless it only listed, and a failing one as nothing" {
	printf '#!/bin/sh\necho ok   a: check passes\nexit 0\n' >check-tasks
	printf '#!/bin/sh\nexit 0\n' >list-tasks
	printf '#!/bin/sh\nexit 1\n' >broken-check
	chmod +x check-tasks list-tasks broken-check
	# Whatever flags come before --cmd.
	run bash -c 'CLAUDE_CODE_SESSION_ID=s20 jouet test --budget 4000 --cmd -- ./check-tasks'
	assert_success
	run --separate-stderr jq -c '.trees[].runs.test | [.command, .exit_code, .full]' "$XDG_STATE_HOME/jouet/sessions/s20.json"
	assert_output '["jouet test --budget 4000 --cmd -- ./check-tasks",0,false]'
	# A pass that only listed tests is no test run.
	run bash -c 'CLAUDE_CODE_SESSION_ID=s23 jouet test --cmd -- ./list-tasks --list'
	assert_success
	run --separate-stderr jq -c '[.trees[].runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/s23.json"
	assert_output '[]'
	run bash -c 'CLAUDE_CODE_SESSION_ID=s21 jouet test --cmd -- ./broken-check'
	run --separate-stderr jq -c '[.trees[].runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/s21.json"
	assert_output '[]'
}

@test "ruff fix previews and fix-only runs do not count as checks" {
	printf '#!/bin/sh\ncase "$*" in *--diff*clean.py*) exit 0;; *--diff*) echo "would fix"; exit 1;; esac\nexit 0\n' >ruff
	chmod +x ruff

	run env CLAUDE_CODE_SESSION_ID=ruff_preview jouet test --cmd -- ./ruff check --diff .
	assert_failure 1
	run --separate-stderr jq -c '[.trees[]?.runs.lint // empty, .trees[]?.runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/ruff_preview.json"
	assert_output '[]'

	run env CLAUDE_CODE_SESSION_ID=ruff_fix_only jouet test --cmd -- ./ruff check --fix-only .
	assert_success
	run --separate-stderr jq -c '[.trees[]?.runs.lint // empty, .trees[]?.runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/ruff_fix_only.json"
	assert_output '[]'

	run env CLAUDE_CODE_SESSION_ID=ruff_shell jouet test --cmd -- sh -c './ruff check --fix-only .'
	assert_success
	run --separate-stderr jq -c '[.trees[]?.runs.lint // empty, .trees[]?.runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/ruff_shell.json"
	assert_output '[]'

	run env CLAUDE_CODE_SESSION_ID=ruff_shell_diff jouet test --cmd -- sh -c './ruff check --diff clean.py'
	assert_success
	run --separate-stderr jq -c '[.trees[]?.runs.lint // empty, .trees[]?.runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/ruff_shell_diff.json"
	assert_output '[]'

	run env CLAUDE_CODE_SESSION_ID=ruff_global jouet test --cmd -- ./ruff --isolated --color never check --fix-only .
	assert_success
	run --separate-stderr jq -c '[.trees[]?.runs.lint // empty, .trees[]?.runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/ruff_global.json"
	assert_output '[]'

	run env CLAUDE_CODE_SESSION_ID=ruff_global_shell jouet test --cmd -- sh -c './ruff --config check check --diff clean.py'
	assert_success
	run --separate-stderr jq -c '[.trees[]?.runs.lint // empty, .trees[]?.runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/ruff_global_shell.json"
	assert_output '[]'

	run env CLAUDE_CODE_SESSION_ID=ruff_check jouet test --cmd -- ./ruff check .
	assert_success
	run --separate-stderr jq -r '.trees[].runs.lint.exit_code' "$XDG_STATE_HOME/jouet/sessions/ruff_check.json"
	assert_output '0'

	# After `--`, a file named `--diff` is not a preview flag.
	run env CLAUDE_CODE_SESSION_ID=ruff_file jouet test --cmd -- ./ruff check -- --diff
	assert_failure 1
	run --separate-stderr jq -r '.trees[].runs.lint.exit_code' "$XDG_STATE_HOME/jouet/sessions/ruff_file.json"
	assert_output '1'
}

@test "unknown tests with Ruff-like flags still count as tests" {
	printf '#!/bin/sh\necho "1 passed"\n' >custom-test
	printf '#!/bin/sh\nexit 0\n' >ruff
	chmod +x custom-test ruff
	for flag in --diff --fix-only; do
		local session="custom_${flag#--}"
		run env CLAUDE_CODE_SESSION_ID="$session" jouet test --cmd -- ./custom-test "$flag"
		assert_success
		run --separate-stderr jq -r '.trees[].runs.test.exit_code' "$XDG_STATE_HOME/jouet/sessions/$session.json"
		assert_output '0'
	done
	run env CLAUDE_CODE_SESSION_ID=custom_chain jouet test --cmd -- sh -c './ruff check --fix-only . && ./custom-test --diff'
	assert_success
	run --separate-stderr jq -r '.trees[].runs.test.exit_code' "$XDG_STATE_HOME/jouet/sessions/custom_chain.json"
	assert_output '0'
}

@test "a run whose runner refused its arguments is no failed test run" {
	printf '#!/bin/sh\necho "error: unexpected argument '"'"'guide'"'"' found"\nexit 1\n' >cargo-like
	chmod +x cargo-like
	run bash -c 'CLAUDE_CODE_SESSION_ID=s22 jouet test --cmd -- ./cargo-like'
	assert_failure 1
	assert_output --partial 'usage: the runner refused its arguments and ran no test'
	run --separate-stderr jq -c '[.trees[].runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/s22.json"
	assert_output '[]'
}

@test "a Vitest run whose path matched no test file is no failed test run" {
	mkdir -p bin
	cat >bin/vitest <<'SH'
#!/bin/sh
echo " RUN  v4.1.12 $PWD"
echo
echo "  No test files found, exiting with code 1"
echo "  filter: $2"
exit 1
SH
	chmod +x bin/vitest
	run bash -c 'PATH="$PWD/bin:$PATH" CLAUDE_CODE_SESSION_ID=s23 jouet test --cmd -- vitest run src/a.acceptance.test.tsx'
	assert_failure 1
	assert_output --partial 'usage: the runner refused its arguments and ran no test'
	assert_output --partial 'No test files found, exiting with code 1'
	run --separate-stderr jq -c '[.trees[].runs.test // empty]' "$XDG_STATE_HOME/jouet/sessions/s23.json"
	assert_output '[]'
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
    echo "# keychain unavailable"
else
    echo "mise ERROR No version is set for shim: fnox"
fi
exit 1
SCRIPT
	git add run.sh
	git commit -q -m base
	git checkout -q -b work
	printf 'echo 1..3\necho "ok 1 a"\necho "ok 2 b"\necho "not ok 3 keychain"\necho "# keychain unavailable"\nexit 1\n' >run.sh
	git commit -q -am change

	run --separate-stderr jouet test --baseline --cmd -- sh run.sh
	assert_failure 1
	assert_output --partial 'baseline: the run at main failed (exit 1) without naming a failing test, so the failures could not be compared'
	# Not the bare word: the log's temporary path can hold it (bats-run-GhnewL).
	refute_output --regexp '(is|are) new'

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
	assert_output --partial 'failure could not be compared'
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
	enable_result_reuse
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

	# The suite's own command, run through --cmd, shares what is kept; on
	# the same files it is answered from the last run.
	run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests
	assert_success
	assert_output --regexp 'reused: .python3 -m unittest discover -s tests. ran on these same files'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 3
	run --separate-stderr jouet test --fresh --cmd -- python3 -m unittest discover -s tests
	assert_success
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 4
	# Another command has nothing kept. Past the limit, --cmd does not run
	# main on its own and names the --baseline command, which keeps what it
	# finds for the next run.
	JOUET_TEST_BASELINE_CMD_UNDER_MS=0 run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests -p 'test_*.py'
	assert_failure 1
	assert_output --regexp 'baseline: not compared, the run took 0s; .jouet test --baseline --cmd -- python3 -m unittest discover -s tests -p .test_\*\.py.. runs it at the merge base'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 5
	run --separate-stderr jouet test --baseline --cmd -- python3 -m unittest discover -s tests -p 'test_*.py'
	assert_success
	run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests -p 'test_*.py'
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	# A short run of a command with nothing kept runs main on its own.
	run --separate-stderr jouet test --cmd -- python3 -m unittest discover -s tests -p 'test_a*.py'
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'

	# Off, a failing run stays failed.
	JOUET_TEST_BASELINE=off run --separate-stderr jouet test
	assert_failure 1
	refute_output --partial 'baseline:'
}

@test "a cached baseline is not reused in a different environment" {
	mkdir -p tests
	cat >tests/test_env.py <<'PY'
import os
import unittest

class T(unittest.TestCase):
    def test_env(self):
        if os.environ.get("JOUET_BASELINE_FIXTURE") == "on":
            self.fail("same failure")
PY
	git add tests/test_env.py
	git commit -q -m base
	git checkout -q -b work
	jouet edit replace tests/test_env.py --old '        if os.environ.get("JOUET_BASELINE_FIXTURE") == "on":' --new '        if True:'

	JOUET_BASELINE_FIXTURE=on run --separate-stderr jouet test --baseline --cmd -- python3 -m unittest discover -s tests
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	JOUET_BASELINE_FIXTURE=off run --separate-stderr jouet test --baseline --cmd -- python3 -m unittest discover -s tests
	assert_failure 1
	assert_output --partial 'baseline: NEW FAILURE'
}

@test "a saved test result does not hide a changed shell environment" {
	enable_result_reuse
	git commit -q --allow-empty -m base
	JOUET_TEST_BASELINE=off OLDPWD=passing run --separate-stderr jouet test --cmd -- python3 -c 'import os, sys; sys.exit(os.environ["OLDPWD"] != "passing")'
	assert_success
	JOUET_TEST_BASELINE=off OLDPWD=passing run --separate-stderr jouet test --cmd -- python3 -c 'import os, sys; sys.exit(os.environ["OLDPWD"] != "passing")'
	assert_success
	assert_output --partial 'reused:'
	JOUET_TEST_BASELINE=off OLDPWD=failing run --separate-stderr jouet test --cmd -- python3 -c 'import os, sys; sys.exit(os.environ["OLDPWD"] != "passing")'
	assert_failure 1
	refute_output --partial 'reused:'
}

@test "result reuse keeps arguments separate from environment entries" {
	git commit -q --allow-empty -m base
	run --separate-stderr env -i "HOME=$HOME" "PATH=$PATH" "XDG_STATE_HOME=$XDG_STATE_HOME" JOUET_TEST_REUSE_S=3600 JOUET_TEST_BASELINE=off jouet test --cmd -- python3 -c 'import os, sys; sys.exit(os.getenv("A_JOUET_COLLISION") == "1")' A_JOUET_COLLISION 1
	assert_success
	run --separate-stderr env -i "HOME=$HOME" "PATH=$PATH" "XDG_STATE_HOME=$XDG_STATE_HOME" JOUET_TEST_REUSE_S=3600 JOUET_TEST_BASELINE=off A_JOUET_COLLISION=1 jouet test --cmd -- python3 -c 'import os, sys; sys.exit(os.getenv("A_JOUET_COLLISION") == "1")'
	assert_failure 1
	refute_output --partial 'reused:'
}

@test "result reuse accepts non UTF-8 environment values" {
	enable_result_reuse
	git commit -q --allow-empty -m base
	JOUET_NON_UTF8=$'\xff' run --separate-stderr jouet test --cmd -- python3 -c 'print("ok")'
	assert_success
	JOUET_NON_UTF8=$'\xff' run --separate-stderr jouet test --cmd -- python3 -c 'print("ok")'
	assert_success
	assert_output --partial 'reused:'
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

@test "a failure that blames the machine is said, and not run at the merge base" {
	enable_result_reuse
	# In the base commit too, so the merge base can run it.
	printf '#!/bin/sh\necho run >>"%s/runs"\nprintf "1..1\\nnot ok 1 keychain secrets\\n# Failed to initialize OS keyring store: Platform failure: DBus error: no session bus\\n"\nexit 1\n' "$BATS_TEST_TMPDIR" >keyring.sh
	chmod +x keyring.sh
	git add keyring.sh
	git commit -q -m base
	git checkout -q -b work
	echo change >notes.md
	git add notes.md
	git commit -q -m change
	run --separate-stderr jouet test --cmd -- ./keyring.sh
	assert_failure 1
	assert_output --partial 'environment: the failure blames this machine, not the change (`Failed to initialize OS keyring store'
	refute_output --partial 'baseline:'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 1
	# --baseline still compares it: it runs at the merge base, and fails the
	# same way there.
	# The run here is not repeated: the files are as they were.
	run --separate-stderr jouet test --baseline --cmd -- ./keyring.sh
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	assert_output --regexp 'reused: .\./keyring\.sh. ran on these same files'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/runs")" 2
}

@test "failing Go tests are run alone at the merge base, not the whole command" {
	cat >go.mod <<-'EOF'
		module example.com/shop

		go 1.22
	EOF
	# What `go test` prints for one failing test, from the check and from the
	# merge base's run of that test alone.
	cat >fail.txt <<-'EOF'
		--- FAIL: TestBroken (0.00s)
		    other_test.go:5: boom
		FAIL
		FAIL	example.com/shop/other	0.004s
		FAIL
	EOF
	printf '#!/bin/sh\necho run >>"%s/checks"\ncat "%s/fail.txt"\nexit 1\n' "$BATS_TEST_TMPDIR" "$PWD" >check.sh
	mkdir -p bin
	printf '#!/bin/sh\necho "$*" >>"%s/go"\ncat "%s/fail.txt"\nexit 1\n' "$BATS_TEST_TMPDIR" "$PWD" >bin/go
	chmod +x check.sh bin/go
	git add go.mod fail.txt check.sh
	git commit -q -m base
	git checkout -q -b work
	echo change >notes.md
	git add notes.md
	git commit -q -m change
	# Slow enough that the whole check is not run again at the merge base.
	PATH="$PWD/bin:$PATH" JOUET_TEST_BASELINE_CMD_UNDER_MS=0 run --separate-stderr jouet test --cmd -- ./check.sh
	assert_success
	assert_output --partial 'baseline: the failure also fails at main'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/checks")" 1
	assert_equal "$(cat "$BATS_TEST_TMPDIR/go")" 'test -run ^(TestBroken)$ example.com/shop/other'
	# One pattern runs in every package: a TestBroken failing in another
	# package at the merge base is not this one.
	sed s/shop.other/shop.elsewhere/ fail.txt >elsewhere.txt
	printf '#!/bin/sh\ncat "%s/elsewhere.txt"\nexit 1\n' "$PWD" >bin/go
	PATH="$PWD/bin:$PATH" JOUET_TEST_BASELINE_CMD_UNDER_MS=0 run --separate-stderr jouet test --cmd -- ./check.sh
	assert_failure 1
	refute_output --partial 'also fails at main'
}

@test "Go JSON baseline reruns the failing test with its package and JSON output" {
	cat >go.mod <<-'EOF'
		module example.com/shop

		go 1.22
	EOF
	cat >fail.json <<-'EOF'
		{"Action":"run","Package":"example.com/shop/other","Test":"TestBroken"}
		{"Action":"output","Package":"example.com/shop/other","Test":"TestBroken","Output":"other_test.go:5: boom\n"}
		{"Action":"fail","Package":"example.com/shop/other","Test":"TestBroken"}
		{"Action":"fail","Package":"example.com/shop/other"}
	EOF
	printf '#!/bin/sh\necho run >>"%s/checks"\ncat "%s/fail.json"\nexit 1\n' "$BATS_TEST_TMPDIR" "$PWD" >check.sh
	mkdir -p bin
	printf '#!/bin/sh\necho "$*" >>"%s/go"\ncat "%s/fail.json"\nexit 1\n' "$BATS_TEST_TMPDIR" "$PWD" >bin/go
	chmod +x check.sh bin/go
	git add go.mod fail.json check.sh
	git commit -q -m base
	git checkout -q -b work
	echo change >notes.md
	git add notes.md
	git commit -q -m change
	PATH="$PWD/bin:$PATH" JOUET_TEST_BASELINE_CMD_UNDER_MS=0 run --separate-stderr jouet test --cmd -- ./check.sh
	assert_success
	assert_output --partial 'also fails at main'
	assert_equal "$(cat "$BATS_TEST_TMPDIR/go")" 'test -json -run ^(TestBroken)$ example.com/shop/other'
	assert_equal "$(grep -c . "$BATS_TEST_TMPDIR/checks")" 1
}

@test "a large passing run names the tests the change adds, and only those" {
	git commit -q --allow-empty -m base
	cat >run-tests <<'SH'
#!/bin/sh
echo 1..10
i=0
for t in alpha beta gamma delta epsilon zeta eta theta iota kappa; do
	i=$((i + 1))
	echo "ok $i $t works"
done
SH
	chmod +x run-tests
	run --separate-stderr jouet test --cmd -- ./run-tests
	assert_success
	refute_output --partial 'among them'
	# The agent adds a test, in a file of its own.
	printf '@test "theta works" {\n\ttrue\n}\n' >new.bats
	run --separate-stderr jouet test --cmd -- ./run-tests
	assert_success
	assert_output --partial '10 passed, among them the changed theta works'
	# What the run writes is not the change: a report naming three tests.
	rm new.bats
	printf '#!/bin/sh\n./run-tests | grep -E "alpha|beta|gamma" >report.txt\n./run-tests\n' >run-and-report
	chmod +x run-and-report
	run --separate-stderr jouet test --cmd -- ./run-and-report
	assert_success
	refute_output --partial 'among them'
}

@test "a program not on PATH names the project's mise task that runs it" {
	command -v mise >/dev/null || skip "mise is not installed"
	printf '[tasks."test:frob"]\nrun = "frob tests/"\n[tasks.lint]\nrun = "echo lint"\n' >mise.toml
	MISE_TRUSTED_CONFIG_PATHS="$PWD" run jouet test --cmd -- frob tests/
	assert_failure
	assert_output --partial 'running frob: No such file or directory'
	assert_output --regexp '.mise run test:frob. runs it in this project'
	# A file task's script counts, and a script that only names the program
	# does not run it.
	mkdir -p mise-tasks/test
	printf '#!/bin/sh\nzork --all\n' >mise-tasks/test/zork
	chmod +x mise-tasks/test/zork
	printf '[tasks.say]\nrun = "echo quux"\n' >>mise.toml
	MISE_TRUSTED_CONFIG_PATHS="$PWD" run jouet test --cmd -- zork
	assert_output --regexp '.mise run test:zork. runs it in this project'
	MISE_TRUSTED_CONFIG_PATHS="$PWD" run jouet test --cmd -- quux
	refute_output --partial 'mise run'
}
