#!/usr/bin/env bats
# shellcheck disable=SC2016 # backticks in expected output are Markdown

# Rules from outside: a repository's .jouet/rules, rule packs pinned by
# commit, and the system directory's enforced rules. HOME is a scratch
# directory; JOUET_SYSTEM_DIR points at one, since bats cannot make a
# directory root owns.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_RULES_DISABLE JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME
	export JOUET_SYSTEM_DIR="$BATS_TEST_TMPDIR/etc-jouet"
	mkdir -p repo/.git
	cd repo || return 1
}

# A rule NAME that denies COMMAND with MESSAGE, and passes its examples.
rule() {
	local name=$1 command=$2 message=${3:-no}
	cat <<TOML
[[rule]]
name = "$name"
why = "a team rule"
message = "$message"
match = { command = "$command" }
examples = { fires = ["$command x"], quiet = ["$command-not x"] }
TOML
}

# A Bash tool payload for COMMAND, in this directory.
bash_payload() {
	jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
}

# A git repository at DIR holding rules/team.toml, committed once per
# MESSAGE, so each commit's rule says something else.
pack_repo() {
	local dir=$1
	shift
	mkdir -p "$dir/rules"
	git -C "$dir" init -q
	for message in "$@"; do
		rule pack-rule curl "$message" >"$dir/rules/team.toml"
		git -C "$dir" add rules
		git -C "$dir" -c user.email=t@example.com -c user.name=t commit -qm "$message"
	done
}

@test "a repository's rules run once the user trusts them at a terminal, and again after each change" {
	mkdir -p .jouet/rules
	rule team ls >.jouet/rules/team.toml

	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted +deny +repo:untrusted'

	# No terminal and no --yes: the user is not there to decide.
	run --separate-stderr jouet rule trust
	assert_failure 4
	[[ "$stderr" == *"trusting rules is the user's decision"* ]]

	run in_tty "XDG_STATE_HOME=$XDG_STATE_HOME" jouet rule trust
	assert_success
	assert_output --partial 'ok    team  deny  a team rule'
	assert_output --partial '(1 rules)'
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +enabled +deny +repo:trusted'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls -l')"
	assert_output --partial '[jouet rule: team]'

	# A pull changes a rule: it stops running until trusted again.
	rule team ls 'changed' >.jouet/rules/team.toml
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted +deny +repo:changed'
	assert_line --partial 'changed since it was trusted'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls -l')"
	assert_output ''
	run --separate-stderr jouet rule trust --yes
	assert_success

	# Back on the branch trusted first, nothing needs trusting again.
	rule team ls >.jouet/rules/team.toml
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +enabled +deny +repo:trusted'

	run --separate-stderr jouet rule trust --list
	assert_line --regexp '^repo +trusted +.*/repo/\.jouet/rules$'
	assert_line --regexp '^trusted  .*/repo/\.jouet/rules  \(2 versions\)$'
}

@test "rules that fail their examples are not trusted" {
	mkdir -p .jouet/rules
	rule team ls >.jouet/rules/team.toml
	cat >.jouet/rules/wrong.toml <<'TOML'
[[rule]]
name = "wrong"
why = "says it fires on ls, but matches rm"
message = "no"
match = { command = "rm" }
examples = { fires = ["ls"], quiet = ["pwd"] }
TOML

	run --separate-stderr jouet rule trust --yes
	assert_failure 1
	assert_line 'FAIL  wrong  deny  says it fires on ls, but matches rm'
	assert_output --partial "its examples do not pass: 1 of its 2 examples fail; see \`jouet rule test wrong\`"
	assert_output --partial 'not trusting'
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted'

	run --separate-stderr jouet --json rule trust --yes
	assert_failure 1
	run jq -c '[.trusted, (.rules | map(select(.problem)) | map(.name))]' <<<"$output"
	assert_output '[null,["wrong"]]'
}

@test "an agent is denied trusting rules or adding a pack, with or without --yes" {
	for command in 'jouet rule trust' 'jouet rule trust --yes' 'jouet rule trust --system --yes' \
		'jouet rule pack add team https://github.com/acme/rules --yes'; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload "$command")"
		assert_success
		run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
		assert_line --index 0 deny
		assert_line --index 1 --partial "Trusting rules is the user's decision"
		assert_line --index 1 --partial '[jouet rule: trust-rules]'
	done

	# Looking is fine.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'jouet rule trust --list')"
	assert_output ''
}

@test "the primer says in one line that this repository's rules do not run, and does not ask the agent to trust them" {
	mkdir -p .jouet/rules
	rule team ls >.jouet/rules/team.toml
	rule other rm >.jouet/rules/other.toml

	run --separate-stderr jouet hook claude session-start <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd}')"
	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_line --regexp "^jouet: rules here are not trusted, so they do not run \(2 in .*/repo/\.jouet/rules\); trusting them is the user's decision, in their own terminal \(\`jouet rule trust\`\), not yours\.$"

	jouet rule trust --yes >/dev/null
	run --separate-stderr jouet hook claude session-start <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd}')"
	refute_output --partial 'not trusted'
}

