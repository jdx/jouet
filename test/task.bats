#!/usr/bin/env bats

# Winding into mise tasks: `jouet task propose` drafting a task from the
# commands sessions repeat in a repository, `jouet task apply` writing it
# through mise, the session start naming what was noticed, and `jouet
# unwind` proposing to remove a task nobody runs. A fake `mise` records its
# arguments in $MISE_LOG and answers `--version`, `tasks ls --json` (from
# $MISE_TASKS) and `tasks add|set|rm --json` the way a mise newer than 2026.10.24
# does.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_WIND JOUET_HOOKS JOUET_HOOKS_PRIME
	unset CLAUDE_CONFIG_DIR CODEX_HOME FAKE_MISE_VERSION
	export MISE_LOG="$BATS_TEST_TMPDIR/mise.log"
	# The project's one task runs `cargo fmt && cargo clippy`.
	export MISE_TASKS="$BATS_TEST_TMPDIR/tasks.json"
	echo '[{"name": "lint", "run": ["cargo fmt && cargo clippy"]}]' >"$MISE_TASKS"
	fake_mise
	git init -q
	git config user.email a@example.com
	git config user.name a
	printf '[tools]\nrust = "1"\n' >mise.toml
	touch Cargo.toml
	git add mise.toml Cargo.toml
	git commit -qm init
	ROOT=$(pwd -P)
	STATE="$XDG_STATE_HOME/jouet/wind.json"
}

fake_mise() {
	cat >"$BATS_TEST_TMPDIR/bin/mise" <<'SH'
#!/usr/bin/env bash
printf '%s\0' "$@" | jq -cRs 'split("\u0000")[:-1]' >>"$MISE_LOG"
# What `tasks add|set|rm --json` prints for the task its arguments name.
report() {
	local action=$1 path="$PWD/mise.toml" name="" run=() dry=false table='{}'
	shift 2
	while (($#)); do
		case $1 in
		--env) path="$PWD/mise.$2.toml"; shift ;;
		--path) path=$2; shift ;;
		--description) table=$(jq -c --arg v "$2" '.description = $v' <<<"$table"); shift ;;
		--depends | --sources) table=$(jq -c --arg k "${1#--}" --arg v "$2" '.[$k] += [$v]' <<<"$table"); shift ;;
		--dry-run) dry=true ;;
		--json | --yes) ;;
		--) shift; run=("$@"); break ;;
		*) name=$1 ;;
		esac
		shift
	done
	local text="${run[*]}"
	[[ ${run[0]} == sh && ${run[1]} == -c ]] && text="sh -c '${run[2]}'"
	jq -n --arg t "$name" --arg a "$action" --arg p "$path" --arg r "$text" --argjson d "$dry" --argjson table "$table" \
		'{task: $t, action: $a, kind: "toml", path: $p, table: ($table + {run: $r}), newly_trusted: false, dry_run: $d}'
}
case "$1 $2" in
"--version ") echo "${FAKE_MISE_VERSION:-2026.10.25} linux-x64 (2026-10-31)" ;;
"tasks ls") cat "$MISE_TASKS" ;;
"tasks set")
	# An old mise reads `set` as a task name and prints the help of `mise tasks`.
	[[ $3 == --help ]] && { echo "Usage: mise tasks [TASK]"; exit 0; }
	# jouet sets a task to what it wrote before, which changes nothing.
	report unchanged "$@" ;;
"tasks add") report added "$@" ;;
"tasks rm") report removed "$@" | jq -s . ;;
*) echo "fake mise: unexpected $*" >&2; exit 1 ;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/bin/mise"
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
			'{type: "user", timestamp: $ts, sessionId: $s, cwd: $cwd, message: {role: "user", content: [{type: "tool_result", tool_use_id: $id, content: "test result: ok. 12 passed", is_error: false}]}}' >>"$file"
	done
}

# Two sessions that run `cargo build && cargo test -p app` three times, and
# `cargo fmt && cargo clippy` (which the `lint` task runs) four times.
sessions() {
	transcript aaaaaaaa-0000-4000-8000-000000000001 \
		"cd $ROOT && cargo build && cargo test -p app 2>&1 | tail -20" "cargo fmt && cargo clippy" "git status"
	transcript aaaaaaaa-0000-4000-8000-000000000002 \
		"cargo build && cargo test -p app" "cargo fmt && cargo clippy" "cargo build && cargo test -p app" \
		"cargo fmt && cargo clippy" "cargo fmt && cargo clippy"
}

# The project's tasks: `build-test` as jouet wrote it.
ours() {
	jq -n '[{name: "build-test", run: ["sh -c '\''cargo build && cargo test -p app'\''"]}]' >"$MISE_TASKS"
}

