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

@test "an environment-wrapped affected test clears its own failure" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	ran 'env -u NO_COLOR jouet test --affected' 1
	run stop_reason
	assert_output --partial 'The last test run failed'
	ran 'env -u NO_COLOR jouet test --affected' 0
	run stop_reason
	assert_output ""
}

@test "an excluded pnpm Vitest run does not clear a failed package test" {
	git init -q repo
	mkdir -p repo/admin
	echo 'export {}' >repo/index.ts
	git -C repo add index.ts

	ran 'pnpm --dir web test' 1
	ran 'cd admin && pnpm test' 0
	ran 'jouet test --cmd -- pnpm --dir web exec vitest run --exclude tests/b.test.ts' 0
	run stop_reason
	assert_output --partial 'pnpm --dir web test'

	ran 'pnpm --dir web test' 0
	run stop_reason
	assert_output ""
}

@test "an ordinary pnpm Vitest run does not clear failures in other modes" {
	git init -q repo
	echo 'export {}' >repo/index.ts
	git -C repo add index.ts

	for mode in typecheck bench watch dev; do
		ran "pnpm --dir web exec vitest $mode" 1
		ran 'pnpm --dir web exec vitest run' 0
		run stop_reason
		assert_output --partial "vitest $mode"
		ran "pnpm --dir web exec vitest $mode" 0
		run stop_reason
		assert_output ""
	done
}

@test "a passing sh -c script of several checks clears their failures" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs

	ran 'cargo test --workspace' 101
	ran "jouet test --cmd -- bash -c 'cargo build && cargo test --workspace'" 0
	run stop_reason
	assert_output ""
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
	# A tree is two hashes, the whole and the part without prose.
	assert_line --index 0 '["full",65]'
	assert_line --index 1 '["partial",65]'
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

@test "a clean merge after a passing run requires a check of the combined tree" {
	git init -q -b work repo
	echo 1 >repo/main.rs
	echo 1 >repo/lib.rs
	g() { git -C repo -c user.name=t -c user.email=t@t -c commit.gpgsign=false "$@"; }
	g add main.rs lib.rs
	g commit -qm base
	g checkout -qb main
	echo 2 >repo/lib.rs
	g commit -qam theirs
	# Each branch passes separately, but their changes interact.
	run bash -c 'test "$(cat repo/main.rs)$(cat repo/lib.rs)" != 22'
	assert_success
	g checkout -q work
	echo 2 >repo/main.rs
	g commit -qam mine
	run bash -c 'test "$(cat repo/main.rs)$(cat repo/lib.rs)" != 22'
	assert_success
	ran 'cargo test --workspace' 0
	g merge -q --no-edit main
	run bash -c 'test "$(cat repo/main.rs)$(cat repo/lib.rs)" != 22'
	assert_failure
	run stop_reason
	assert_output --partial 'Files changed after the last test run passed ('
	# A later passing check at the combined tree clears the gate.
	ran 'cargo test --workspace' 0
	run stop_reason
	assert_output ""
}

@test "files a formatter rewrote after a passing run need no second run" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	about_to() {
		jq -cn --arg c "$1" --arg cwd "$PWD/repo" \
			'{session_id: "s1", cwd: $cwd, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: {command: $c}}' |
			jouet hook claude pre-tool-use >/dev/null
	}

	ran 'cargo test --workspace wait_github' 0
	# `cargo fmt` reformats main.rs between its two hooks.
	about_to 'cargo fmt --all && git status --short'
	echo 'fn main() { }' >repo/main.rs
	ran 'cargo fmt --all && git status --short' 0
	run stop_reason
	assert_output ""
	# An edit afterwards is still a change.
	echo 'fn main() { todo!() }' >repo/main.rs
	run stop_reason
	assert_output --partial 'Files changed after the last test run passed ('
	# With steering off, no rule runs before a call, but the tree is kept.
	ran 'cargo test --workspace wait_github' 0
	JOUET_HOOKS_STEER=false about_to 'cargo fmt --all'
	echo 'fn main() { todo!(); }' >repo/main.rs
	ran 'cargo fmt --all' 0
	run stop_reason
	assert_output ""
}

