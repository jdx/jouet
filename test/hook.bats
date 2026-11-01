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
	assert_line --index 0 --partial 'jouet is installed, a CLI toolkit for coding agents: '
	assert_output --partial "python-edit (use \`jouet edit\`)"
	assert_output --partial 'git-stash'
	refute_output --partial 'large-read'
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

@test "a change after the last passing test run asks for a test run once" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs

	ran 'cargo test --workspace' 0
	ran 'cargo test --workspace wait_github' 0
	run stop_reason
	assert_output ""

	echo 'fn main() { todo!() }' >repo/main.rs
	run stop_reason
	assert_output --partial 'Files changed after the last test run passed ('
	assert_output --partial 'cargo test --workspace wait_github'
	# The agent's answer stands after one nudge.
	run --separate-stderr jouet hook claude stop <<<'{"session_id":"s1","stop_hook_active":true}'
	assert_output ""
	# Part of the suite passing at the files now is enough: CI runs the rest.
	ran 'cargo test --workspace wait_github' 0
	run stop_reason
	assert_output ""
}

@test "a failure is raised once per tree and runs, not at every turn's stop" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs

	ran 'cargo test --workspace brew::cask' 101
	run stop_reason
	assert_output --partial 'exited 101'
	# The next turn ends at the same tree, with no run since: the agent has
	# already said why it cannot pass.
	run stop_reason
	assert_output ""

	# The tree changed: raised again, then quiet again.
	echo 'fn main() { todo!() }' >repo/main.rs
	run stop_reason
	assert_output --partial 'exited 101'
	run stop_reason
	assert_output ""

	# A new run, even the same failure at the same tree, raises it again.
	ran 'cargo test --workspace brew::cask' 101
	run stop_reason
	assert_output --partial 'exited 101'
}

@test "a jouet test run still going holds up nothing, and its end restores the check" {
	git init -q repo
	ran 'cargo test --workspace' 101

	# The agent waits on a run it started in the background: the failure
	# before it is not raised while the run goes.
	(cd repo && CLAUDE_CODE_SESSION_ID=s1 jouet test --cmd -- sh -c 'sleep 3' >/dev/null 2>&1 &)
	state="$XDG_STATE_HOME/jouet/sessions/s1.json"
	for _ in $(seq 50); do
		jq -e '.trees[].running' "$state" >/dev/null 2>&1 && break
		sleep 0.1
	done
	run stop_reason
	assert_output ""

	# Once it has ended, the failure it did not clear holds up finishing again.
	for _ in $(seq 100); do
		jq -e '[.trees[].running] | all(. == null)' "$state" >/dev/null 2>&1 && break
		sleep 0.1
	done
	run stop_reason
	assert_output --partial 'exited 101'
}

@test "a failure stops counting when the files are back to ones that passed" {
	git init -q repo
	echo 'fixed' >repo/a.go
	git -C repo add a.go

	# The test passes with the fix, then fails with it taken out to prove the
	# test: at the failing files, finishing waits.
	ran 'go test -run TestA .' 0
	echo 'unfixed' >repo/a.go
	ran 'go test -run TestA .' 1
	run stop_reason
	assert_output --partial 'The last test run failed'

	# With the fix back, the files are the ones that passed.
	echo 'fixed' >repo/a.go
	run stop_reason
	assert_output ""
}

@test "a run after a cd counts for the work tree it went to" {
	git init -q repo
	git init -q other
	echo 'fn main() {}' >other/main.rs
	git -C other add main.rs

	# From ./repo, the session tests ./other: its full run counts there, and
	# part of the suite after it asks for nothing.
	ran "cd $PWD/other && cargo test --workspace" 0
	ran "cd $PWD/other && bats test/a.bats" 0
	run stop_reason
	assert_output ""

	# After the tree there changes with no run since, it asks for one,
	# naming the tree.
	echo 'fn main() { todo!() }' >other/main.rs
	run stop_reason
	assert_output --partial "In $(cd other && pwd -P): Files changed after the last test run passed"

	# A cd it cannot follow records nothing, in either tree.
	ran "cd $PWD/other && cargo test --workspace" 0
	# shellcheck disable=SC2016 # the $DIR is the command's, unexpanded
	ran 'cd "$DIR" && cargo test --workspace' 101
	run stop_reason
	assert_output ""
}

