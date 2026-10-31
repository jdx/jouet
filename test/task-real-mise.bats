#!/usr/bin/env bats

# `jouet task` against a real mise, not task.bats' fake: opt in by naming the
# binaries, which CI does not, so CI stays hermetic and every test here skips.
#
#   JOUET_REAL_MISE=/path/to/mise   a mise newer than 2026.10.24, or a build
#                                   of mise's main from after jdx/mise#15213
#   JOUET_OLD_MISE=/path/to/mise    optional: mise 2026.10.24, to be refused
#
#   git -C ~/src/mise fetch origin
#   git -C ~/src/mise worktree add --detach /tmp/mise-main origin/main
#   (cd /tmp/mise-main && cargo build) && cp /tmp/mise-main/target/debug/mise /tmp/mise-new
#   gh release download v2026.10.24 -R jdx/mise -p 'mise-v2026.10.24-linux-x64' -O /tmp/mise-old
#   chmod +x /tmp/mise-old
#   JOUET_REAL_MISE=/tmp/mise-new JOUET_OLD_MISE=/tmp/mise-old test/bats/bin/bats test/task-real-mise.bats
#
# mise runs with its own HOME, data, config, state and cache directories
# under the test's, and trusts only the scratch repository, so it never reads
# or writes the real mise config.