@test "an edit during a read-only status call still requires a check" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	ran 'cargo test --workspace' 0
	jq -cn --arg cwd "$PWD/repo" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: {command: "git status --short"}}' |
		jouet hook claude pre-tool-use >/dev/null
	ran 'git status --short' 0
	run stop_reason
	assert_output ""
	jq -cn --arg cwd "$PWD/repo" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: {command: "git status --short"}}' |
		jouet hook claude pre-tool-use >/dev/null
	# Another process can edit while status is running; that is not formatting.
	echo 'fn main() { todo!() }' >repo/main.rs
	ran 'git status --short' 0
	run stop_reason
	assert_output --partial 'Files changed after the last test run passed ('
}

@test "Codex skips formatting snapshots for standalone status calls" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	codex_before() {
		jq -cn --arg cwd "$PWD/repo" --arg c "$1" \
			'{session_id: "s1", cwd: $cwd, hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: {command: $c}}' |
			jouet hook codex pre-tool-use >/dev/null
	}
	codex_before 'git status --short'
	[ ! -e "$XDG_STATE_HOME/jouet/sessions/s1.json" ]
	codex_before 'cargo fmt --all'
	[ -f "$XDG_STATE_HOME/jouet/sessions/s1.json" ]
}

@test "files a jouet commit's hooks rewrote need no second run, whatever else its call ran" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	git -C repo config user.name test
	git -C repo config user.email test@example.com
	git -C repo commit -q -m base
	# A pre-commit hook that reformats what it commits, as hk fix does.
	printf '#!/bin/sh\necho "fn main() { }" >main.rs\ngit add main.rs\n' >repo/.git/hooks/pre-commit
	chmod +x repo/.git/hooks/pre-commit
	echo 'fn main() { todo!() }' >repo/main.rs

	ran 'cargo test --workspace' 0
	# The agent's call ran the tests and committed after a cd, which the
	# hooks cannot tell from an edit; jouet commit tells them itself.
	(cd repo && CLAUDE_CODE_SESSION_ID=s1 jouet commit main.rs --message change >/dev/null)
	assert_equal "$(cat repo/main.rs)" 'fn main() { }'
	run stop_reason
	assert_output ""
}

@test "a failure is raised once for its run, not at every turn's stop or after other changes" {
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

	# Files changed with no run since: the failure was said once.
	echo 'fn main() { todo!() }' >repo/main.rs
	run stop_reason
	assert_output ""

	# A new run failing at the files now is new.
	ran 'cargo test --workspace brew::cask' 101
	run stop_reason
	assert_output --partial 'exited 101'
	# The same failure of the same files again is not.
	ran 'cargo test --workspace brew::cask' 101
	run stop_reason
	assert_output ""
}

@test "a failure whose output blames the machine is a note, not a block" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	jq -cn --arg cwd "$PWD/repo" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: "PostToolUseFailure", tool_name: "Bash",
		  tool_input: {command: "bats test/edit.bats"},
		  error: "Exit code 1\n1..1\nnot ok 1 keychain secrets\n# Failed to initialize OS keyring store: Platform failure: DBus error: no session bus\n"}' |
		jouet hook claude post-tool-use-failure
	run --separate-stderr sh -c "jq -cn --arg cwd '$PWD/repo' '{session_id: \"s1\", cwd: \$cwd, stop_hook_active: false}' | jouet hook claude stop"
	run jq -r '[.decision // "none", .systemMessage // ""] | @tsv' <<<"$output"
	assert_output --partial 'none'
	assert_output --partial 'blame the machine'
	# Said once.
	run --separate-stderr sh -c "jq -cn --arg cwd '$PWD/repo' '{session_id: \"s1\", cwd: \$cwd, stop_hook_active: false}' | jouet hook claude stop"
	assert_output ''
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

