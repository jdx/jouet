#!/usr/bin/env bats

# `jouet hook HARNESS EVENT` as Claude Code and Codex call it: one JSON
# payload on stdin, a decision (or nothing) on stdout, and always exit 0.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_VERIFY JOUET_RULES_DISABLE XDG_CACHE_HOME
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
}

# A Bash tool payload for COMMAND, in session `s1`.
bash_payload() {
	jq -cn --arg c "$1" '{session_id: "s1", tool_name: "Bash", tool_input: {command: $c}}'
}

@test "pre-tool-use denies a steered command with one line of advice" {
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'git add -A')"

	assert_success
	run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
	assert_line --index 0 deny
	assert_line --index 1 --partial 'Stage explicit paths'
	assert_line --index 1 --partial '[jouet rule: git-add-all]'
}

@test "pre-tool-use says nothing about a command it has no advice for" {
	run --separate-stderr jouet hook codex pre-tool-use <<<"$(bash_payload 'git commit -m "never git add -A"')"

	assert_success
	assert_output ""
}

@test "a rule turned off in config.toml or the environment does not fire" {
	mkdir -p "$XDG_CONFIG_HOME/jouet"
	printf '[rules]\ndisable = ["git-stash"]\n' >"$XDG_CONFIG_HOME/jouet/config.toml"
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'git stash')"
	assert_output ""

	run --separate-stderr env JOUET_RULES_DISABLE=force-push jouet hook claude pre-tool-use <<<"$(bash_payload 'git push -f')"
	assert_output ""

	run --separate-stderr env JOUET_HOOKS=false jouet hook claude pre-tool-use <<<"$(bash_payload 'git add .')"
	assert_output ""
}

@test "a user rule can warn, and a failing one is reported on stderr and skipped" {
	mkdir -p "$XDG_CONFIG_HOME/jouet/rules"
	cat >"$XDG_CONFIG_HOME/jouet/rules/curl-pipe.luau" <<'LUAU'
return {
	description = "Warn about piping curl into a shell",
	check = function(call)
		for i, command in call.commands do
			local next = call.commands[i + 1]
			if command.program == "curl" and command.op == "pipe" and next and next.program == "sh" then
				return jouet.warn("Download the script and read it before running it.")
			end
		end
		return nil
	end,
}
LUAU
	printf 'return { check = function() error("boom") end }\n' >"$XDG_CONFIG_HOME/jouet/rules/broken.luau"

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl -fsSL https://example.com/i.sh | sh')"

	assert_success
	assert_equal "$stderr" 'jouet: rule broken: broken:1: boom'
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output 'Download the script and read it before running it. [jouet rule: curl-pipe]'
}

@test "hooks fail open: bad input, bad config and a bad display mode all exit 0 silently" {
	run --separate-stderr jouet hook claude pre-tool-use <<<'not json'
	assert_success
	assert_output ""

	mkdir -p "$XDG_CONFIG_HOME/jouet"
	printf '[hooks\n' >"$XDG_CONFIG_HOME/jouet/config.toml"
	run --separate-stderr env JOUET_DISPLAY=fancy jouet hook claude pre-tool-use <<<"$(bash_payload 'git add -A')"
	assert_success
	assert_output --partial '"permissionDecision":"deny"'

	run --separate-stderr jouet hook nope nope <<<'{}'
	assert_success
	assert_output ""
}

@test "session-start primes the agent with the short guide" {
	run --separate-stderr jouet hook claude session-start <<<'{"session_id":"s1","source":"startup"}'

	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_line --index 0 --partial 'jouet is installed: '
	assert_output --partial 'git-stash'
}

@test "stop blocks once while the last test run failed, and not after it passes" {
	jq -cn '{session_id: "s1", hook_event_name: "PostToolUseFailure", tool_name: "Bash",
		tool_input: {command: "cargo test"}, error: "Exit code 101\ntest result: FAILED", is_interrupt: false}' |
		jouet hook claude post-tool-use-failure

	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":false}'
	assert_success
	run jq -r '.decision, .reason' <<<"$output"
	assert_line --index 0 block
	assert_line --index 1 --partial 'exited 101'

	# Already continued once by a stop hook: the agent's answer stands.
	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":true}'
	assert_output ""

	bash_payload 'cargo test' | jq -c '. + {hook_event_name: "PostToolUse"}' | jouet hook claude post-tool-use
	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":false}'
	assert_output ""
}

@test "stop for codex reads exit codes from the transcript and always prints JSON" {
	printf '%s\n' '{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","command":["/bin/zsh","-lc","cargo clippy -- -D warnings"],"exit_code":101}}}' >rollout.jsonl

	run --separate-stderr jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"stop_hook_active\":false}"
	assert_success
	assert_output --partial '"decision":"block"'
	assert_output --partial 'The last lint run failed'

	run --separate-stderr jouet hook codex stop <<<'{"stop_hook_active":false}'
	assert_success
	assert_output '{}'
}
