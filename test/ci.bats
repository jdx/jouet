#!/usr/bin/env bats

# `jouet ci run` against a fake jactionlint that records its arguments and
# prints a transcript in the format of `jactionlint run`.

setup() {
	load "test_helper/common_setup"
	_common_setup
	mkdir -p .github/workflows
	printf 'name: CI\non: push\njobs: {}\n' >.github/workflows/ci.yml
	cat >"$BATS_TEST_TMPDIR/bin/jactionlint" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$BATS_TEST_TMPDIR/jactionlint.args"
case "$*" in
"run --version") echo "jactionlint run, runner behavior version 1" ;;
"run --list --json")
	cat <<'JSON'
[{"workflow": ".github/workflows/ci.yml", "name": "CI", "event": "push", "jobs": [
  {"id": "build", "stage": 1, "selected": true, "cells": [{"name": "build"}]},
  {"id": "test", "stage": 2, "selected": true, "needs": ["build"], "cells": [{"name": "test"}]},
  {"id": "mac", "stage": 1, "selected": true, "cells": [{"name": "mac"}]}]}]
JSON
	;;
"run --list")
	printf '.github/workflows/ci.yml  (CI)\n  stage 1\n    build   runs-on: ubuntu-latest\n    mac   runs-on: macos-latest\n  stage 2\n    test   needs: build   runs-on: ubuntu-latest   name: Unit tests\n'
	;;
*)
	cat <<'OUT'
[job] build
  [step 1] Run make
    built
  [step 1] success
[job] build: success
[job] Unit tests
  [step 1] run the suite
    ok 1
    not ok 2 - parses dates
  [step 1] failure (exit status 1)
[job] Unit tests: failure
[job] mac: skipped (runs-on macos-latest needs macOS, this machine is Linux (jactionlint run --explain runs-on-other-os))

Summary
  success   build
  failure   Unit tests
  skipped   mac  (runs-on macos-latest needs macOS, this machine is Linux (jactionlint run --explain runs-on-other-os))
OUT
	exit 1
	;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/jactionlint"
}

@test "a check name runs its job, and the failed step's output is shown" {
	run --separate-stderr jouet ci run CI --check "Unit tests" --display agent -- --secret TOKEN=abc

	assert_failure 1
	assert_line --index 0 --regexp '^FAIL jactionlint run \.github/workflows/ci\.yml -j test  1 pass, 1 fail, 1 skipped  \([0-9]+s\)$'
	assert_line "FAIL Unit tests"
	assert_line '  [step 1] "run the suite"  exit status 1'
	assert_line "    not ok 2 - parses dates"
	assert_line "skipped mac  (runs-on macos-latest needs macOS, this machine is Linux (jactionlint run --explain runs-on-other-os))"
	assert_line --regexp "^full log: $JOUET_LOG_DIR/ci-run-[0-9]+-[0-9]+\.log$"
	# The display name is only in the text plan, so both were read.
	assert_equal "$(cat "$BATS_TEST_TMPDIR/jactionlint.args")" "run --version
run --list --json
run --list
run .github/workflows/ci.yml -j test --secret TOKEN=abc"
	# The log keeps the whole transcript, as jactionlint printed it.
	run cat "$JOUET_LOG_DIR"/ci-run-*.log
	assert_line "    built"
	assert_line "  failure   Unit tests"
}

@test "--json reports the jobs, the excerpt and a masked command" {
	run --separate-stderr jouet ci run ci -j test --json -- --secret TOKEN=abc

	assert_failure 1
	run jq -c '[.result, .workflow, .selected, .command, .jobs[1].failed_steps[0].excerpt, .explain]' <<<"$output"
	assert_output '["failure","ci",["test"],["jactionlint","run","ci","-j","test","--secret","TOKEN=***"],["ok 1","not ok 2 - parses dates"],["runs-on-other-os"]]'
}

@test "a name that matches nothing lists what there is" {
	run --separate-stderr jouet ci run --check unit

	assert_failure 3
	assert_equal "$stderr" "jouet: no job matches the check \`unit\`
hint: the jobs: build, Unit tests, mac; or give the job with -j ID"

	run --separate-stderr jouet ci run deploy
	assert_failure 3
	assert_equal "$stderr" "jouet: no workflow is named \`deploy\`
hint: the workflows: .github/workflows/ci.yml (CI)"
}

@test "without jactionlint it says how to install it" {
	rm "$BATS_TEST_TMPDIR/bin/jactionlint"
	PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet ci run ci

	assert_failure 4
	assert_equal "$stderr" "jouet: jactionlint is not installed; it runs the workflow
hint: mise use -g jactionlint"
}

@test "a jactionlint without run is refused" {
	printf '#!/bin/sh\necho "unknown command run" >&2\nexit 2\n' >"$BATS_TEST_TMPDIR/bin/jactionlint"

	run --separate-stderr jouet ci run ci

	assert_failure 4
	assert_equal "$stderr" "jouet: this jactionlint cannot run workflows: it has no \`run\` command
hint: upgrade it: mise up jactionlint"
}