setup() {
	load "test_helper/common_setup"
	_common_setup
	[[ -x ${JOUET_REAL_MISE:-} ]] || skip "set JOUET_REAL_MISE to a mise newer than 2026.10.24"
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_WIND JOUET_HOOKS JOUET_HOOKS_PRIME CLAUDE_CONFIG_DIR CODEX_HOME
	local name
	for name in $(compgen -e | grep '^MISE_'); do
		unset "$name"
	done
	export MISE_DATA_DIR="$BATS_TEST_TMPDIR/mise/data" MISE_CONFIG_DIR="$BATS_TEST_TMPDIR/mise/config"
	export MISE_STATE_DIR="$BATS_TEST_TMPDIR/mise/state" MISE_CACHE_DIR="$BATS_TEST_TMPDIR/mise/cache"
	ln -s "$JOUET_REAL_MISE" "$BATS_TEST_TMPDIR/bin/mise"

	git init -q
	git config user.email a@example.com
	git config user.name a
	# A table to keep: mise writes a task after it. (Into a file holding only
	# a comment, mise 2026.10.24's main writes the task above the comment.)
	printf '# the project config\n[env]\nAPP_ENV = "dev"\n' >mise.toml
	mkdir scripts
	printf '#!/bin/sh\necho built\n' >scripts/build.sh
	printf '#!/bin/sh\necho checked\n' >scripts/check.sh
	cat >scripts/report.sh <<'SH'
#!/bin/sh
mkdir -p "$(dirname "$1")" && echo reported >"$1"
SH
	chmod +x scripts/*.sh
	git add mise.toml scripts
	git commit -qm init
	ROOT=$(pwd -P)
	export MISE_TRUSTED_CONFIG_PATHS=$ROOT
	STATE="$XDG_STATE_HOME/jouet/wind.json"

	# The ledger's view, from the hooks: the shared sequence in two sessions.
	for s in s1 s1 s2; do
		ran "$s" "./scripts/build.sh && ./scripts/check.sh"
	done
	transcript aaaaaaaa-0000-4000-8000-000000000001 \
		"cd $ROOT && ./scripts/build.sh && ./scripts/check.sh" "./scripts/report.sh ~/reports/today.txt && ./scripts/check.sh" \
		"./scripts/build.sh && ./scripts/check.sh 2>&1 | tail -5"
	transcript aaaaaaaa-0000-4000-8000-000000000002 \
		"./scripts/build.sh && ./scripts/check.sh" "./scripts/report.sh ~/reports/today.txt && ./scripts/check.sh" \
		"./scripts/report.sh ~/reports/today.txt && ./scripts/check.sh"
}

# transcript ID COMMAND...: a Claude Code transcript in which session ID,
# in the repository, runs each COMMAND through the Bash tool.
transcript() {
	local id=$1
	shift
	local dir
	dir="$HOME/.claude/projects/$(tr '/.' '--' <<<"$ROOT")"
	mkdir -p "$dir"
	local file="$dir/$id.jsonl" n=0
	jq -cn --arg s "$id" --arg cwd "$ROOT" \
		'{type: "user", timestamp: "2026-10-30T10:00:00.000Z", sessionId: $s, cwd: $cwd, message: {role: "user", content: "build it"}}' >"$file"
	for command in "$@"; do
		n=$((n + 1))
		jq -cn --arg s "$id" --arg cwd "$ROOT" --arg c "$command" --arg id "t$n" --arg ts "2026-10-30T10:0$n:00.000Z" \
			'{type: "assistant", timestamp: $ts, sessionId: $s, cwd: $cwd, message: {id: ("m" + $id), usage: {input_tokens: 5, output_tokens: 40}, content: [{type: "tool_use", id: $id, name: "Bash", input: {command: $c}}]}}' >>"$file"
		jq -cn --arg s "$id" --arg cwd "$ROOT" --arg id "t$n" --arg ts "2026-10-30T10:0$n:02.000Z" \
			'{type: "user", timestamp: $ts, sessionId: $s, cwd: $cwd, message: {role: "user", content: [{type: "tool_result", tool_use_id: $id, content: "built\nchecked", is_error: false}]}}' >>"$file"
	done
}

# A command SESSION ran successfully in the repository, as Claude Code reports it.
ran() {
	jq -cn --arg s "$1" --arg c "$2" --arg cwd "$ROOT" \
		'{session_id: $s, cwd: $cwd, hook_event_name: "PostToolUse", tool_name: "Bash", tool_input: {command: $c}}' |
		jouet hook claude post-tool-use
}

primer() {
	jq -cn --arg s "$1" --arg cwd "$ROOT" '{hook_event_name: "SessionStart", session_id: $s, cwd: $cwd}' |
		jouet hook claude session-start |
		jq -r '.hookSpecificOutput.additionalContext'
}

@test "real mise: the ledger's sequence is named, then propose drafts it and a local one" {
	run --separate-stderr primer s3
	# The ledger keeps a program's name, not its path.
	assert_line --partial "jouet wind noticed \`build.sh && check.sh\` run 3 times in 2 sessions in this repository"

	run --separate-stderr jouet task propose --json
	assert_success
	run jq -c '[.drafted, .new, (.proposals[] | [.id, .name, .run, .suggest.file, .evidence.runs, .evidence.ledger_runs])]' <<<"$output"
	assert_output '[["t1"],["t2"],["t1","build-check","./scripts/build.sh && ./scripts/check.sh","shared",3,3],["t2","report-check","./scripts/report.sh ~/reports/today.txt && ./scripts/check.sh","local",3,null]]'
}

@test "real mise: apply writes mise.toml and mise.local.toml, and mise runs both tasks" {
	jouet task propose >/dev/null

	run --separate-stderr jouet task apply t1 --file shared --json
	assert_success
	run jq -c '[.mise.task, .mise.action, .mise.kind, .mise.path, .mise.newly_trusted, .mise.dry_run]' <<<"$output"
	assert_output "[\"build-check\",\"added\",\"toml\",\"$ROOT/mise.toml\",false,false]"
	run cat mise.toml
	assert_output "# the project config
[env]
APP_ENV = \"dev\"

[tasks.build-check]
description = \"build.sh, then check.sh (added by jouet: agents ran it 3 times in 2 sessions)\"
run = \"sh -c './scripts/build.sh && ./scripts/check.sh'\""

	echo mise.local.toml >.git/info/exclude
	run --separate-stderr jouet task apply t2 --file local
	assert_success
	assert_output "task report-check added in $ROOT/mise.local.toml; run it with \`mise run report-check\`"
	run cat mise.local.toml
	assert_output "[tasks.report-check]
description = \"report.sh, then check.sh (added by jouet: agents ran it 3 times in 2 sessions)\"
run = \"sh -c './scripts/report.sh ~/reports/today.txt && ./scripts/check.sh'\""

	run --separate-stderr mise tasks ls --json
	assert_success
	run jq -c 'map([.name, .run]) | sort' <<<"$output"
	assert_output "[[\"build-check\",[\"sh -c './scripts/build.sh && ./scripts/check.sh'\"]],[\"report-check\",[\"sh -c './scripts/report.sh ~/reports/today.txt && ./scripts/check.sh'\"]]]"

	run --separate-stderr mise run build-check
	assert_success
	assert_output "built
checked"
	run --separate-stderr mise run report-check
	assert_success
	assert_equal "$(cat "$HOME/reports/today.txt")" reported

	# Applying again goes through `mise tasks set`, which changes nothing.
	run --separate-stderr jouet task apply t1 --file shared --json
	assert_success
	run jq -c '[.command[1:3], .mise.action]' <<<"$output"
	assert_output '[["tasks","set"],"unchanged"]'
}

@test "real mise: unwind removes a task nobody ran with mise tasks rm" {
	jouet task propose >/dev/null
	jouet task apply t1 --file shared >/dev/null
	jq '.tasks.applied[0].at -= 30 * 86400' "$STATE" >"$STATE.new" && mv "$STATE.new" "$STATE"

	run --separate-stderr jouet unwind --apply --json
	assert_success
	run jq -c '.removed_tasks | map([.task, .action, .path])' <<<"$output"
	assert_output "[[\"build-check\",\"removed\",\"$ROOT/mise.toml\"]]"
	run cat mise.toml
	refute_output --partial 'build-check'
	run --separate-stderr mise tasks ls --json
	assert_output '[]'
}

@test "real mise: a mise no newer than 2026.10.24 is refused" {
	[[ -x ${JOUET_OLD_MISE:-} ]] || skip "set JOUET_OLD_MISE to mise 2026.10.24"
	jouet task propose >/dev/null
	ln -sf "$JOUET_OLD_MISE" "$BATS_TEST_TMPDIR/bin/mise"

	run jouet task apply t1 --file local
	assert_failure 4
	assert_line --index 0 "jouet: this needs a mise newer than 2026.10.24, the first with \`mise tasks set\` and \`rm\` and their JSON; the mise on PATH is 2026.10.24"
	[[ ! -e mise.local.toml ]]
}
