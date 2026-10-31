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

# A workflow whose jobs read secrets, and a jactionlint that plans it and
# keeps a copy of the --secret-file it is given, with its mode.
secrets_workflow() {
	cat >.github/workflows/ci.yml <<'YAML'
name: CI
on: push
env:
  REGISTRY: ${{ secrets.REGISTRY_TOKEN }}
jobs:
  build:
    steps:
      - run: make
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
          GH: ${{ secrets.GITHUB_TOKEN }}
  deploy:
    needs: build
    steps:
      - run: ./deploy
        env:
          KEY: ${{ secrets['DEPLOY_KEY'] }}
          CERT: ${{ secrets.CERT }}
YAML
	printf '[secrets]\nREGISTRY_TOKEN = {}\nNPM_TOKEN = {}\nCERT = {}\n[profiles.prod.secrets]\nDEPLOY_KEY = {}\n' >fnox.toml
	cat >"$BATS_TEST_TMPDIR/bin/jactionlint" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$BATS_TEST_TMPDIR/jactionlint.args"
case "$*" in
"run --version") echo "jactionlint run, runner behavior version 1" ;;
"run --list --json"*)
	deploy=true
	[[ "$*" == *"-j build"* ]] && deploy=false
	printf '[{"workflow": ".github/workflows/ci.yml", "name": "CI", "event": "push", "jobs": [
  {"id": "build", "stage": 1, "selected": true, "cells": [{"name": "build"}]},
  {"id": "deploy", "stage": 2, "selected": %s, "needs": ["build"], "cells": [{"name": "deploy"}]}]}]\n' "$deploy"
	;;
*)
	args=("$@")
	for i in "${!args[@]}"; do
		if [[ "${args[i]}" == --secret-file ]]; then
			file="${args[i + 1]}"
			cp "$file" "$BATS_TEST_TMPDIR/secret-file.copy"
			ls -l "$file" | cut -c1-10 >"$BATS_TEST_TMPDIR/secret-file.mode"
			printf '%s\n' "$file" >"$BATS_TEST_TMPDIR/secret-file.path"
		fi
	done
	printf '[job] build\n  [step 1] Run make\n  [step 1] success\n[job] build: success\n'
	;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/jactionlint"
}

@test "--secrets-from fnox passes the secrets the jobs read in a 0600 file, deleted afterwards" {
	secrets_workflow
	fake_fnox
	cat >"$BATS_TEST_TMPDIR/fnox.env" <<'ENV'
FAKE_FNOX_VAL_REGISTRY_TOKEN=reg-value-1
FAKE_FNOX_VAL_NPM_TOKEN='npm value 2'
FAKE_FNOX_VAL_CERT='line one\nline two'
FAKE_FNOX_VAL_DEPLOY_KEY=deploy-value-3
FAKE_FNOX_UNDEFINED=DEPLOY_KEY
ENV

	run --separate-stderr jouet ci run ci --secrets-from fnox --display agent

	assert_success
	assert_line "  secrets from fnox: given NPM_TOKEN, REGISTRY_TOKEN; missing DEPLOY_KEY, GITHUB_TOKEN; multi-line, not given CERT"
	refute_output --partial "value"
	assert_equal "$(cat "$BATS_TEST_TMPDIR/secret-file.copy")" 'NPM_TOKEN="npm value 2"
REGISTRY_TOKEN="reg-value-1"'
	assert_equal "$(cat "$BATS_TEST_TMPDIR/secret-file.mode")" "-rw-------"
	assert [ ! -e "$(cat "$BATS_TEST_TMPDIR/secret-file.path")" ]
	# Values never reach jactionlint's command line.
	run grep -c value "$BATS_TEST_TMPDIR/jactionlint.args"
	assert_output 0
	# All at once, then one by one after an unknown name failed the batch.
	assert_equal "$(cat "$BATS_TEST_TMPDIR/fnox.log")" "export --format json CERT DEPLOY_KEY NPM_TOKEN REGISTRY_TOKEN
export --format json CERT
export --format json DEPLOY_KEY
export --format json NPM_TOKEN
export --format json REGISTRY_TOKEN"
}

@test "--secrets-from fnox only resolves what the selected jobs read, and reports it in --json" {
	secrets_workflow
	fake_fnox
	printf 'FAKE_FNOX_VAL_NPM_TOKEN=npm-value\nFAKE_FNOX_VAL_REGISTRY_TOKEN=reg-value\n' >"$BATS_TEST_TMPDIR/fnox.env"

	run --separate-stderr jouet ci run ci -j build --secrets-from fnox --json

	assert_success
	run jq -c '.secrets' <<<"$output"
	assert_output '{"from":"fnox","given":["NPM_TOKEN","REGISTRY_TOKEN"],"missing":["GITHUB_TOKEN"]}'
	assert_equal "$(cat "$BATS_TEST_TMPDIR/fnox.log")" "export --format json NPM_TOKEN REGISTRY_TOKEN"
}

@test "--secrets-from fnox refuses without fnox, or beside a --secret-file of yours" {
	secrets_workflow

	PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet ci run ci --secrets-from fnox
	assert_failure 4
	assert_equal "${stderr_lines[0]}" "jouet: --secrets-from fnox needs fnox, and it is not installed"

	fake_fnox
	run --separate-stderr jouet ci run ci --secrets-from fnox -- --secret-file mine.env
	assert_failure 2

	run --separate-stderr jouet ci run ci --secrets-from vault
	assert_failure 2
}
