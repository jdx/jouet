#!/usr/bin/env bats
# Messages name commands in backquotes, which the tests match literally.
# shellcheck disable=SC2016

# jouet with pitchfork, for the long-running services a project declares:
# `jouet wait daemon`, the daemons in `jouet handoff` and `jouet pickup`, a
# stop hook that leaves them alone, and the `pitchfork-*` rules. A fake
# `pitchfork` on PATH records its arguments and answers with the JSON shapes
# pitchfork 2.30 prints.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE XDG_CACHE_HOME
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	export FAKE_PITCHFORK="$BATS_TEST_TMPDIR/pitchfork"
	mkdir -p "$FAKE_PITCHFORK/status" "$FAKE_PITCHFORK/logs" "$BATS_TEST_TMPDIR/fake"
	cat >"$BATS_TEST_TMPDIR/fake/pitchfork" <<'SH'
#!/usr/bin/env bash
# Records each call, then answers from $FAKE_PITCHFORK: status/<id> holds one
# status JSON document per line, and each call takes the next until the
# last; list.json and list-project.json answer `list`; logs/<id> is the log.
printf '%s\t%s\n' "$PWD" "$*" >>"$FAKE_PITCHFORK/calls"
key() { printf '%s' "${1//\//--}"; }
case "$1" in
status)
	f="$FAKE_PITCHFORK/status/$(key "$2")"
	if [[ ! -f $f ]]; then
		echo "Error: pitchfork::deps::not_found (https://pitchfork.jdx.dev/configuration#depends)" >&2
		echo "" >&2
		echo "  × daemon '$2' not found in configuration" >&2
		exit 1
	fi
	head -n 1 "$f"
	if [[ $(wc -l <"$f") -gt 1 ]]; then
		tail -n +2 "$f" >"$f.next" && mv "$f.next" "$f"
	fi
	;;
list)
	if [[ " $* " == *" --project "* ]]; then
		cat "$FAKE_PITCHFORK/list-project.json"
	else
		cat "$FAKE_PITCHFORK/list.json"
	fi
	;;
logs) cat "$FAKE_PITCHFORK/logs/$(key "$2")" 2>/dev/null ;;
*) exit 0 ;;
esac
SH
	chmod +x "$BATS_TEST_TMPDIR/fake/pitchfork"
	export PATH="$BATS_TEST_TMPDIR/fake:$PATH"
}

# `pitchfork status ID --json` as pitchfork prints it, on one line.
status_json() {
	local id=$1 status=$2 pid=${3:-null} port=${4:-}
	jq -cn --arg id "$id" --arg status "$status" --argjson pid "$pid" --arg port "$port" '{
		id: $id, namespace: ($id | split("/")[0]), name: ($id | split("/")[1]), pid: $pid,
		status: $status, oneshot: false,
		port: (if $port == "" then [] else [$port | tonumber] end), proxy_url: null, url: null}'
}

# One entry of `pitchfork list --json`.
list_entry() {
	local id=$1 status=$2 pid=${3:-null} error=${4:-}
	jq -cn --arg id "$id" --arg status "$status" --argjson pid "$pid" --arg error "$error" '{
		id: $id, namespace: ($id | split("/")[0]), name: ($id | split("/")[1]), pid: $pid,
		status: $status, oneshot: false, disabled: false, available: false,
		proxy_url: null, url: null, error: (if $error == "" then null else $error end), port: []}'
}

# Each argument is one `status` answer for daemon `myproj/api`, in order.
api_statuses() {
	: >"$FAKE_PITCHFORK/status/api"
	for s in "$@"; do
		status_json myproj/api "$s" "$([[ $s == running ]] && echo 4242 || echo null)" >>"$FAKE_PITCHFORK/status/api"
	done
}

@test "wait daemon is met once the daemon runs, asking pitchfork here" {
	api_statuses waiting restarting running

	run --separate-stderr jouet wait daemon api --interval 10ms --timeout 10s

	assert_success
	assert_output "MET daemon api: running (0s)"
	assert_equal "$(cut -f1 "$FAKE_PITCHFORK/calls" | sort -u)" "$PWD"
	assert_equal "$(cut -f2 "$FAKE_PITCHFORK/calls" | sort -u)" "status api --json"
	assert_equal "$(wc -l <"$FAKE_PITCHFORK/calls" | tr -d ' ')" 3
	[[ $stderr == *"daemon api: waiting"*"daemon api: restarting"*"daemon api: running"* ]]
}

