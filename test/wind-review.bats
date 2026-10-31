#!/usr/bin/env bats

# Session review: `jouet wind digest`, `jouet wind review` and `jouet wind
# propose`, and the stop and session-start hooks with `[wind] review` on.
# The transcripts are the jouet-audit digest fixtures: a Claude Code session
# in /work/app with an interruption, a correction, a refused force-push, a
# repeated instruction holding a token, failing retries and long sleeps, and
# a Codex rollout with an aborted turn and a command that never worked.

CLAUDE_ID=d1d1d1d1-0000-4000-8000-000000000001
CODEX_ID=01a00000-0000-7000-8000-00000000d1d1

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_WIND JOUET_WIND_REVIEW JOUET_WIND_REVIEW_EVERY
	unset JOUET_WIND_GITLEAKS CLAUDE_CONFIG_DIR CODEX_HOME JOUET_HOOKS JOUET_HOOKS_PRIME
	local fixtures="$PROJECT_ROOT/crates/jouet-audit/tests/fixtures/digest"
	cp -r "$fixtures/claude" "$HOME/.claude"
	cp -r "$fixtures/codex" "$HOME/.codex"
	CLAUDE_FILE="$HOME/.claude/projects/-work-app/$CLAUDE_ID.jsonl"
	CODEX_FILE=$(echo "$HOME"/.codex/sessions/2026/10/30/rollout-*.jsonl)
	# Two hours old; BSD touch takes no relative dates.
	perl -e 'my $t = time - 7200; utime $t, $t, @ARGV' "$CODEX_FILE"
	touch "$CLAUDE_FILE"
}

# The session-start hook's primer for SESSION, in the current directory.
primer() {
	jq -cn --arg s "$1" --arg cwd "$PWD" '{hook_event_name: "SessionStart", session_id: $s, cwd: $cwd}' |
		jouet hook claude session-start |
		jq -r '.hookSpecificOutput.additionalContext'
}

@test "digest reduces a session to its corrections, failures, waits and timeline" {
	run --separate-stderr jouet wind digest d1d1

	assert_success
	assert_line "# Session $CLAUDE_ID (claude) in /work/app"
	assert_line "## User corrections (4)"
	assert_line --partial '(after an interrupt): "no, use mise run test instead of cargo test'
	assert_line --partial 'refusal: "never force-push; open a PR instead"'
	assert_line --regexp '^- L4-8 .cargo test 2>&1 \| tail -<n>. failed 2 of 2, never worked: error\[E0425\]'
	assert_line '- src/lib.rs 4 reads, 2 with no edit since the last — L2-18'
	assert_line '## Timeline (4 phases)'
	assert_line --partial "Transcript: ~/.claude/projects/-work-app/$CLAUDE_ID.jsonl"
	# Secrets are redacted, and the home directory is ~.
	assert_line --partial 'GH_TOKEN=<redacted>'
	refute_output --partial 'ghp_'
	refute_output --partial "$HOME"

	run --separate-stderr jouet wind digest "$CODEX_FILE" --json
	assert_success
	run jq -c '.digests[0] | [.harness, (.corrections | map(.kind)), .failures[0].recovered]' <<<"$output"
	assert_output '["codex",["interrupt","stop"],false]'
}

@test "digest picks the last session, or every session since a time" {
	run --separate-stderr jouet wind digest --last --json
	assert_success
	run jq -r '.digests | map(.session) | join(" ")' <<<"$output"
	assert_output "$CLAUDE_ID"

	run --separate-stderr jouet wind digest --since 1d --harness codex --json
	run jq -r '.digests | map(.harness) | join(" ")' <<<"$output"
	assert_output 'codex'

	run --separate-stderr jouet wind digest --since 1d
	assert_success
	assert_line --partial "# Session $CLAUDE_ID"
	assert_line --partial "# Session $CODEX_ID"

	run --separate-stderr jouet wind digest --last --project /elsewhere
	assert_failure 3
}

@test "digest refuses an unknown session, an ambiguous one, and no choice at all" {
	run --separate-stderr jouet wind digest nope
	assert_failure 3
	assert_equal "$stderr" "jouet: no session \`nope\`"

	cp "$CLAUDE_FILE" "$HOME/.claude/projects/-work-app/d1d1d1d1-9999.jsonl"
	run --separate-stderr jouet wind digest d1d1d1d1
	assert_failure 3
	assert [ -n "$(grep -F 'matches 2 sessions' <<<"$stderr")" ]

	run --separate-stderr jouet wind digest
	assert_failure 2
	run --separate-stderr jouet wind digest --last --since 1d
	assert_failure 2
}

@test "a gitleaks config adds its rules to the redaction" {
	mkdir -p "$HOME/.config/jouet"
	cat >"$HOME/.config/jouet/gitleaks.toml" <<'TOML'
[[rules]]
id = "acme-repo"
regex = '''acme/[a-z]+'''
keywords = ["acme"]
TOML
	run --separate-stderr jouet wind digest d1d1
	assert_success
	assert_line 'Redacted with jouet'"'"'s secret patterns and 1 gitleaks rules; local only.'
	refute_output --partial 'acme/app'

	run --separate-stderr env JOUET_WIND_GITLEAKS="$HOME/missing.toml" jouet wind digest d1d1
	assert_success
	assert [ -n "$(grep -F 'not using gitleaks' <<<"$stderr")" ]
}