# The session-start hook's primer for SESSION, in the repository.
primer() {
	jq -cn --arg s "$1" --arg cwd "$ROOT" '{hook_event_name: "SessionStart", session_id: $s, cwd: $cwd}' |
		jouet hook claude session-start |
		jq -r '.hookSpecificOutput.additionalContext'
}

# A command SESSION ran successfully in the repository, as Claude Code reports it.
ran() {
	jq -cn --arg s "$1" --arg c "$2" --arg cwd "$ROOT" \
		'{session_id: $s, cwd: $cwd, hook_event_name: "PostToolUse", tool_name: "Bash", tool_input: {command: $c}}' |
		jouet hook claude post-tool-use
}

@test "propose drafts a task for a command repeated across sessions, and none a task runs" {
	sessions

	run --separate-stderr jouet task propose

	assert_success
	assert_line "read 2 sessions in $ROOT since 14d"
	assert_line --regexp '^t1  build-test  ran 3 times in 2 sessions, 6s, about [0-9]+ tokens of output$'
	assert_line '    run      cargo build && cargo test -p app'
	assert_line '    suggest  mise.toml: it runs only tools the project already uses (cargo); it uses only paths inside the repository'
	assert_line --partial "    ask      Add the mise task \`build-test\` (\`cargo build && cargo test -p app\`) to mise.local.toml, just for you and not committed, or to mise.toml, shared"
	assert_line '    apply    jouet task apply t1 --file shared  (or --file local)'
	refute_output --partial 'clippy'
	assert_equal "$(head -1 "$MISE_LOG")" '["tasks","ls","--json"]'

	# Proposing again finds the same task, under the same id.
	run --separate-stderr jouet task propose --json
	assert_success
	run jq -c '[.updated, (.proposals[] | [.id, .name, .run, .suggest.file, .evidence.runs, .evidence.sessions, .apply])]' <<<"$output"
	assert_output '[["t1"],["t1","build-test","cargo build && cargo test -p app","shared",3,2,"jouet task apply t1 --file shared"]]'

	run --separate-stderr jouet wind show t1
	assert_success
	assert_line "t1 (pending): mise task \`build-test\`, ran 3 times in 2 sessions"
}

@test "propose suggests mise.local.toml where the repository shares no mise config" {
	git rm -q --cached mise.toml
	sessions

	run --separate-stderr jouet task propose --json

	assert_success
	run jq -c '.proposals[0].suggest' <<<"$output"
	assert_output '{"file":"local","reasons":["the repository has no mise.toml in git, so the task would be the first one shared"]}'
}

@test "propose refuses without a mise to read the project's tasks from" {
	rm "$BATS_TEST_TMPDIR/bin/mise"
	PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" run --separate-stderr jouet task propose
	assert_failure 4
	assert_equal "$stderr" "jouet: reading the project's tasks, to propose none it has: mise is not installed; jouet writes tasks through a mise newer than 2026.10.24"
}

@test "apply asks for the user's choice of file, needs a recent mise, and writes through mise" {
	sessions
	jouet task propose >/dev/null

	# Errors go to stderr, and there is nothing on stdout.
	run jouet task apply t1
	assert_failure 4
	assert_line --index 0 --partial "jouet: choose the file for the task \`build-test\`: ask the user whether it goes in mise.local.toml (just theirs, not committed) or mise.toml"
	assert_line --index 0 --partial 'Suggest mise.toml: it runs only tools the project already uses (cargo)'
	assert_line --index 1 "hint: then \`jouet task apply t1 --file shared\`, or \`--file local\`"

	run --separate-stderr jouet task apply t1 --file mine
	assert_failure 2

	FAKE_MISE_VERSION=2026.10.24 run jouet task apply t1 --file local
	assert_failure 4
	assert_line --index 0 "jouet: this needs a mise newer than 2026.10.24, the first with \`mise tasks set\` and \`rm\` and their JSON; the mise on PATH is 2026.10.24"
	assert_equal "$(tail -1 "$MISE_LOG")" '["tasks","set","--help"]'

	run --separate-stderr jouet task apply t1 --file local --json
	assert_success
	run jq -c '[.task, .file, .mise.action, .mise.path, .mise.table.run, .warning]' <<<"$output"
	assert_output "[\"build-test\",\"local\",\"added\",\"$ROOT/mise.local.toml\",\"sh -c 'cargo build && cargo test -p app'\",\"mise.local.toml is not ignored by git here; add it to .gitignore or .git/info/exclude so the task stays yours\"]"
	assert_equal "$(tail -1 "$MISE_LOG")" '["tasks","add","--json","--env","local","--description","cargo build, then cargo test (added by jouet: agents ran it 3 times in 2 sessions)","build-test","--","sh","-c","cargo build && cargo test -p app"]'

	run --separate-stderr jouet wind show t1
	assert_line --partial 't1 (adopted)'

	# The task is jouet's now, so applying again changes it in place, in the
	# file that has it: `mise tasks set` fails on another.
	ours
	run --separate-stderr jouet task apply t1 --file local
	assert_success
	assert_line --index 0 "task build-test unchanged in $ROOT/mise.local.toml; run it with \`mise run build-test\`"
	run jq -r '.[1:5] | join(" ")' <<<"$(tail -1 "$MISE_LOG")"
	assert_output "set --json --env local"
	run jouet task apply t1 --file shared
	assert_failure 4
	assert_line --index 0 "jouet: jouet added \`build-test\` to $ROOT/mise.local.toml, not mise.toml"

	# Anyone else's task is left alone.
	echo '[{"name": "lint", "run": ["cargo fmt && cargo clippy"]}]' >"$MISE_TASKS"
	run jouet task apply t1 --file shared --name lint
	assert_failure 4
	assert_line --index 0 "jouet: the project already has a task named \`lint\`"

	run --separate-stderr jouet task apply t9 --file local
	assert_failure 3
}

