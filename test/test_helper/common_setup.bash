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
