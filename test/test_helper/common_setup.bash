#!/usr/bin/env bash

_common_setup() {
	bats_require_minimum_version 1.5.0
	load "test_helper/bats-support/load"
	load "test_helper/bats-assert/load"

	export PROJECT_ROOT
	PROJECT_ROOT="$(cd "$BATS_TEST_DIRNAME/.." && pwd)"
	local target_dir="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"
	if [[ "$target_dir" != /* ]]; then
		target_dir="$PROJECT_ROOT/$target_dir"
	fi
	JOUET_BIN="${JOUET_BIN:-$target_dir/debug/jouet}"
	export JOUET_BIN
	if [[ ! -x "$JOUET_BIN" ]]; then
		echo "jouet binary not found; run 'mise run build' first: $JOUET_BIN" >&2
		return 1
	fi

	# On PATH as `jouet`, so a test reads like the command an agent runs, and a
	# script the test writes can call it too.
	mkdir -p "$BATS_TEST_TMPDIR/bin"
	ln -s "$JOUET_BIN" "$BATS_TEST_TMPDIR/bin/jouet"
	export PATH="$BATS_TEST_TMPDIR/bin:$PATH"

	# The display mode is under test, so nothing from the developer's shell
	# chooses it. Agent detection is cleared per test where it matters.
	unset JOUET_DISPLAY
	# The developer's own `entire` stays out of the tests; entire.bats turns it
	# back on with a fake one on PATH.
	export JOUET_ENTIRE=false
	export HOME="$BATS_TEST_TMPDIR/home"
	export XDG_STATE_HOME="$BATS_TEST_TMPDIR/state"
	export JOUET_LOG_DIR="$BATS_TEST_TMPDIR/logs"
	mkdir -p "$HOME" "$BATS_TEST_TMPDIR/work"
	cd "$BATS_TEST_TMPDIR/work" || return 1
}

# Runs a command under a pseudo-terminal with no agent in the environment, so
# the TTY check decides the display mode unless the arguments name an agent:
# `in_tty CLAUDECODE=1 jouet ...`. Leading NAME=VALUE words are set for the
# command. Output arrives with the terminal's CRLF line endings.
in_tty() {
	local -a vars=()
	while [[ "${1:-}" == *=* ]]; do
		vars+=("$1")
		shift
	done
	local -a cmd=(env -i "PATH=$PATH" "HOME=$HOME" "JOUET_LOG_DIR=$JOUET_LOG_DIR" TERM=dumb "${vars[@]}" "$@")
	if script --version >/dev/null 2>&1; then
		# util-linux takes one shell command string.
		script -qec "$(printf '%q ' "${cmd[@]}")" /dev/null </dev/null
	else
		# BSD script (macOS) takes the command as arguments.
		script -q /dev/null "${cmd[@]}" </dev/null
	fi
}

# The SHA-256 of a file, as `jouet edit hash` prints it.
sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$1" | cut -d' ' -f1
	else
		shasum -a 256 "$1" | cut -d' ' -f1
	fi
}

# Fails unless FILE holds exactly CONTENT, trailing newlines included.
assert_content() {
	local actual
	actual="$(cat "$1" && printf x)"
	assert_equal "${actual%x}" "$2"
}

# Turns the work directory into a git repository that the developer's git
# config cannot reach: no system or global config, a fixed identity, and hooks
# only from .git/hooks. The usual agent markers are cleared too, but the agent
# running the tests may set others, so a test that depends on `jouet commit`'s
# trailers sets AI_AGENT or JOUET_COMMIT_TRAILERS itself.
init_repo() {
	export GIT_CONFIG_NOSYSTEM=1
	export GIT_CONFIG_GLOBAL="$BATS_TEST_TMPDIR/gitconfig"
	: >"$GIT_CONFIG_GLOBAL"
	export GIT_AUTHOR_NAME=Test GIT_AUTHOR_EMAIL=test@example.com
	export GIT_COMMITTER_NAME=Test GIT_COMMITTER_EMAIL=test@example.com
	unset AI_AGENT CLAUDECODE CLAUDE_CODE_SESSION_ID CODEX_THREAD_ID JOUET_SESSION_ID JOUET_COMMIT_TRAILERS
	git init -q -b main
	git config core.hooksPath .git/hooks
}

# Writes FILE with CONTENT and commits it with MESSAGE.
commit_file() {
	mkdir -p "$(dirname "$1")"
	printf '%s' "$2" >"$1"
	git add -- "$1"
	git commit -q -m "$3"
}

# Puts a fake `fnox` on PATH that appends its arguments to
# $BATS_TEST_TMPDIR/fnox.log and, for `fnox scan --values --staged`, does
# what the real one does: reads the files the index stages (honouring
# GIT_INDEX_FILE), reports those holding FAKE_FNOX_VALUE as the secret
# FAKE_FNOX_NAME in fnox's JSON, and exits 1 when it found any. With
# FAKE_FNOX_ERROR set it fails as fnox does when it cannot resolve the
# secrets: a miette error on stderr, nothing on stdout, exit 1. For `fnox
# export --format json NAME...` it prints FAKE_FNOX_VAL_<NAME> for each
# name that has one, as JSON text, and fails as fnox does on a name listed
# in FAKE_FNOX_UNDEFINED. Those are read from $BATS_TEST_TMPDIR/fnox.env, a
# file of NAME=VALUE lines.
fake_fnox() {
	cat >"$BATS_TEST_TMPDIR/bin/fnox" <<'SH'
#!/bin/sh
printf '%s\n' "$*" >>"$BATS_TEST_TMPDIR/fnox.log"
# shellcheck disable=SC1091
[ -f "$BATS_TEST_TMPDIR/fnox.env" ] && . "$BATS_TEST_TMPDIR/fnox.env"
if [ "$1 $2 $3" = "export --format json" ]; then
	shift 3
	out=""
	for name in "$@"; do
		case " ${FAKE_FNOX_UNDEFINED:-} " in *" $name "*)
			printf "Error: fnox::secret_not_found\n\n  \303\227 Secret '%s' not found in profile 'default'\n\n" "$name" >&2
			exit 1
			;;
		esac
		eval "value=\${FAKE_FNOX_VAL_$name:-}"
		# shellcheck disable=SC2154 # set by the eval
		[ -n "$value" ] && out="$out${out:+,}
    \"$name\": \"$value\""
	done
	printf '{\n  "secrets": {%s\n  }\n}\n' "$out"
	exit 0
fi
[ "$1 $2 $3" = "scan --values --staged" ] || exit 0
if [ -n "${FAKE_FNOX_ERROR:-}" ]; then
	printf 'Error: fnox::config::error\n\n  \303\227 Configuration error: %s\n\n' "$FAKE_FNOX_ERROR" >&2
	exit 1
fi
if [ -n "${FAKE_FNOX_UNCHECKED:-}" ]; then
	printf 'Warning: 1 secret(s) have no value and were not checked: %s\n' "$FAKE_FNOX_UNCHECKED" >&2
fi
files=0
findings=""
for f in $(git diff --cached --name-only --diff-filter=ACMR); do
	files=$((files + 1))
	line=$(git show ":$f" | grep -n -F -- "${FAKE_FNOX_VALUE:-unset-value}" | head -1 | cut -d: -f1)
	if [ -n "$line" ]; then
		[ -n "$findings" ] && findings="$findings,"
		findings="$findings
    {
      \"path\": \"$f\",
      \"line\": $line,
      \"column\": 1,
      \"detector\": \"resolved-secret\",
      \"severity\": \"high\",
      \"redacted\": \"****\",
      \"secret\": \"${FAKE_FNOX_NAME:-SECRET}\"
    }"
	fi
done
count=$(printf '%s' "$findings" | grep -c '"path"')
printf '{\n  "findings": [%s\n  ],\n  "summary": {\n    "files_scanned": %s,\n    "files_with_findings": %s,\n    "findings": %s\n  }\n}\n' "$findings" "$files" "$count" "$count"
[ "$count" -eq 0 ]
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/fnox"
}