@test "a work tree the session left committed and idle for an hour is not asked about" {
	git init -q repo
	git init -q other
	commit() { git -C other -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit -qm "$1"; }
	echo 'fn main() {}' >other/main.rs
	git -C other add main.rs
	commit init
	ran "cd $PWD/other && cargo test --workspace" 0

	# Its branch moves on (merged, pulled): clean, but not the tree that
	# passed. Two hours later, the session stops in its own tree.
	echo 'fn main() { println!() }' >other/main.rs
	git -C other add main.rs
	commit merged
	local state="$XDG_STATE_HOME/jouet/sessions/s1.json" root
	root=$(cd other && pwd -P)
	jq --arg r "$root" --argjson t "$(($(date +%s) - 7200))" '.trees[$r].last_run = $t' "$state" >"$state.new"
	mv "$state.new" "$state"
	run stop_reason
	assert_output ""

	# With something left uncommitted there, it is asked about.
	echo 'fn main() { todo!() }' >other/main.rs
	run stop_reason
	assert_output --partial "In $root: Files changed after the last test run passed"
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

@test "stop for codex lets a failure whose output blames the machine through, with a note" {
	git init -q repo
	item() {
		jq -cn --arg command "$1" --arg output "$2" \
			'{type: "event_msg", payload: {type: "item_completed", item: {type: "CommandExecution",
			  command: ["/bin/zsh", "-lc", $command], exit_code: 1, aggregated_output: $output}}}'
	}
	codex_stop() {
		jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"cwd\":\"$PWD/repo\",\"stop_hook_active\":false}"
	}
	item "bats test/edit.bats" $'1..1\nnot ok 1 keychain secrets\n# Failed to initialize OS keyring store: Platform failure: DBus error: no session bus\n' >rollout.jsonl
	run --separate-stderr codex_stop
	assert_success
	refute_output --partial '"decision":"block"'
	assert_output --partial 'blame the machine'
	# The same command failing on the change's own assertion still blocks.
	item "bats test/edit.bats" $'1..1\nnot ok 1 keychain secrets\n# expected: 1\n# actual: 2\n' >rollout.jsonl
	run --separate-stderr codex_stop
	assert_output --partial '"decision":"block"'
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

@test "a matching baseline comparison clears a wrapped lint failure in a codex transcript" {
	git init -q repo
	item() {
		jq -cn --arg command "$1" --argjson exit_code "$2" \
			'{type: "event_msg", payload: {type: "item_completed", item: {type: "CommandExecution",
			  command: ["/bin/zsh", "-lc", $command], exit_code: $exit_code}}}'
	}
	item 'jouet test --cmd -- env MISE_ENV=bench mise x ruff@0.17.4 -- ruff check --isolated a.py b.py' 1 >rollout.jsonl
	run --separate-stderr jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"cwd\":\"$PWD/repo\",\"stop_hook_active\":false}"
	assert_success
	assert_output --partial '"decision":"block"'
	item 'jouet test --baseline --cmd -- env MISE_ENV=bench mise x ruff@0.17.4 -- ruff check --isolated a.py c.py' 0 >>rollout.jsonl
	run --separate-stderr jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"cwd\":\"$PWD/repo\",\"stop_hook_active\":false}"
	assert_success
	assert_output --partial '"decision":"block"'
	item 'jouet test --baseline --cmd -- env MISE_ENV=bench mise x ruff@0.17.4 -- ruff check --isolated a.py b.py' 0 >>rollout.jsonl
	run --separate-stderr jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"cwd\":\"$PWD/repo\",\"stop_hook_active\":false}"
	assert_success
	refute_output --partial '"decision":"block"'
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
	# The deny and its override name the revision of the rule that decided.
	run jq -r '.decisions[0].revision // "-"' "$ledger"
	assert_line --index 0 --regexp '^[0-9a-f]{12}$'
	assert_line --index 1 "${lines[0]}"
	assert_line --index 3 '-'
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
	# A test run gets only its 10-minute timeout.
	for command in 'env FOO=bar cargo test' 'echo "$HOME"' 'fnox exec -- npm test' 'cat .env.example'; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(fnox_payload Bash "$command")"
		assert_success
		run jq -c '.hookSpecificOutput | del(.updatedInput, .hookEventName) // empty' <<<"$output"
		assert_output --regexp '^(\{\})?$'
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