@test "a whole-tree lint warns while the tree has changes, not on a clean checkout" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	git -C repo -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -qm init
	lint() {
		jq -cn --arg cwd "$PWD/repo" '{session_id: "s1", cwd: $cwd, tool_name: "Bash",
			tool_input: {command: "cargo clippy --workspace 2>&1 | head -20"}}' |
			jouet hook claude pre-tool-use
	}

	# Freshly merged or checked out: nothing to iterate on.
	run --separate-stderr lint
	assert_success
	assert_output ""

	echo 'fn main() { todo!() }' >repo/main.rs
	run --separate-stderr lint
	assert_success
	assert_output --partial 'jouet diag check --changed'
	assert_output --partial '[jouet rule: scoped-lint]'
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

@test "the ledger keeps a compound command's structure and drops its values" {
	jq -cn '{session_id: "s1", tool_name: "Bash", tool_input: {command: "cd /repo && for pr in 101 102; do gh pr checks $pr --repo acme/app; done"}}' |
		jouet hook claude pre-tool-use
	jq -cn '{session_id: "s1", tool_name: "Bash", tool_input: {command: "case \"$e\" in\n  deploy-prod) make ship;;\nesac"}}' |
		jouet hook claude pre-tool-use

	ledger="$XDG_STATE_HOME/jouet/ledger/ledger.jsonl"
	run jq -r '.shape' "$ledger"
	assert_line --index 0 'cd <arg> && for <arg> in <arg>…; do gh pr checks <arg> --repo <arg>; done'
	assert_line --index 1 'case <arg> in <arg>) make <arg>;; esac'
	# Every field but the timestamp, whose digits can hold "101".
	run bash -c "jq -c 'del(.ts)' '$ledger' | grep -cE 'acme|deploy-prod|101'"
	assert_output 0
}

# A tool call in a project that uses fnox: `$PWD/app`, whose fnox.toml
# defines STRIPE_KEY and reads secrets.env through a plain provider.
fnox_payload() {
	mkdir -p app
	printf '[providers.file]\ntype = "plain"\nfile = "secrets.env"\nformat = "env"\n\n[secrets]\nSTRIPE_KEY = { provider = "file", value = "STRIPE_KEY" }\n' >app/fnox.toml
	jq -cn --arg cwd "$PWD/app" --arg tool "$1" --arg arg "$2" \
		'{session_id: "s1", cwd: $cwd, tool_name: $tool, tool_input: (if $tool == "Bash" then {command: $arg} else {file_path: $arg} end)}'
}

@test "in a fnox project, commands that print secrets are denied and fnox exec is suggested" {
	# shellcheck disable=SC2016 # the variables are for the hook to read, not to expand
	for command in env printenv 'echo "$STRIPE_KEY"' 'fnox get STRIPE_KEY' 'cat .env' 'cat ../app/secrets.env'; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(fnox_payload Bash "$command")"
		assert_success
		run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
		assert_line --index 0 deny
		assert_line --index 1 --partial "Run \`fnox exec -- <command>\` instead."
	done

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(fnox_payload Read "$PWD/app/.env.local")"
	run jq -r '.hookSpecificOutput.permissionDecisionReason' <<<"$output"
	assert_output --partial '[jouet rule: secret-files]'

	# shellcheck disable=SC2016
	for command in 'env FOO=bar cargo test' 'echo "$HOME"' 'fnox exec -- npm test' 'cat .env.example'; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(fnox_payload Bash "$command")"
		assert_success
		assert_output ""
	done
}

@test "outside a fnox project, env and .env files are left alone" {
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: "env; cat .env"}}')"

	assert_success
	assert_output ""
}

@test "session-start says when the project uses fnox, and when the agent runs under its proxy" {
	fnox_payload Bash ls >/dev/null
	run --separate-stderr jouet hook claude session-start <<<"$(jq -cn --arg cwd "$PWD/app" '{session_id: "s1", source: "startup", cwd: $cwd}')"
	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial 'jouet: this project keeps its secrets in fnox.'

	FNOX_PROXY_MCP=1 run --separate-stderr jouet hook claude session-start <<<"$(jq -cn --arg cwd "$PWD/app" '{session_id: "s1", source: "startup", cwd: $cwd}')"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial "jouet: this session runs under \`fnox proxy run\`"
}
