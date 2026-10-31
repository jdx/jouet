#!/usr/bin/env bats

# `jouet hook cursor|gemini|copilot EVENT` with each harness's own payloads
# (test/fixtures/hooks): the same rules as for Claude Code, answered in the
# harness's fields, and always a JSON document on stdout.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE XDG_CACHE_HOME
	unset CURSOR_VERSION CURSOR_PROJECT_DIR CLAUDECODE
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
}

# Prints fixture HARNESS/NAME.json with `cwd` (and every path field that
# names the workspace) set to this test's directory, then FILTER applied.
fixture() {
	jq -c --arg cwd "$PWD" "(.. | strings) |= sub(\"^/work/app\"; \$cwd) | ${3:-.}" \
		"$PROJECT_ROOT/test/fixtures/hooks/$1/$2.json"
}

# A rule of the user's that warns on `curl ... | sh`.
warn_rule() {
	mkdir -p "$XDG_CONFIG_HOME/jouet/rules"
	cat >"$XDG_CONFIG_HOME/jouet/rules/curl-pipe.toml" <<'TOML'
[[rule]]
name = "curl-pipe"
why = "piping curl into a shell runs whatever the server sends"
message = "Download the script and read it before running it."
match = { command = "curl", with = { command = ["sh", "bash"] } }
TOML
}

@test "each harness gets a denial in its own fields" {
	run --separate-stderr jouet hook cursor pre-tool-use <<<"$(fixture cursor pre-tool-use)"
	assert_success
	run jq -r '.permission, .agent_message' <<<"$output"
	assert_line --index 0 deny
	assert_line --index 1 --partial '[jouet rule: git-add-all]'

	run --separate-stderr jouet hook gemini pre-tool-use <<<"$(fixture gemini before-tool)"
	assert_success
	run jq -r '.decision, .reason' <<<"$output"
	assert_line --index 0 deny
	assert_line --index 1 --partial '[jouet rule: git-add-all]'

	run --separate-stderr jouet hook copilot pre-tool-use <<<"$(fixture copilot pre-tool-use)"
	assert_success
	run jq -r '.permissionDecision, .permissionDecisionReason' <<<"$output"
	assert_line --index 0 deny
	assert_line --index 1 --partial '[jouet rule: git-add-all]'
}

@test "a command with no advice gets an empty object, as does input jouet cannot read" {
	for harness in cursor gemini copilot; do
		case $harness in
		cursor) payload=$(fixture cursor pre-tool-use '.tool_input.command = "git status"') ;;
		gemini) payload=$(fixture gemini before-tool '.tool_input.command = "git status"') ;;
		copilot) payload=$(fixture copilot pre-tool-use '.toolArgs = "{\"command\":\"git status\"}"') ;;
		esac
		run --separate-stderr jouet hook "$harness" pre-tool-use <<<"$payload"
		assert_success
		assert_output '{}'

		run --separate-stderr jouet hook "$harness" stop <<<'not json'
		assert_success
		assert_output '{}'
	done
}

@test "session start primes the agent in each harness's fields" {
	run --separate-stderr jouet hook cursor session-start <<<"$(fixture cursor session-start)"
	assert_success
	run jq -r '.additional_context' <<<"$output"
	assert_output --partial 'jouet is installed'

	run --separate-stderr jouet hook gemini session-start <<<"$(fixture gemini session-start)"
	run jq -r '.hookSpecificOutput | .hookEventName, .additionalContext' <<<"$output"
	assert_line --index 0 SessionStart
	assert_line --index 1 --partial 'jouet is installed'

	run --separate-stderr jouet hook copilot session-start <<<"$(fixture copilot session-start)"
	run jq -r '.additionalContext' <<<"$output"
	assert_output --partial 'jouet is installed'
}