@test "a pack is pinned at a full commit, runs that commit's rules, and is checked against its pin" {
	pack_repo "$BATS_TEST_TMPDIR/pack" 'first' 'second'
	first=$(git -C "$BATS_TEST_TMPDIR/pack" rev-parse HEAD~1)
	short=${first:0:12}

	run --separate-stderr jouet rule pack add acme "$BATS_TEST_TMPDIR/pack" --rev "$short"
	assert_failure 4

	run --separate-stderr jouet rule pack add acme "$BATS_TEST_TMPDIR/pack" --rev "$short" --yes
	assert_success
	assert_line 'ok    pack-rule  deny  a team rule'
	assert_line --regexp "^pinned acme \(.*/pack at $short, 1 rules\) in .*/\.config/jouet/packs\.toml$"
	run grep '^rev = ' "$HOME/.config/jouet/packs.toml"
	assert_output "rev = \"$first\""

	# The rule as it was at the pinned commit, not at the branch's tip.
	run --separate-stderr jouet rule list
	assert_line --regexp '^pack-rule +enabled +deny +pack:acme +'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial 'first'
	refute_output --partial 'second'

	run --separate-stderr jouet rule pack ls
	assert_line --regexp "^acme  verified  user  .*/pack at $short$"

	# The kept rules changed on disk: they stop running until fetched again.
	kept=$(echo "$XDG_STATE_HOME"/jouet/packs/*/team.toml)
	sed -i.bak 's/first/tampered/' "$kept" && rm "$kept.bak"
	run --separate-stderr jouet rule list
	assert_line --regexp '^pack-rule +untrusted +deny +pack:acme:mismatch'
	assert_line --partial 'do not match its pin, so they do not run; run `jouet rule pack sync`'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output ''
	run --separate-stderr jouet rule pack sync
	assert_success
	assert_line 'ok    acme  fetched'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial 'first'

	# A pin moved to another commit, with the old hash, fails to fetch.
	second=$(git -C "$BATS_TEST_TMPDIR/pack" rev-parse HEAD)
	sed -i.bak "s/$first/$second/" "$HOME/.config/jouet/packs.toml"
	rm -rf "$XDG_STATE_HOME"/jouet/packs/*
	run --separate-stderr jouet rule pack sync
	assert_failure 5
	assert_line --partial 'hash to'

	run --separate-stderr jouet rule pack rm acme
	assert_success
	run --separate-stderr jouet rule pack ls
	assert_output 'no rule packs are pinned'
}

@test "a pack whose rules fail their examples is not pinned, and a pin written by hand is still held to them" {
	mkdir -p "$BATS_TEST_TMPDIR/bad/rules"
	cat >"$BATS_TEST_TMPDIR/bad/rules/wrong.toml" <<'TOML'
[[rule]]
name = "wrong"
why = "says it fires on ls, but matches rm"
message = "no"
match = { command = "rm" }
examples = { fires = ["ls"], quiet = ["pwd"] }
TOML

	run --separate-stderr jouet --json rule pack add bad "$BATS_TEST_TMPDIR/bad" --yes
	assert_failure 1
	digest=$(jq -r .digest <<<"$output")
	[[ "$(jq -r .pinned <<<"$output")" == null ]]
	[[ ! -e "$HOME/.config/jouet/packs.toml" ]]

	mkdir -p "$HOME/.config/jouet"
	cat >"$HOME/.config/jouet/packs.toml" <<TOML
[pack.bad]
path = "$BATS_TEST_TMPDIR/bad"
dir = "rules"
digest = "$digest"
TOML
	rm -rf "$XDG_STATE_HOME/jouet/packs"
	run --separate-stderr jouet rule list
	assert_line --partial 'rule pack bad'
	jouet rule pack sync >/dev/null
	run --separate-stderr jouet rule list
	assert_line --regexp '^wrong +broken: its examples do not pass: 1 of its 2 examples fail'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'rm x')"
	assert_output ''
}

@test "a repository's pinned packs run once the repository is trusted" {
	pack_repo "$BATS_TEST_TMPDIR/pack" 'from the repo'
	mkdir -p .jouet

	run --separate-stderr jouet rule pack add acme "$BATS_TEST_TMPDIR/pack" --rev HEAD --repo --yes
	assert_success
	assert_line --partial 'need trusting again'
	[[ -f .jouet/packs.toml ]]

	run --separate-stderr jouet rule pack ls
	assert_line --partial 'pins packs, which are not fetched or run until it is trusted'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output ''

	# Trusting fetches the packs it pins and runs their examples too.
	rm -rf "$XDG_STATE_HOME"/jouet/packs
	run --separate-stderr jouet rule trust --yes
	assert_success
	assert_output --partial 'and the packs acme'
	run --separate-stderr jouet rule list
	assert_line --regexp '^pack-rule +enabled +deny +pack:acme +'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial 'from the repo'
}

@test "an enforced rule runs whatever the user's settings say" {
	mkdir -p "$JOUET_SYSTEM_DIR/rules"
	rule org ls 'the organisation says no' >"$JOUET_SYSTEM_DIR/rules/org.toml"

	# The user owns this directory, so it is not the administrator's: it
	# runs only once trusted, or anything could set JOUET_SYSTEM_DIR.
	run --separate-stderr jouet rule list
	assert_line --regexp '^org +untrusted +deny +enforced:untrusted'
	assert_line --regexp "is not trusted and is not the administrator's \(.* is owned by uid [0-9]+, not root\)|jouet runs as root"
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls')"
	assert_output ''

	run --separate-stderr jouet rule trust --system --yes
	assert_success

	mkdir -p "$HOME/.config/jouet"
	printf '[rules]\ndisable = ["org", "git-stash"]\n' >"$HOME/.config/jouet/config.toml"
	export JOUET_RULES_DISABLE=all
	run --separate-stderr jouet rule list
	assert_line --regexp '^org +enabled +deny +enforced:trusted'
	assert_line --regexp '^git-stash +disabled'
	assert_line --partial 'org is enforced by '
	assert_line --partial 'so the setting that disables it does not apply'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls')"
	run jq -r '.hookSpecificOutput.permissionDecisionReason' <<<"$output"
	assert_output --partial 'the organisation says no'
	assert_output --partial '[jouet rule: org]'
}
