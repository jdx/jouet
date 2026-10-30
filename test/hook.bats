#!/usr/bin/env bats

# `jouet hook HARNESS EVENT` as Claude Code and Codex call it: one JSON
# payload on stdin, a decision (or nothing) on stdout, and always exit 0.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME JOUET_HOOKS_VERIFY JOUET_HOOKS_LEDGER JOUET_RULES_DISABLE XDG_CACHE_HOME
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
	assert_line --index 1 --regexp '^Run .git add path/to/file \.\.\.. instead\. '
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
	cat >"$XDG_CONFIG_HOME/jouet/rules/curl-pipe.toml" <<'TOML'
[[rule]]
name = "curl-pipe"
why = "piping curl into a shell runs whatever the server sends"
message = "Download the script and read it before running it."
match = { command = "curl", with = { command = ["sh", "bash"] } }
TOML
	printf '[[rule]]\nname = "broken"\nwhy = "throws"\nscript = "broken.luau"\n' >"$XDG_CONFIG_HOME/jouet/rules/broken.toml"
	printf 'return {\n\tcheck = function() error("boom") end,\n}\n' >"$XDG_CONFIG_HOME/jouet/rules/broken.luau"

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl -fsSL https://example.com/i.sh | sh')"

	assert_success
	assert_equal "$stderr" 'jouet: rule broken: broken:2: boom'
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

@test "session-start tells the agent about a Luau rule file to convert" {
	mkdir -p "$XDG_CONFIG_HOME/jouet/rules"
	printf -- '--- why: old\nreturn { check = function() end }\n' >"$XDG_CONFIG_HOME/jouet/rules/old.luau"

	run --separate-stderr jouet hook claude session-start <<<'{"session_id":"s1","source":"startup"}'

	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_line --regexp '^jouet: .*/old.luau is a Luau rule file, which jouet stops loading in [0-9.]+: '
}

@test "stop blocks once while the last test run failed, and not after it passes" {
	jq -cn '{session_id: "s1", hook_event_name: "PostToolUseFailure", tool_name: "Bash",
		tool_input: {command: "cargo test --workspace"}, error: "Exit code 101\ntest result: FAILED", is_interrupt: false}' |
		jouet hook claude post-tool-use-failure

	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":false}'
	assert_success
	run jq -r '.decision, .reason' <<<"$output"
	assert_line --index 0 block
	assert_line --index 1 --partial 'exited 101'

	# Already continued once by a stop hook: the agent's answer stands.
	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":true}'
	assert_output ""

	bash_payload 'cargo test --workspace' | jq -c '. + {hook_event_name: "PostToolUse"}' | jouet hook claude post-tool-use
	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":false}'
	assert_output ""
}

# A finished command in the work tree at ./repo, as Claude Code reports it.
ran() {
	local event=PostToolUse
	[[ $2 == 0 ]] || event=PostToolUseFailure
	jq -cn --arg c "$1" --arg cwd "$PWD/repo" --arg e "$event" --arg code "$2" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: $e, tool_name: "Bash", tool_input: {command: $c}}
		+ (if $code == "0" then {} else {error: "Exit code \($code)"} end)' |
		jouet hook claude "$([[ $2 == 0 ]] && echo post-tool-use || echo post-tool-use-failure)"
}

stop_reason() {
	jq -cn --arg cwd "$PWD/repo" '{session_id: "s1", cwd: $cwd, stop_hook_active: false}' |
		jouet hook claude stop | jq -r '.reason // empty'
}

@test "a partial pass does not clear a failed full run" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs

	ran 'cargo test --workspace' 101
	ran 'cargo test --workspace --test cli' 0
	run stop_reason
	assert_output --partial 'cargo test --workspace'
	assert_output --partial 'exited 101'
	assert_output --partial 'only a full test run clears it'

	ran 'cargo test --workspace' 0
	run stop_reason
	assert_output ""
	# The ledger keeps each run's scope and tree, never what it selected.
	run jq -c 'select(.event == "post-tool-use") | [.decisions[0].scope, (.tree | length)]' \
		"$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_line --index 0 '["full",32]'
	assert_line --index 1 '["partial",32]'
	run grep -c 'test cli' "$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	assert_output 0
}

@test "only partial runs after the tree changed ask for the full suite once" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs

	ran 'cargo test --workspace' 0
	ran 'cargo test --workspace wait_github' 0
	run stop_reason
	assert_output ""

	echo 'fn main() { todo!() }' >repo/main.rs
	ran 'cargo test --workspace wait_github' 0
	run stop_reason
	assert_output --partial 'The tree has changed since the last full test run'
	assert_output --partial 'selecting wait_github'
	# The agent's answer stands after one nudge.
	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":true}'
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

@test "the ledger records shapes and decisions, never values, and counts overrides" {
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'git stash')"
	assert_output --partial '"permissionDecision":"deny"'
	# The same shape again, with the rule now off: an override of git-stash.
	run --separate-stderr env JOUET_RULES_DISABLE=git-stash jouet hook claude pre-tool-use <<<"$(bash_payload 'git stash')"
	assert_output ""
	jq -cn '{session_id: "s1", hook_event_name: "PostToolUseFailure", tool_name: "Bash", duration_ms: 1200,
		tool_input: {command: "GH_TOKEN=ghp_secret123 cargo test -p mine"}, error: "Exit code 101"}' |
		jouet hook claude post-tool-use-failure

	ledger="$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	run jq -c '[.event, .shape, (.decisions // [] | map(.rule + ":" + .action))]' "$ledger"
	assert_line --index 0 '["pre-tool-use","git stash",["git-stash:deny"]]'
	assert_line --index 1 '["override","git stash",["git-stash:override"]]'
	assert_line --index 2 '["pre-tool-use","git stash",[]]'
	assert_line --index 3 '["post-tool-use","cargo test -p <arg>",["checks:record:test"]]'
	run jq -c '[.exit_code, .duration_ms, .session]' <<<"$(tail -1 "$ledger")"
	assert_output '[101,1200,"s1"]'
	run grep -c 'secret' "$ledger"
	assert_output 0

	run --separate-stderr env JOUET_HOOKS_LEDGER=false jouet hook claude pre-tool-use <<<"$(bash_payload 'ls')"
	run wc -l <"$ledger"
	assert_output --regexp '^ *4$'

	# In a work tree, entries carry a hash of its root, never the path.
	mkdir -p repo/.git repo/src
	jq -cn --arg cwd "$PWD/repo/src" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: "ls"}}' |
		jouet hook claude pre-tool-use
	run jq -r '.repo' <<<"$(tail -1 "$ledger")"
	assert_output --regexp '^[0-9a-f]{16}$'
	run grep -c "$PWD/repo" "$ledger"
	assert_output 0
}