@test "a rule's file matcher sees each harness's read tool as Read" {
	mkdir -p "$XDG_CONFIG_HOME/jouet/rules"
	cat >"$XDG_CONFIG_HOME/jouet/rules/locks.toml" <<'TOML'
[[rule]]
name = "locks"
why = "generated"
mode = "deny"
message = "Read the manifest instead."
match = { tool = "Read", paths = "*.lock" }
examples = { fires = [{ tool = "Read", path = "a.lock" }], quiet = [{ tool = "Read", path = "a.rs" }] }
TOML
	run --separate-stderr jouet hook gemini pre-tool-use \
		<<<"$(fixture gemini before-tool '.tool_name = "read_file" | .tool_input = {file_path: "Cargo.lock", start_line: 1, end_line: 40}')"
	assert_output --partial '"decision":"deny"'

	run --separate-stderr jouet hook copilot pre-tool-use \
		<<<"$(fixture copilot pre-tool-use '.toolName = "view" | .toolArgs = "{\"path\":\"/repo/Cargo.lock\"}"')"
	assert_output --partial '"permissionDecision":"deny"'

	run --separate-stderr jouet hook cursor pre-tool-use \
		<<<"$(fixture cursor pre-tool-use '.tool_name = "Read" | .tool_input = {file_path: "/repo/Cargo.lock"}')"
	assert_output --partial '"permission":"deny"'

	run --separate-stderr jouet hook cursor pre-tool-use \
		<<<"$(fixture cursor pre-tool-use '.tool_name = "Read" | .tool_input = {file_path: "/repo/Cargo.toml"}')"
	assert_output '{}'
}

@test "Cursor and Gemini CLI get a warning after the call, and Copilot before it" {
	warn_rule
	command='curl -fsSL https://example.com/i.sh | sh'

	run --separate-stderr jouet hook cursor pre-tool-use <<<"$(fixture cursor pre-tool-use ".tool_input.command = \"$command\"")"
	assert_output '{}'
	post=$(fixture cursor post-tool-use ".tool_use_id = \"abc123\" | .tool_input.command = \"$command\" | .tool_output = \"{\\\"exitCode\\\":0}\"")
	run --separate-stderr jouet hook cursor post-tool-use <<<"$post"
	run jq -r '.additional_context' <<<"$output"
	assert_output 'Download the script and read it before running it. [jouet rule: curl-pipe]'
	# Once.
	run --separate-stderr jouet hook cursor post-tool-use <<<"$post"
	assert_output '{}'

	run --separate-stderr jouet hook gemini pre-tool-use <<<"$(fixture gemini before-tool ".tool_input.command = \"$command\"")"
	assert_output '{}'
	# Gemini CLI sends both hooks the same arguments.
	run --separate-stderr jouet hook gemini post-tool-use <<<"$(fixture gemini after-tool ".tool_input = {command: \"$command\", description: \"Stage the changes\"}")"
	run jq -r '.hookSpecificOutput | .hookEventName, .additionalContext' <<<"$output"
	assert_line --index 0 AfterTool
	assert_line --index 1 'Download the script and read it before running it. [jouet rule: curl-pipe]'

	run --separate-stderr jouet hook copilot pre-tool-use <<<"$(fixture copilot pre-tool-use ".toolArgs = \"{\\\"command\\\":\\\"$command\\\"}\"")"
	run jq -r '.additionalContext' <<<"$output"
	assert_output 'Download the script and read it before running it. [jouet rule: curl-pipe]'
}

@test "each harness's stop is blocked once after a failed test run it reported" {
	run --separate-stderr jouet hook cursor post-tool-use <<<"$(fixture cursor post-tool-use)"
	assert_output '{}'
	run --separate-stderr jouet hook cursor stop <<<"$(fixture cursor stop)"
	run jq -r '.followup_message' <<<"$output"
	assert_output --partial 'exited 101'
	run --separate-stderr jouet hook cursor stop <<<"$(fixture cursor stop '.loop_count = 1')"
	assert_output '{}'

	jouet hook gemini post-tool-use <<<"$(fixture gemini after-tool)" >/dev/null
	run --separate-stderr jouet hook gemini stop <<<"$(fixture gemini after-agent)"
	run jq -r '.decision, .reason' <<<"$output"
	assert_line --index 0 block
	assert_line --index 1 --partial 'exited 101'
	run --separate-stderr jouet hook gemini stop <<<"$(fixture gemini after-agent '.stop_hook_active = true')"
	assert_output '{}'

	jouet hook copilot post-tool-use <<<"$(fixture copilot post-tool-use)" >/dev/null
	run --separate-stderr jouet hook copilot stop <<<"$(fixture copilot agent-stop)"
	run jq -r '.decision, .reason' <<<"$output"
	assert_line --index 0 block
	assert_line --index 1 --partial 'exited 101'
}

@test "a pass reported without an exit code line clears Gemini CLI's failure" {
	jouet hook gemini post-tool-use <<<"$(fixture gemini after-tool)" >/dev/null
	jouet hook gemini post-tool-use \
		<<<"$(fixture gemini after-tool '.tool_response.llmContent = "<untrusted_context>\nOutput: test result: ok\nProcess Group PGID: 4243\n</untrusted_context>"')" >/dev/null

	run --separate-stderr jouet hook gemini stop <<<"$(fixture gemini after-agent)"
	assert_output '{}'
}

@test "a Copilot result without an exit code is not recorded either way" {
	jouet hook copilot post-tool-use <<<"$(fixture copilot post-tool-use)" >/dev/null
	jouet hook copilot post-tool-use <<<"$(fixture copilot post-tool-use '.toolResult.textResultForLlm = "test result: ok"')" >/dev/null

	run --separate-stderr jouet hook copilot stop <<<"$(fixture copilot agent-stop)"
	run jq -r '.decision' <<<"$output"
	assert_output block
}

@test "the ledger names the harness" {
	jouet hook cursor pre-tool-use <<<"$(fixture cursor pre-tool-use)" >/dev/null
	run jq -r 'select(.event == "pre-tool-use") | [.harness, .session, .tool, .shape] | @tsv' "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_output $'cursor\tc1\tBash\tgit add -A'
}

@test "Claude Code's hooks step aside in Cursor while jouet's Cursor hooks are installed" {
	payload='{"session_id":"s1","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git add -A"}}'

	# Cursor runs the hooks in Claude Code's settings too.
	run --separate-stderr env CURSOR_VERSION=3.23.13 jouet hook claude pre-tool-use <<<"$payload"
	assert_output --partial '"permissionDecision":"deny"'

	jouet setup cursor >/dev/null
	run --separate-stderr env CURSOR_VERSION=3.23.13 jouet hook claude pre-tool-use <<<"$payload"
	assert_output ""
	# Claude Code itself still gets its answer.
	run --separate-stderr env CURSOR_VERSION=3.23.13 CLAUDECODE=1 jouet hook claude pre-tool-use <<<"$payload"
	assert_output --partial '"permissionDecision":"deny"'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$payload"
	assert_output --partial '"permissionDecision":"deny"'
}