@test "a running daemon is ready only once its port answers" {
	status_json myproj/api running 4242 1 >"$FAKE_PITCHFORK/status/api"

	# Port 1 refuses connections on any machine the tests run on.
	run --separate-stderr jouet wait daemon api --timeout 0 --no-logs
	assert_failure 124
	assert_output --regexp '^TIMEOUT daemon api: running, port localhost:1 (closed|unreachable)'

	run --separate-stderr jouet wait daemon api --http http://127.0.0.1:1/ --timeout 0 --no-logs
	assert_failure 124
	assert_output --regexp '^TIMEOUT daemon api: running, http http://127\.0\.0\.1:1/ unreachable'
}

@test "an errored daemon fails the wait at once with the end of its log" {
	api_statuses errored
	printf '%s\n' "$(list_entry myproj/api errored null 'exit code 1')" | jq -s . >"$FAKE_PITCHFORK/list.json"
	for i in $(seq 1 100); do echo "request $i ok"; done >"$FAKE_PITCHFORK/logs/myproj--api"
	echo "Error: listen EADDRINUSE: address already in use :::3000" >>"$FAKE_PITCHFORK/logs/myproj--api"

	run --separate-stderr jouet wait daemon api --log-lines 3

	assert_failure 1
	assert_line --index 0 "FAILED daemon api: errored (exit code 1) after 0s"
	assert_line --index 1 "  ... 98 lines skipped"
	assert_line --index 2 "  request 99 ok"
	assert_line --index 3 "  request 100 ok"
	assert_line --index 4 "  Error: listen EADDRINUSE: address already in use :::3000"
	assert_line --index 5 --regexp "^  full log: $JOUET_LOG_DIR/wait-daemon-myproj-api-[0-9]+-[0-9]+\.log\$"
	local log=${lines[5]#  full log: }
	assert_equal "$(wc -l <"$log" | tr -d ' ')" 101
	assert_equal "$(grep -c "logs myproj/api --raw --no-pager" "$FAKE_PITCHFORK/calls")" 1
}

@test "a timeout shows the end of the log and exits 124" {
	api_statuses waiting
	printf 'compiling\nstill compiling\n' >"$FAKE_PITCHFORK/logs/myproj--api"

	run --separate-stderr jouet wait daemon api --timeout 0 --json

	assert_failure 124
	assert_equal "$(jq -r '.result, .state' <<<"$output")" $'timeout\nwaiting'
	assert_equal "$(jq -c .output <<<"$output")" '["compiling","still compiling"]'
	assert [ -f "$(jq -r .log_path <<<"$output")" ]
}

@test "until stopped is met by a stopped or completed daemon" {
	api_statuses running stopping stopped
	run --separate-stderr jouet wait daemon api --until stopped --interval 10ms
	assert_success
	assert_output "MET daemon api: stopped (0s)"

	api_statuses completed
	run --separate-stderr jouet wait daemon api --until stopped
	assert_success
}

@test "an unknown daemon is exit 3, and no pitchfork is exit 4" {
	run --separate-stderr jouet wait daemon nope
	assert_failure 3
	assert_equal "$stderr" "jouet: pitchfork has no daemon \`nope\` here (daemon 'nope' not found in configuration); \`pitchfork list --project\` lists them"

	run --separate-stderr env PATH="$BATS_TEST_TMPDIR/bin" jouet wait daemon api --json
	assert_failure 4
	assert_equal "$(jq -r .error.code <<<"$output")" precondition
	assert_output --partial "pitchfork is not installed"
}

@test "wait daemon refuses arguments that do not go together" {
	run --separate-stderr jouet wait daemon api --until running
	assert_failure 2
	assert_equal "$stderr" "jouet: --until must be ready or stopped, not \`running\`"

	run --separate-stderr jouet wait daemon api --port 3000 --http http://localhost:3000
	assert_failure 2

	run --separate-stderr jouet wait daemon api --until stopped --port 3000
	assert_failure 2
	[[ ! -f "$FAKE_PITCHFORK/calls" ]]
}

@test "handoff records the project's daemons and pickup says which stopped or failed" {
	init_repo
	commit_file pitchfork.toml $'[daemons.api]\nrun = "npm run dev"\n' "init"
	{
		list_entry myproj/api running 4242
		list_entry myproj/worker running 4343
		list_entry myproj/old stopped
		list_entry global/mise-history running 99
	} | jq -s . >"$FAKE_PITCHFORK/list-project.json"

	run --separate-stderr jouet handoff --no-pr
	assert_success
	assert_line "- **pitchfork daemons (2):**"
	assert_line "  - \`myproj/api\` running, pid 4242: \`pitchfork logs myproj/api -n 50\`, \`jouet wait daemon myproj/api\`"
	assert_line "  - \`myproj/worker\` running, pid 4343: \`pitchfork logs myproj/worker -n 50\`, \`jouet wait daemon myproj/worker\`"
	refute_output --partial "myproj/old"
	refute_output --partial "global/"
	assert_equal "$(grep -c "list --project --json" "$FAKE_PITCHFORK/calls")" 1

	run --separate-stderr jouet pickup
	assert_success
	assert_line "- \`myproj/api\`: running, was running"

	{
		list_entry myproj/api errored null "exit code 1"
		list_entry myproj/worker stopped
	} | jq -s . >"$FAKE_PITCHFORK/list-project.json"
	run --separate-stderr jouet pickup --json
	assert_failure 1
	assert_equal "$(jq -r '.drift[].code' <<<"$output")" $'daemon_failed\ndaemon_stopped'
	assert_equal "$(jq -c '.daemons[0]' <<<"$output")" '{"id":"myproj/api","then":"running","now":"errored","error":"exit code 1"}'
}

@test "handoff without pitchfork says nothing about daemons" {
	init_repo
	commit_file a.txt $'a\n' "init"

	run --separate-stderr env PATH="$BATS_TEST_TMPDIR/bin:/usr/bin:/bin" jouet handoff --no-pr

	assert_success
	refute_output --partial "pitchfork"
}

@test "handoff lists jobs and pitchfork daemons apart" {
	init_repo
	commit_file pitchfork.toml $'[daemons.api]\nrun = "npm run dev"\n' "init"
	list_entry myproj/api running 4242 | jq -s . >"$FAKE_PITCHFORK/list-project.json"
	jouet job start build -- sleep 30 >/dev/null

	run --separate-stderr jouet handoff --no-pr
	jouet job stop build >/dev/null
	assert_success
	assert_line "- **Jobs (1):**"
	assert_line "- **pitchfork daemons (1):**"
}

@test "the stop leaves pitchfork daemons alone" {
	status_json myproj/api running 4242 >"$FAKE_PITCHFORK/status/api"
	jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, hook_event_name: "PostToolUse", tool_name: "Bash",
		tool_input: {command: "pitchfork start api"}}' | jouet hook claude post-tool-use

	run --separate-stderr bash -c "jq -cn --arg cwd '$PWD' '{session_id: \"s1\", cwd: \$cwd, stop_hook_active: false}' | jouet hook claude stop"

	assert_success
	assert_output ""
	[[ ! -f "$FAKE_PITCHFORK/calls" ]]
}