@test "apply with --dry-run reports what mise would write and records nothing" {
	sessions
	jouet task propose >/dev/null
	echo mise.local.toml >.gitignore

	run --separate-stderr jouet task apply t1 --file local --dry-run
	assert_success
	assert_output "task build-test added in $ROOT/mise.local.toml (dry run: nothing written); run it with \`mise run build-test\`"
	run jq -r '.[3]' <<<"$(tail -1 "$MISE_LOG")"
	assert_output '--dry-run'
	run jq -r '.tasks.proposals[0].status' "$STATE"
	assert_output pending
}

@test "the session start names a sequence the ledger noticed, then the task drafted for it" {
	for s in s1 s1 s2; do
		ran "$s" "cargo build && cargo test -p app 2>&1 | tail -5"
	done

	run --separate-stderr primer s3
	assert_success
	assert_line "jouet wind noticed \`cargo build && cargo test -p <arg>\` run 3 times in 2 sessions in this repository; a mise task may replace it. At a natural pause, \`jouet task propose\` drafts one from the session transcripts (\`jouet wind dismiss t1\` drops it)."

	sessions
	run --separate-stderr jouet task propose --json
	run jq -c '[.drafted, .proposals[0].evidence.ledger_runs]' <<<"$output"
	assert_output '[["t1"],3]'

	run --separate-stderr primer s4
	assert_line --partial "jouet task t1 is ready: a mise task \`build-test\` for \`cargo build && cargo test -p app\`, which agents ran 3 times in 2 sessions here"
	assert_line --partial "suggesting mise.toml because it runs only tools the project already uses (cargo); then \`jouet task apply t1 --file local|shared\`, or \`jouet wind dismiss t1\`."

	# Another repository's session hears nothing of it.
	mkdir other && git -C other init -q
	run --separate-stderr bash -c "jq -cn --arg cwd '$ROOT/other' '{hook_event_name: \"SessionStart\", session_id: \"s5\", cwd: \$cwd}' | jouet hook claude session-start"
	refute_output --partial 'jouet task'

	jouet wind dismiss t1
	run --separate-stderr primer s6
	refute_output --partial 'jouet task t1'
}

@test "unwind proposes removing a task jouet added that no session runs" {
	sessions
	jouet task propose >/dev/null
	jouet task apply t1 --file local >/dev/null
	ours

	# Not yet: it was added today.
	run --separate-stderr jouet unwind
	refute_output --partial 'build-test'

	jq '.tasks.applied[0].at -= 30 * 86400' "$STATE" >"$STATE.new" && mv "$STATE.new" "$STATE"
	run --separate-stderr jouet unwind
	assert_success
	assert_line "propose  task build-test ($ROOT/mise.local.toml)  jouet task apply added it 30 days ago (t1), and no agent session has run it since"

	# A task someone changed is theirs now.
	echo '[{"name": "build-test", "run": ["cargo build"]}]' >"$MISE_TASKS"
	run --separate-stderr jouet unwind
	refute_output --partial 'build-test'
	ours

	run --separate-stderr jouet unwind --apply
	assert_success
	assert_line --partial 'removed  task build-test'
	assert_equal "$(tail -1 "$MISE_LOG")" "[\"tasks\",\"rm\",\"--json\",\"--yes\",\"--path\",\"$ROOT/mise.local.toml\",\"build-test\"]"
	run jq -c '.tasks.applied' "$STATE"
	assert_output '[]'
}

@test "a session running a task jouet added keeps unwind from proposing it" {
	sessions
	jouet task propose >/dev/null
	jouet task apply t1 --file local >/dev/null
	ours
	jq '.tasks.applied[0].at -= 30 * 86400' "$STATE" >"$STATE.new" && mv "$STATE.new" "$STATE"

	ran s7 "mise run build-test"

	run jq -c '.tasks.applied[0].uses' "$STATE"
	assert_output 1
	run --separate-stderr jouet unwind
	refute_output --partial 'build-test'
}