@test "python-edit lets a replace through when its text is in the file, and names text that is not" {
	git init -q repo
	mkdir -p repo/src
	printf 'fn main() {\n    old();\n}\n' >repo/src/a.rs
	edit() {
		jq -cn --arg cwd "$PWD/repo" --arg c "$1" '{session_id: "s1", cwd: $cwd, tool_name: "Bash",
			tool_input: {command: $c}}' |
			jouet hook claude pre-tool-use
	}
	script() {
		printf "python3 - <<'E'\np='src/a.rs'\ns=open(p).read()\ns=s.replace(\"\"\"%s\"\"\", \"new();\")\nopen(p,'w').write(s)\nE" "$1"
	}

	# The old text is in the file, across lines too: the edit lands.
	run --separate-stderr edit "$(script '    old();\n}')"
	assert_success
	assert_output ""

	# It is not: the script would change nothing, and the denial says why.
	run --separate-stderr edit "$(script 'missing();')"
	assert_success
	assert_output --partial 'text '
	assert_output --partial 'missing();'
	assert_output --partial 'is not in src/a.rs, so this script would change nothing there'
	assert_output --partial 'jouet edit replace src/a.rs --old TEXT'
	assert_output --partial '[jouet rule: python-edit]'

	# After a cd, the path is from there.
	run --separate-stderr edit "cd src && python3 - <<'E'
s=open('a.rs').read()
open('a.rs','w').write(s.replace('old();', 'new();'))
E"
	assert_success
	assert_output ""

	# One script editing two files in turn through the same variable, with
	# statements joined by `;`, an old text held in a variable, and a
	# replacement of text an earlier one inserted: every old text is found.
	printf '#[test]\nfn t() {}\n' >repo/src/a_test.rs
	run --separate-stderr edit "python3 - <<'E'
p='src/a.rs'; s=open(p).read()
s=s.replace('old();', 'new();\n    added();')
s=s.replace('added();', 'more();')
open(p,'w').write(s)
p='src/a_test.rs'
s=open(p).read()
marker='fn t() {}'
s=s.replace(marker, 'fn u() {}\n'+marker, 1)
open(p,'w').write(s)
E"
	assert_success
	assert_output ""

	# A replacement of the exact text an earlier one inserted is found; an
	# old text that is only part of the path is not.
	run --separate-stderr edit "python3 - <<'E'
p='src/a.rs'; s=open(p).read()
s=s.replace('old();', 'mid();')
s=s.replace('mid();', 'new();')
open(p,'w').write(s)
E"
	assert_success
	assert_output ""
	run --separate-stderr edit "python3 - <<'E'
p='src/a.rs'; s=open(p).read()
s=s.replace('a.rs', 'b.rs')
open(p,'w').write(s)
E"
	assert_output --partial 'is not in src/a.rs, so'

	# Text in neither file is still named, with both files.
	run --separate-stderr edit "python3 - <<'E'
p='src/a.rs'; s=open(p).read()
s=s.replace('old();', 'new();')
open(p,'w').write(s)
p='src/a_test.rs'; s=open(p).read()
s=s.replace('fn gone() {}', 'fn u() {}')
open(p,'w').write(s)
E"
	assert_output --partial 'fn gone() {}'
	assert_output --partial 'is not in src/a.rs or src/a_test.rs'
	assert_output --partial 'jouet edit replace PATH --old TEXT --new TEXT'
}