# A pre-tool-use payload for COMMAND in this directory.
bash_payload() {
	jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
}

# The rule that denied COMMAND, and the command its advice names.
denied() {
	jouet hook claude pre-tool-use <<<"$(bash_payload "$1")" |
		jq -r '.hookSpecificOutput | select(.permissionDecision == "deny") | .permissionDecisionReason' |
		sed -E 's/.*\[jouet rule: ([a-z-]+)\]$/\1/; t; s/.*//'
}

@test "a daemon pitchfork.toml declares is started and stopped through pitchfork" {
	printf '[daemons.web]\nrun = "npm run dev"\nport = 5173\n\n[daemons.api]\nrun = "cd api && exec uvicorn app:app --port 8000"\n' >pitchfork.toml
	mkdir -p web
	cd web

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'npm run dev &')"
	assert_equal "$(jq -r .hookSpecificOutput.permissionDecisionReason <<<"$output" | cut -d'`' -f1-4)" '`npm run dev` is the `web'

	run denied 'uvicorn app:app --port 8000 --reload'
	assert_output pitchfork-start
	run denied 'kill $(lsof -ti :5173)'
	assert_output pitchfork-stop
	run denied "pkill -f 'npm run dev'"
	assert_output pitchfork-stop
	# Not a declared daemon: one-off work, for jouet job.
	run denied 'python3 -m http.server 9000 &'
	assert_output job-start
	run denied 'npm run build'
	assert_output ""
}

@test "without a pitchfork config, the pitchfork rules stay out of the way" {
	run denied 'npm run dev &'
	assert_output job-start
	run denied 'kill $(lsof -ti :5173)'
	assert_output ""
}

@test "the primer names the pitchfork rules only in a project with a pitchfork config" {
	primer() {
		jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, hook_event_name: "SessionStart"}' |
			jouet hook claude session-start | jq -r .hookSpecificOutput.additionalContext
	}
	run --separate-stderr primer
	assert_output --partial "job-start"
	refute_output --partial "pitchfork-start"

	touch pitchfork.toml
	run --separate-stderr primer
	assert_output --partial "pitchfork-start"
}

@test "each pitchfork rule passes its examples" {
	for rule in pitchfork-start pitchfork-stop pitchfork-logs; do
		run --separate-stderr jouet rule test "$rule"
		assert_success
		assert_line --regexp "^$rule: [0-9]+ of [0-9]+ examples passed\$"
	done
}
