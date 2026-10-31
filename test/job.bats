#!/usr/bin/env bats
# Messages name commands in backquotes, which the tests match literally.
# shellcheck disable=SC2016

# `jouet job` and `jouet wait job`: background work run detached, without a
# daemon, as an agent runs it; and the jobs in handoff, pickup and the stop
# hook.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE
	unset JOUET_SESSION_ID CLAUDE_CODE_SESSION_ID CODEX_THREAD_ID
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	[[ "$(uname)" != MINGW* && "$(uname)" != MSYS* ]] || skip "jobs run on Unix only"
}

teardown() {
	# Nothing a test started outlives it.
	local -a ids
	mapfile -t ids < <(jouet job ls --json 2>/dev/null | jq -r '.[] | select(.state == "running") | .id' 2>/dev/null)
	[[ ${#ids[@]} -eq 0 ]] || jouet job stop "${ids[@]}" --grace 1s >/dev/null 2>&1 || true
}

@test "start returns once the output says it is ready, and the job runs on" {
	run --separate-stderr jouet job start web --output '^listening$' -- sh -c 'echo booting; sleep 0.3; echo listening; exec sleep 30'

	assert_success
	assert_line --index 0 --regexp '^STARTED job web \(pid [0-9]+\): running, log .*/web\.log matched \([0-9]+s\)$'
	assert_line --index 1 --regexp '^  log: .*/jobs/[0-9a-f]{16}/web\.log$'
	assert_line --index 2 '  next: `jouet job logs web`, `jouet wait job web`, `jouet job stop web`'

	run --separate-stderr jouet job ls --json
	assert_equal "$(jq -r '.[0] | .id, .state' <<<"$output")" $'web\nrunning'
	assert_equal "$(jq -c '.[0].command' <<<"$output")" '["sh","-c","echo booting; sleep 0.3; echo listening; exec sleep 30"]'
	assert_equal "$(jq -c '.[0].ready' <<<"$output")" '{"output":"^listening$"}'

	run --separate-stderr jouet job logs web -n 1
	assert_success
	assert_output $'... 1 earlier lines (full log: '"$(jouet job ls --json | jq -r '.[0].log')"$')\nlistening'

	run --separate-stderr jouet job start web -- true
	assert_failure 4
	assert_equal "$stderr" "jouet: job \`web\` is already running (pid $(jouet job ls --json | jq -r '.[0].pid')); \`jouet job stop web\` stops it, or pass --replace"

	run --separate-stderr jouet job stop web
	assert_success
	assert_output "stopped web"
	assert_equal "$(jouet job ls --json | jq -r '.[0].state')" stopped
	run --separate-stderr jouet job stop web
	assert_output "web had already been stopped"
}

# The live processes in process group $1, from a `ps` every Unix has.
group() {
	ps -A -o pid=,pgid=,stat= | awk -v g="$1" '$2 == g && $3 !~ /^Z/'
}

@test "stop takes the whole process group, children included" {
	jouet job start tree -- sh -c 'sleep 30 & sleep 30 & wait' >/dev/null
	local pgid
	pgid=$(jouet job ls --json | jq -r '.[0].pid')
	[[ $(group "$pgid" | wc -l) -ge 3 ]]

	run --separate-stderr jouet job stop tree
	assert_success

	run group "$pgid"
	assert_output ""
}

@test "a command that fails at once fails the start with the end of its log" {
	run --separate-stderr jouet job start broken --port 1 -- sh -c 'echo "Error: port in use" >&2; exit 3'

	assert_failure 1
	assert_line --index 0 "FAILED job broken: exited 3 after 0s"
	assert_line --index 1 "  Error: port in use"
	assert_line --index 2 --regexp '^  full log: .*/broken\.log$'

	run --separate-stderr jouet job start missing -- jouet-no-such-program
	assert_failure 1
	assert_line --index 0 "FAILED job missing: exited 127 after 0s"
	assert_line --index 1 --regexp '^  jouet-job: .*jouet-no-such-program.*not found'
}

@test "a ready check that never passes leaves the job running and exits 124" {
	run --separate-stderr jouet job start slow --output never --timeout 1s -- sh -c 'echo working; exec sleep 30'

	assert_failure 124
	assert_line --index 0 --regexp '^TIMEOUT job slow: running, log .* following after 1s; it is still running \(`jouet job stop slow` stops it, `jouet wait job slow --until ready` keeps waiting\)$'
	assert_line --index 1 "  working"
	assert_equal "$(jouet job ls --json | jq -r '.[0].state')" running
}

@test "wait job is met when the job exits 0 and fails with its log otherwise" {
	jouet job start ok -- sh -c 'sleep 0.2; echo done' >/dev/null
	run --separate-stderr jouet wait job ok --interval 50ms --timeout 10s
	assert_success
	assert_output "MET job ok: exited 0 (0s)"

	jouet job start tests -- sh -c 'sleep 0.5; for i in $(seq 1 100); do echo "test $i ok"; done; echo "test 101 FAILED"; exit 101' >/dev/null
	run --separate-stderr jouet wait job tests --interval 50ms --timeout 10s --log-lines 2 --json
	assert_failure 1
	assert_equal "$(jq -r '.result, .state' <<<"$output")" $'failed\nexited 101'
	assert_equal "$(jq -c .output <<<"$output")" '["... 99 lines skipped","test 100 ok","test 101 FAILED"]'
	assert_equal "$(jq -r .log_path <<<"$output")" "$(jouet job ls --json | jq -r '.[] | select(.id == "tests") | .log')"

	run --separate-stderr jouet wait job nope
	assert_failure 3
	assert_equal "$stderr" 'jouet: no job `nope` in this work tree; `jouet job ls` lists them'

	run --separate-stderr jouet wait job ok --port 3000
	assert_failure 2
}

@test "wait job --until ready asks the job's own ready check again" {
	jouet job start web --output '^up$' -- sh -c 'echo up; exec sleep 30' >/dev/null
	run --separate-stderr jouet wait job web --until ready --timeout 5s
	assert_success
	assert_line --index 0 --regexp '^MET job web: running, log .*web\.log matched \(0s\)$'
	assert_line --index 1 '  up'
}

@test "jobs belong to the work tree they were started in" {
	init_repo
	commit_file a.txt $'a\n' "init"
	jouet job start here -- sleep 30 >/dev/null
	mkdir -p "$BATS_TEST_TMPDIR/other"
	cd "$BATS_TEST_TMPDIR/other"
	git init -q

	run --separate-stderr jouet job ls
	assert_output "no jobs in this work tree; \`jouet job start ID -- CMD\` starts one"

	cd "$BATS_TEST_TMPDIR/work"
	mkdir -p sub
	cd sub
	run --separate-stderr jouet job ls --json
	assert_equal "$(jq -r '.[].id' <<<"$output")" here
}

@test "handoff records the jobs and pickup says which ended" {
	init_repo
	commit_file a.txt $'a\n' "init"
	jouet job start server -- sleep 30 >/dev/null
	jouet job start build -- sh -c 'exit 2' >/dev/null || true

	run --separate-stderr jouet handoff --no-pr
	assert_success
	assert_line "- **Jobs (2):**"
	assert_line --regexp '^  - `server` running, pid [0-9]+: `sleep 30`; `jouet job logs server`, `jouet wait job server`$'
	assert_line --regexp '^  - `build` exited 2, pid [0-9]+: `sh -c '"'exit 2'"'`; `jouet job logs build`$'
	refute_line --partial "sleep 30\`: attach with"

	run --separate-stderr jouet pickup
	assert_success

	jouet job stop server >/dev/null
	run --separate-stderr jouet pickup --json
	assert_failure 1
	assert_equal "$(jq -r '.drift[].code' <<<"$output")" job_ended
	assert_equal "$(jq -c '.jobs[] | select(.id == "server")' <<<"$output")" '{"id":"server","then":"running","now":"stopped"}'
}

# A Stop hook payload for this directory, in session `s1`.
stop() {
	jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}' | jouet hook claude stop
}

@test "the stop notes the jobs this session started that still run" {
	CLAUDE_CODE_SESSION_ID=s1 jouet job start dev -- sleep 30 >/dev/null
	CLAUDE_CODE_SESSION_ID=s2 jouet job start theirs -- sleep 30 >/dev/null
	CLAUDE_CODE_SESSION_ID=s1 jouet job start finished -- true >/dev/null

	run --separate-stderr stop
	assert_success
	assert_equal "$(jq -r 'keys[]' <<<"$output")" systemMessage
	local note='A job this session started is still running: `dev`. Leave it if it should stay up; `jouet job stop dev` stops it. [jouet: jobs]'
	assert_equal "$(jq -r .systemMessage <<<"$output")" "$note"

	# A failed check still blocks, with the note in its reason.
	jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, hook_event_name: "PostToolUseFailure", tool_name: "Bash",
		tool_input: {command: "cargo test --workspace"}, error: "Exit code 101"}' |
		jouet hook claude post-tool-use-failure
	run --separate-stderr stop
	assert_equal "$(jq -r .decision <<<"$output")" block
	assert_equal "$(jq -r .reason <<<"$output" | tail -n 1)" "$note"

	jouet job stop dev >/dev/null
	rm -f "$XDG_STATE_HOME/jouet/sessions/s1.json"
	run --separate-stderr stop
	assert_output ""
}

@test "the job rules steer background work to jouet job" {
	bash_payload() {
		jq -cn --arg c "$1" '{session_id: "s1", tool_name: "Bash", tool_input: {command: $c}}'
	}
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'nohup python3 -m http.server 8000 > server.log 2>&1 &')"
	assert_equal "$(jq -r .hookSpecificOutput.permissionDecision <<<"$output")" deny
	assert_equal "$(jq -r .hookSpecificOutput.permissionDecisionReason <<<"$output" | cut -d'`' -f2)" "jouet job start http-server --port 8000 -- python3 -m http.server 8000"

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'cargo build & cargo test & wait')"
	assert_output ""

	for rule in job-start job-ready job-logs job-stop; do
		run --separate-stderr jouet rule test "$rule"
		assert_success
		assert_line --regexp "^$rule: ([0-9]+) of \\1 examples passed\$"
	done
}