@test "a check run gets a 10-minute timeout, so it ends in that call" {
	call() {
		jq -cn --arg c "$1" --arg mode "$2" --argjson extra "${3:-{\}}" \
			'{session_id: "s1", permission_mode: $mode, tool_name: "Bash", tool_input: ({command: $c} + $extra)}' |
			jouet hook claude pre-tool-use
	}

	# In every mode, with no permission decision, so the user's own check
	# still applies to the call.
	for mode in default acceptEdits plan bypassPermissions; do
		run --separate-stderr call 'cargo test --workspace' "$mode"
		assert_success
		run jq -c '.hookSpecificOutput | [.permissionDecision, .permissionDecisionReason, .updatedInput]' <<<"$output"
		assert_output '[null,null,{"command":"cargo test --workspace","timeout":600000}]'
	done
	# A timeout of its own, or a background run, stands.
	run --separate-stderr call 'cargo test --workspace' bypassPermissions '{"timeout": 30000}'
	assert_output ""
	run --separate-stderr call 'cargo test --workspace' bypassPermissions '{"run_in_background": true}'
	assert_output ""
	# Not a test, build or lint.
	run --separate-stderr call 'ls -la' bypassPermissions
	assert_output ""
	# A denial stays a denial.
	run --separate-stderr call 'git add -A' bypassPermissions
	run jq -r '.hookSpecificOutput.permissionDecision' <<<"$output"
	assert_output "deny"
}

# A recorded Claude Code payload from test/fixtures/hooks/claude, with the
# jq filter FILTER applied: `claude_payload subagent-pre-tool-use '.cwd = "x"'`.
claude_payload() {
	jq -c "${2:-.}" "$PROJECT_ROOT/test/fixtures/hooks/claude/$1.json"
}

@test "a subagent's reads are its own, not its siblings' or the lead's" {
	printf 'fn a() {}\nfn b() {}\nfn c() {}\n' >lib.rs
	read_by() {
		claude_payload subagent-post-tool-use \
			"$1 | .cwd = \"$PWD\" | .tool_input.file_path = \"$PWD/lib.rs\" | .hook_event_name = \"$2\""
	}
	jouet hook claude post-tool-use <<<"$(read_by . PostToolUse)"

	# The subagent that read it is told; a sibling and the lead are not.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(read_by . PreToolUse)"
	assert_output --partial '[jouet rule: reread]'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(read_by '.agent_id = "b8e2d0c4a6f1"' PreToolUse)"
	assert_output ""
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(read_by 'del(.agent_id, .agent_type)' PreToolUse)"
	assert_output ""
}

@test "jouet read in a subagent's Bash call remembers for that subagent only" {
	printf 'fn fetch() {}\n' >lib.rs
	export CLAUDE_CODE_SESSION_ID=s1
	# Bash as Claude Code runs it: the pre-tool-use hook, the command with
	# only the session in its environment, then the post-tool-use hook.
	bash_by() {
		local call
		call="$(claude_payload subagent-pre-tool-use \
			"$1 | .cwd = \"$PWD\" | .tool_input.command = \"jouet read lib.rs --symbol fetch\"")"
		jouet hook claude pre-tool-use <<<"$call" >/dev/null
		jouet read lib.rs --symbol fetch
		jouet hook claude post-tool-use <<<"$(jq -c '.hook_event_name = "PostToolUse"' <<<"$call")"
	}

	run --separate-stderr bash_by .
	assert_line --index 0 'lib.rs: function fetch (1-1)'
	run --separate-stderr bash_by '.agent_id = "b8e2d0c4a6f1"'
	assert_line --index 0 'lib.rs: function fetch (1-1)'
	run --separate-stderr bash_by 'del(.agent_id, .agent_type)'
	assert_line --index 0 'lib.rs: function fetch (1-1)'
	run --separate-stderr bash_by .
	assert_output --partial 'already shown, unchanged'
	run --separate-stderr bash_by 'del(.agent_id, .agent_type)'
	assert_output --partial 'already shown, unchanged'

	# Two agents' reads running at once: it cannot tell whose, so it
	# collapses nothing.
	claude_payload subagent-pre-tool-use '.tool_use_id = "toolu_other" | .agent_id = "c0"' |
		jouet hook claude pre-tool-use >/dev/null
	run --separate-stderr bash_by .
	assert_line --index 0 'lib.rs: function fetch (1-1)'
}