@test "review prints the prompt, then the digest" {
	run --separate-stderr jouet wind review d1d1

	assert_success
	assert_line --index 0 --partial 'You are reviewing a coding-agent session'
	assert_line --partial "{\"session\": \"$CLAUDE_ID\", \"proposals\": ["
	assert_line '---'
	assert_line "# Session $CLAUDE_ID (claude) in /work/app"

	run --separate-stderr jouet wind review d1d1 --json
	run jq -r '(.prompt | startswith("You are reviewing")), (.digests | length)' <<<"$output"
	assert_output $'true\n1'
}

@test "propose records a review's proposals, which the next session starts show" {
	run --separate-stderr jouet wind propose <<JSON
\`\`\`json
{"session": "$CLAUDE_ID", "proposals": [
  {"kind": "agents_md", "title": "Say that tests run through mise", "why": "The user had to correct cargo test (L8-11).", "evidence": ["L8-11"], "scope": "project", "draft": "Run tests with \`mise run test\`, never cargo test directly."},
  {"kind": "rule", "title": "Leak", "why": "x", "draft": "GH_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789"}
]}
\`\`\`
JSON
	assert_success
	assert_line 'recorded p1; the next session starts show it'
	assert_line 'skipped "Leak": it holds something that looks like a secret'

	run --separate-stderr jouet wind show p1
	assert_success
	assert_line --index 0 'p1 (pending, agents_md, scope project): Say that tests run through mise'
	assert_line --regexp 'Run tests with .mise run test., never cargo test directly'

	run primer next-session
	assert_line --partial 'jouet wind review proposed an AGENTS.md note (p1): Say that tests run through mise.'

	run --separate-stderr jouet wind adopt p1
	assert_success
	run primer another-session
	refute_output --partial 'p1'

	run --separate-stderr jouet wind propose <<<'not json'
	assert_failure 2
	run --separate-stderr jouet wind propose <<<'{"proposals": [{"kind": "nothing"}]}'
	assert_line 'nothing to record: the review found nothing worth changing'
}

@test "with review on, the stop hook queues a corrected session for the next start" {
	git init -q .
	stop() {
		jq -cn --arg s "$1" --arg t "$CLAUDE_FILE" --arg cwd "$PWD" \
			'{hook_event_name: "Stop", session_id: $s, transcript_path: $t, cwd: $cwd}' |
			jouet hook claude stop
	}

	# Off by default: the stop hook reads no transcript.
	stop "$CLAUDE_ID"
	run primer next-session
	refute_output --partial 'jouet wind review'

	export JOUET_WIND_REVIEW=1
	stop "$CLAUDE_ID"
	run primer "$CLAUDE_ID"
	refute_output --partial 'review it in a subagent'
	run primer next-session
	assert_line --partial "jouet wind: session d1d1d1d1 ("
	assert_line --partial "this repository) had 3 user corrections, 2 failed retries, 2m21s polling."
	assert_line --partial "\`jouet wind review $CLAUDE_ID\`"

	run --separate-stderr jouet wind show r1
	assert_line --index 0 --partial 'is worth a review: 3 corrections'
	run --separate-stderr jouet wind dismiss r1
	run primer third-session
	refute_output --partial 'review it in a subagent'
}

@test "a digest records what it finds about jouet itself, for jouet share" {
	cp -r "$PROJECT_ROOT/crates/jouet-audit/tests/fixtures/jouet/claude/projects/-work-app/." "$HOME/.claude/projects/-work-app/"
	run --separate-stderr jouet wind digest e2e2
	assert_success
	assert_line '## jouet itself (4)'
	assert_line --regexp '^- L2 .jouet pr threads. crash: panicked at src/cmd/threads.rs:41:9$'
	refute_output --partial 'index out of bounds'

	# Digesting the same session again counts nothing twice.
	run --separate-stderr jouet wind digest e2e2
	run --separate-stderr jouet share --list --json
	assert_success
	run jq -c '.ideas | map([.key, .destination])' <<<"$output"
	assert_output '[["bad-json-pr-status","issue"],["crash-pr-threads","issue"]]'

	# Off by default: no session-start line.
	run primer next-session
	refute_output --partial 'jouet noticed something for its maintainers'

	export JOUET_SHARE_CONTRIBUTE=1
	run primer next-session
	assert_line --regexp 'jouet noticed something for its maintainers: .jouet pr threads. crashed 1 times'
	assert_line --regexp '.jouet share --idea crash-pr-threads. drafts an Issue on jdx/jouet'

	run --separate-stderr jouet share --idea crash-pr-threads
	assert_success
	assert_line --regexp '^title: Bug: .jouet pr threads. panicked$'
	refute_output --partial 'secret-app'
	assert_line --partial 'Nothing was posted.'
}

@test "with contribute on, a review may propose a change to jouet, recorded as an idea" {
	run --separate-stderr jouet wind review d1d1
	refute_output --partial '"jouet": a change to jouet itself'
	run --separate-stderr env JOUET_SHARE_CONTRIBUTE=1 jouet wind review d1d1
	assert_line --partial '- "jouet": a change to jouet itself, for its maintainers'

	run --separate-stderr jouet wind propose <<<'{"proposals": [{"kind": "jouet", "title": "Say which flag was unknown", "why": "The agent guessed three spellings (L5-7).", "evidence": ["L5-7"], "draft": "Name the closest flag in the usage error."}]}'
	assert_success
	assert_line --regexp "^recorded idea note-say-which-flag-was-unknown for jouet's maintainers; .jouet share --idea note-say-which-flag-was-unknown. drafts it, and posts only with --yes$"
	run --separate-stderr jouet share --idea note-say-which-flag-was-unknown
	assert_success
	assert_line 'title: Idea: Say which flag was unknown'
	assert_line 'A review of one of my agent sessions suggests this change to jouet:'
	assert_line 'Name the closest flag in the usage error.'
}