@test "the lead's stop is not held up by a subagent's scratch repository" {
	git init -q repo
	echo 'fn main() {}' >repo/main.rs
	git -C repo add main.rs
	git init -q scratch
	echo 'x = 1' >scratch/a.py
	git -C scratch add a.py
	claude_payload subagent-post-tool-use \
		".cwd = \"$PWD/scratch\" | .tool_name = \"Bash\" | .tool_input = {command: \"cargo test --workspace\"} | del(.tool_response)" |
		jouet hook claude post-tool-use
	echo 'x = 2' >scratch/a.py
	stop() {
		claude_payload stop ".cwd = \"$PWD/repo\"" | jouet hook claude stop | jq -r '.reason // empty'
	}

	run stop
	assert_output ""

	# Once the lead runs something there, the tree is the lead's too.
	jq -cn --arg cwd "$PWD/scratch" \
		'{session_id: "s1", cwd: $cwd, hook_event_name: "PostToolUse", tool_name: "Bash", tool_input: {command: "cargo test --workspace"}}' |
		jouet hook claude post-tool-use
	echo 'x = 3' >scratch/a.py
	run stop
	# macOS puts the test directory under /private/tmp.
	assert_output --regexp "^In (/private)?$PWD/scratch: Files changed after the last test run passed"
}

@test "session-start names the project's checks from a trusted .jouet/project.toml" {
	git init -q repo
	mkdir -p repo/.jouet
	printf '[checks]\nfull = "mise run test"\n[checks.focused]\nbats = "jouet test --cmd -- mise run test:bats -- test/NAME.bats"\n' >repo/.jouet/project.toml
	start() {
		jq -cn --arg cwd "$PWD/repo" '{session_id: "s1", source: "startup", cwd: $cwd}' |
			jouet hook claude session-start | jq -r '.hookSpecificOutput.additionalContext'
	}
	# Not before a person trusts it: it is text every session reads.
	run start
	refute_output --partial "prefer these project-specific checks over generic examples when applicable"
	(cd repo && jouet trust --yes >/dev/null)
	run start
	assert_output --partial "jouet: prefer these project-specific checks over generic examples when applicable (.jouet/project.toml): bats \`jouet test --cmd -- mise run test:bats -- test/NAME.bats\`; full suite \`mise run test\`."
}

@test "session-start uses a project's affected command only while trusted" {
	git init -q repo
	mkdir -p repo/.jouet
	printf '[package]\nname = "fixture"\nversion = "0.1.0"\n' >repo/Cargo.toml
	printf '[checks]\naffected = "env -u NO_COLOR jouet test --affected"\n' >repo/.jouet/project.toml
	affected_start() {
		jq -cn --arg cwd "$PWD/repo" '{session_id: "s1", source: "startup", cwd: $cwd}' |
			jouet hook codex session-start | jq -r '.hookSpecificOutput.additionalContext'
	}
	run affected_start
	assert_output --partial "Test with \`jouet test --affected\`"
	refute_output --partial 'env -u NO_COLOR jouet test --affected'
	(cd repo && jouet trust --yes >/dev/null)
	run affected_start
	assert_output --partial "Test with \`env -u NO_COLOR jouet test --affected\`"
	refute_output --partial "Test with \`jouet test --affected\`"
	printf '[checks]\naffected = "env -u NO_COLOR jouet test --affected"\n[checks.broken]\n"env -u NO_COLOR jouet test --affected" = "missing dependency"\n' >repo/.jouet/project.toml
	(cd repo && jouet trust --yes >/dev/null)
	run affected_start
	assert_output --partial "Test with \`jouet test --cmd"
	refute_output --partial "Test with \`env -u NO_COLOR jouet test --affected\`"
	printf '[checks.broken]\n"jouet test --affected" = "missing dependency"\n' >repo/.jouet/project.toml
	(cd repo && jouet trust --yes >/dev/null)
	run affected_start
	assert_output --partial "Test with \`jouet test --cmd"
	refute_output --partial "Test with \`jouet test --affected\`"
	printf '[checks]\naffected = "env -u NO_COLOR jouet test --affected"\n' >repo/.jouet/project.toml
	printf '\n# changed\n' >>repo/.jouet/project.toml
	run affected_start
	assert_output --partial "Test with \`jouet test --affected\`"
	refute_output --partial 'env -u NO_COLOR jouet test --affected'
	printf '[checks]\naffected = ""\n' >repo/.jouet/project.toml
	run bash -c 'cd repo && jouet trust --yes'
	assert_failure
	assert_output --partial 'checks.affected must be a nonempty single-line command'
}

@test "session-start points at a trusted .jouet/map.md without inlining it" {
	git init -q repo
	mkdir -p repo/.jouet
	printf '# Map\n\n- src/backend/: one file per backend\n' >repo/.jouet/map.md
	map_start() {
		jq -cn --arg cwd "$PWD/repo" '{session_id: "s1", source: "startup", cwd: $cwd}' |
			jouet hook claude session-start | jq -r '.hookSpecificOutput.additionalContext'
	}
	run map_start
	refute_output --partial '.jouet/map.md'
	# Trusting it shows it for review.
	run bash -c 'cd repo && jouet trust --yes'
	assert_output --partial 'map.md (every session is told to read it):'
	assert_output --partial 'one file per backend'
	run map_start
	assert_output --partial 'jouet: .jouet/map.md maps where this project'"'"'s code, tests and generated files live (3 lines): read it before searching'
	refute_output --partial 'one file per backend'
	# A changed map needs trusting again.
	echo '- lib/: more' >>repo/.jouet/map.md
	run map_start
	refute_output --partial '.jouet/map.md maps'
}

@test "a check .jouet/project.toml lists as broken is a note, not a block, once trusted" {
	git init -q repo
	mkdir -p repo/.jouet
	printf '[checks.broken]\n"mise run lint" = "golangci-lint is built with an older Go"\n' >repo/.jouet/project.toml
	lint_failed() {
		jq -cn --arg cwd "$PWD/repo" --arg command "$1" --arg session "$2" \
			'{session_id: $session, cwd: $cwd, hook_event_name: "PostToolUseFailure", tool_name: "Bash",
			  tool_input: {command: $command}, error: "Exit code 1\n[lint] ERROR task failed\n"}' |
			jouet hook claude post-tool-use-failure
	}
	stopping() {
		jq -cn --arg cwd "$PWD/repo" --arg session "$1" '{session_id: $session, cwd: $cwd, stop_hook_active: false}' |
			jouet hook claude stop
	}
	# Untrusted, it is not taken at its word.
	lint_failed "mise run lint" s1
	run --separate-stderr stopping s1
	run jq -r '.decision // "none"' <<<"$output"
	assert_output block
	(cd repo && jouet trust --yes >/dev/null)
	# A new session, so nothing was said before.
	lint_failed "jouet test --cmd -- mise run lint" s2
	run --separate-stderr stopping s2
	run jq -r '[.decision // "none", .systemMessage // ""] | @tsv' <<<"$output"
	assert_output --partial none
	assert_output --partial "fails here before any change: golangci-lint is built with an older Go"
	# Codex, whose runs are read again from its transcript at each stop.
	printf '%s\n' '{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","command":["/bin/zsh","-lc","jouet test --cmd -- mise run lint"],"exit_code":1}}}' >rollout.jsonl
	run --separate-stderr jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"cwd\":\"$PWD/repo\",\"stop_hook_active\":false}"
	assert_success
	refute_output --partial '"decision":"block"'
	# A check the file does not list still blocks.
	printf '%s\n' '{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","command":["/bin/zsh","-lc","mise run test"],"exit_code":1}}}' >rollout.jsonl
	run --separate-stderr jouet hook codex stop <<<"{\"transcript_path\":\"$PWD/rollout.jsonl\",\"cwd\":\"$PWD/repo\",\"stop_hook_active\":false}"
	assert_output --partial '"decision":"block"'
}

@test "trust refuses a .jouet/project.toml that does not parse" {
	git init -q repo
	mkdir -p repo/.jouet
	printf '[checks]\nfast = "cargo test"\n' >repo/.jouet/project.toml
	cd repo
	run jouet trust --yes
	assert_failure
	assert_output --partial "project.toml does not parse: unknown field"
	assert_output --partial "not trusting"
}
