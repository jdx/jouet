#!/usr/bin/env bats
# shellcheck disable=SC2016 # backticks in expected output are Markdown

# Rules and routines from outside: a repository's .jouet, plugins pinned by
# commit, and the system directory's enforced ones. HOME is a scratch
# directory, and JOUET_SYSTEM_DIR points at one.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_RULES_DISABLE JOUET_HOOKS JOUET_HOOKS_STEER JOUET_HOOKS_PRIME
	export JOUET_SYSTEM_DIR="$BATS_TEST_TMPDIR/etc-jouet"
	# `joue` as a link to jouet, the way argv[0] dispatch sees it.
	ln -s "$JOUET_BIN" "$BATS_TEST_TMPDIR/bin/joue"
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

# A routine that prints MESSAGE.
routine() {
	printf -- '--- about: says %s\nreturn { run = function() return "%s" end }\n' "$1" "$1"
}

# A Bash tool payload for COMMAND, in this directory.
bash_payload() {
	jq -cn --arg c "$1" --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd, tool_name: "Bash", tool_input: {command: $c}}'
}

# The plugin acme at DIR: a rule denying curl and a routine, both saying
# MESSAGE.
plugin_files() {
	local dir=$1 message=$2
	mkdir -p "$dir/rules" "$dir/routines"
	printf 'name = "acme"\ndescription = "the team'"'"'s"\n' >"$dir/jouet-plugin.toml"
	rule no-curl curl "$message" >"$dir/rules/no-curl.toml"
	routine "$message" >"$dir/routines/hello.luau"
}

# A git repository at DIR holding the plugin acme, committed once per
# MESSAGE, so each commit's rule and routine say something else.
plugin_repo() {
	local dir=$1
	shift
	git init -q "$dir"
	for message in "$@"; do
		plugin_files "$dir" "$message"
		git -C "$dir" add .
		git -C "$dir" -c user.email=t@example.com -c user.name=t commit -qm "$message"
	done
}

@test "a repository's rules and routines run once the user trusts them at a terminal, and again after each change" {
	mkdir -p .jouet/rules .jouet/routines
	rule team ls >.jouet/rules/team.toml
	routine 'team ran' >.jouet/routines/team.luau

	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted +deny +repo:untrusted'
	run --separate-stderr joue team
	assert_failure 4

	# No terminal and no --yes: the user is not there to decide.
	run --separate-stderr jouet trust
	assert_failure 4
	[[ "$stderr" == *"trusting a repository's rules and routines is the user's decision"* ]]

	run in_tty "XDG_STATE_HOME=$XDG_STATE_HOME" jouet trust
	assert_success
	assert_output --partial 'ok    team  deny  a team rule'
	assert_output --partial 'routines: team'
	assert_output --partial '(1 rules, 1 routines)'
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +enabled +deny +repo:trusted'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls -l')"
	assert_output --partial '[jouet rule: team]'
	run --separate-stderr joue team
	assert_output 'team ran'

	# A pull changes the routine: neither runs until trusted again.
	routine 'changed' >.jouet/routines/team.luau
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted +deny +repo:changed'
	assert_line --partial 'changed since it was trusted'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls -l')"
	assert_output ''
	run --separate-stderr joue team
	assert_failure 4
	run --separate-stderr jouet trust --yes
	assert_success

	# Back on the branch trusted first, nothing needs trusting again.
	routine 'team ran' >.jouet/routines/team.luau
	run --separate-stderr joue team
	assert_output 'team ran'

	run --separate-stderr jouet trust --list
	assert_line --regexp '^repo +trusted +.*/repo/\.jouet$'
	assert_line --regexp '^trusted  .*/repo/\.jouet  \(2 versions\)$'

	run --separate-stderr jouet untrust
	assert_output --partial 'no longer trusting'
	run --separate-stderr joue team
	assert_failure 4
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

	run --separate-stderr jouet trust --yes
	assert_failure 1
	assert_line 'FAIL  wrong  deny  says it fires on ls, but matches rm'
	assert_output --partial "its examples do not pass: 1 of its 2 examples fail; see \`jouet rule test wrong\`"
	assert_output --partial 'not trusting'
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted'

	run --separate-stderr jouet --json trust --yes
	assert_failure 1
	run jq -c '[.trusted, (.rules | map(select(.problem)) | map(.name))]' <<<"$output"
	assert_output '[null,["wrong"]]'
}

@test "an agent is denied trusting a repository or adding a plugin, with or without --yes" {
	for command in 'jouet trust' 'jouet trust --yes' \
		'jouet plugin add https://github.com/acme/plugin --yes'; do
		run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload "$command")"
		assert_success
		run jq -r '.hookSpecificOutput | .permissionDecision, .permissionDecisionReason' <<<"$output"
		assert_line --index 0 deny
		assert_line --index 1 --partial "is the user's decision"
		assert_line --index 1 --partial '[jouet rule: trust]'
	done

	# Looking is fine.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'jouet trust --list')"
	assert_output ''
}

@test "the primer says in one line that this repository's rules and routines do not run, and does not ask the agent to trust them" {
	mkdir -p .jouet/rules .jouet/routines
	rule team ls >.jouet/rules/team.toml
	rule other rm >.jouet/rules/other.toml
	routine hi >.jouet/routines/hi.luau

	run --separate-stderr jouet hook claude session-start <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd}')"
	assert_success
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_line --regexp "^jouet: this repository's rules and routines are not trusted, so they do not run \(2 rules and 1 routines in .*/repo/\.jouet\); trusting them is the user's decision, in their own terminal \(\`jouet trust\`\), not yours\.$"

	jouet trust --yes >/dev/null
	run --separate-stderr jouet hook claude session-start <<<"$(jq -cn --arg cwd "$PWD" '{session_id: "s1", cwd: $cwd}')"
	refute_output --partial 'not trusted'
}

@test "a plugin is pinned at a full commit, runs that commit's rules and routines under its name, and is checked against its pin" {
	plugin_repo "$BATS_TEST_TMPDIR/acme" 'first' 'second'
	first=$(git -C "$BATS_TEST_TMPDIR/acme" rev-parse HEAD~1)
	short=${first:0:12}

	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --rev "$short"
	assert_failure 4

	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --rev "$short" --yes
	assert_success
	assert_line 'ok    acme/no-curl  deny  a team rule'
	assert_line 'routines: acme/hello'
	assert_line --regexp "^pinned acme \(.*/acme at $short, 1 rules, 1 routines\) in .*/\.config/jouet/plugins\.toml$"
	run grep '^rev = ' "$HOME/.config/jouet/plugins.toml"
	assert_output "rev = \"$first\""

	# The rule and routine as they were at the pinned commit, under the
	# plugin's name.
	run --separate-stderr jouet rule list
	assert_line --regexp '^acme/no-curl +enabled +deny +plugin:acme +'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial 'first'
	assert_output --partial '[jouet rule: acme/no-curl]'
	refute_output --partial 'second'
	run --separate-stderr joue acme/hello
	assert_output 'first'
	run --separate-stderr joue --list
	assert_line --regexp '^ +from .*/hello\.luau \(plugin:acme\)$'

	run --separate-stderr jouet plugin ls
	assert_line --regexp "^acme  verified  user  .*/acme at $short$"

	# The kept files changed on disk: they stop running until fetched again.
	kept=$(echo "$XDG_STATE_HOME"/jouet/plugins/*/routines/hello.luau)
	sed -i.bak 's/first/tampered/' "$kept" && rm "$kept.bak"
	run --separate-stderr jouet rule list
	assert_line --regexp '^acme/no-curl +untrusted +deny +plugin:acme:mismatch'
	assert_line --partial 'do not match its pin, so it does not run; run `jouet plugin sync`'
	run --separate-stderr joue acme/hello
	assert_failure 4
	run --separate-stderr jouet plugin sync
	assert_success
	assert_line 'ok    acme  fetched'
	run --separate-stderr joue acme/hello
	assert_output 'first'

	# A pin moved to another commit, with the old hash, fails to fetch.
	second=$(git -C "$BATS_TEST_TMPDIR/acme" rev-parse HEAD)
	sed -i.bak "s/$first/$second/" "$HOME/.config/jouet/plugins.toml"
	rm -rf "$XDG_STATE_HOME"/jouet/plugins/*
	run --separate-stderr jouet plugin sync
	assert_failure 5
	assert_line --partial 'hash to'

	run --separate-stderr jouet plugin rm acme
	assert_success
	run --separate-stderr jouet plugin ls
	assert_output 'no plugins are pinned'
}

@test "a plugin ships rules and routines only, from a manifest jouet can read" {
	plugin_files "$BATS_TEST_TMPDIR/acme" hi

	mkdir "$BATS_TEST_TMPDIR/acme/skills"
	echo x >"$BATS_TEST_TMPDIR/acme/skills/x.md"
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --yes
	assert_failure 1
	[[ "$stderr" == *"skills/ is not something a plugin ships; it holds rules/, routines/, formats/ only, and jouet-plugin.toml"* ]]
	rm -r "$BATS_TEST_TMPDIR/acme/skills"

	echo 'hooks = []' >>"$BATS_TEST_TMPDIR/acme/jouet-plugin.toml"
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --yes
	assert_failure 1
	[[ "$stderr" == *'unknown field `hooks`'* ]]

	printf 'name = "acme"\njouet = "999.0"\n' >"$BATS_TEST_TMPDIR/acme/jouet-plugin.toml"
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --yes
	assert_failure 1
	[[ "$stderr" == *"acme needs jouet 999.0 or newer"* ]]

	# A README and the source's own dot-directories are left out, not refused.
	printf 'name = "acme"\n' >"$BATS_TEST_TMPDIR/acme/jouet-plugin.toml"
	mkdir "$BATS_TEST_TMPDIR/acme/.github"
	echo x >"$BATS_TEST_TMPDIR/acme/.github/ci.yml"
	echo '# acme' >"$BATS_TEST_TMPDIR/acme/README.md"
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --yes
	assert_success
	[[ ! -e "$(echo "$XDG_STATE_HOME"/jouet/plugins/*/README.md)" ]]
}

@test "a plugin whose rules fail their examples is not pinned, and a pin written by hand is still held to them" {
	plugin_files "$BATS_TEST_TMPDIR/bad" x
	cat >"$BATS_TEST_TMPDIR/bad/rules/no-curl.toml" <<'TOML'
[[rule]]
name = "wrong"
why = "says it fires on ls, but matches rm"
message = "no"
match = { command = "rm" }
examples = { fires = ["ls"], quiet = ["pwd"] }
TOML

	run --separate-stderr jouet --json plugin add "$BATS_TEST_TMPDIR/bad" --yes
	assert_failure 1
	digest=$(jq -r .digest <<<"$output")
	[[ "$(jq -r .pinned <<<"$output")" == null ]]
	[[ ! -e "$HOME/.config/jouet/plugins.toml" ]]

	mkdir -p "$HOME/.config/jouet"
	cat >"$HOME/.config/jouet/plugins.toml" <<TOML
[plugin.acme]
path = "$BATS_TEST_TMPDIR/bad"
digest = "$digest"
TOML
	rm -rf "$XDG_STATE_HOME/jouet/plugins"
	run --separate-stderr jouet rule list
	assert_line --partial 'plugin acme'
	jouet plugin sync >/dev/null
	run --separate-stderr jouet rule list
	assert_line --regexp '^acme/wrong +broken: its examples do not pass: 1 of its 2 examples fail'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'rm x')"
	assert_output ''
}

@test "a repository's pinned plugins run once the repository is trusted" {
	plugin_repo "$BATS_TEST_TMPDIR/acme" 'from the repo'

	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/acme" --rev HEAD --repo --yes
	assert_success
	assert_line --partial 'needs trusting again'
	[[ -f .jouet/plugins.toml ]]

	run --separate-stderr jouet plugin ls
	assert_line --partial 'pins plugins, which are not fetched or run until it is trusted'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output ''

	# Trusting fetches the plugins it pins and runs their examples too.
	rm -rf "$XDG_STATE_HOME"/jouet/plugins
	run --separate-stderr jouet trust --yes
	assert_success
	assert_line 'routines: acme/hello'
	assert_output --partial 'and the plugins acme'
	run --separate-stderr jouet rule list
	assert_line --regexp '^acme/no-curl +enabled +deny +plugin:acme +'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial 'from the repo'
	run --separate-stderr joue acme/hello
	assert_output 'from the repo'

	# There is no .jouet/rules, and the session reaches the repository through
	# a symlink (as macOS's /var is one): it is still the one trusted.
	ln -s "$BATS_TEST_TMPDIR/work" "$BATS_TEST_TMPDIR/link"
	cd "$BATS_TEST_TMPDIR/link/repo"
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial 'from the repo'
}

@test "the system directory's rules, routines and plugins run whatever the user's settings say" {
	mkdir -p "$JOUET_SYSTEM_DIR/rules" "$JOUET_SYSTEM_DIR/routines" "$JOUET_SYSTEM_DIR/plugins"
	rule org ls 'the organisation says no' >"$JOUET_SYSTEM_DIR/rules/org.toml"
	routine 'org ran' >"$JOUET_SYSTEM_DIR/routines/org-check.luau"
	plugin_files "$JOUET_SYSTEM_DIR/plugins/acme" 'installed by the organisation'
	chmod -R go-w "$JOUET_SYSTEM_DIR"

	mkdir -p "$HOME/.config/jouet"
	printf '[rules]\ndisable = ["org", "acme/no-curl", "git-stash"]\n' >"$HOME/.config/jouet/config.toml"
	export JOUET_RULES_DISABLE=all
	run --separate-stderr jouet rule list
	assert_line --regexp '^org +enabled +deny +enforced +'
	assert_line --regexp '^acme/no-curl +enabled +deny +enforced:acme +'
	assert_line --regexp '^git-stash +disabled'
	assert_line --partial 'org is enforced by '
	assert_line --partial 'so the setting that disables it does not apply'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls')"
	run jq -r '.hookSpecificOutput.permissionDecisionReason' <<<"$output"
	assert_output --partial 'the organisation says no'
	assert_output --partial '[jouet rule: org]'
	run --separate-stderr joue org-check
	assert_output 'org ran'
	run --separate-stderr joue acme/hello
	assert_output 'installed by the organisation'
	run --separate-stderr jouet plugin ls
	assert_line --regexp '^acme  enforced  system  .*/etc-jouet/plugins/acme$'

	# Others could have written it: it is not the organisation's alone.
	chmod g+w "$JOUET_SYSTEM_DIR/rules/org.toml"
	run --separate-stderr jouet rule list
	assert_line --regexp '^org +untrusted +deny +enforced:writable'
	assert_line --regexp 'org\.toml can be written by its group or others, so the enforced rules and routines of .* do not run until only its owner can write them$'
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'ls')"
	assert_output ''
	run --separate-stderr joue org-check
	assert_failure 4
}

@test "a plugin's name is its own: adding one refuses a name already pinned or enforced" {
	plugin_files "$BATS_TEST_TMPDIR/mine" 'mine'
	plugin_files "$BATS_TEST_TMPDIR/theirs" 'theirs'
	jouet plugin add "$BATS_TEST_TMPDIR/mine" --yes >/dev/null

	# The same name, pinned by the repository as well, would make
	# `acme/no-curl` name two rules.
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/theirs" --repo --yes
	assert_failure 4
	[[ "$stderr" == *"there is already a plugin named acme, pinned by the user plugins.toml"* ]]
	[[ ! -e .jouet/plugins.toml ]]

	# Into the same plugins.toml, it replaces the pin.
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/theirs" --yes
	assert_success
	run --separate-stderr joue acme/hello
	assert_output 'theirs'

	# The system's name comes first.
	jouet plugin rm acme >/dev/null
	mkdir -p "$JOUET_SYSTEM_DIR/plugins"
	plugin_files "$JOUET_SYSTEM_DIR/plugins/acme" 'the organisation'"'"'s'
	chmod -R go-w "$JOUET_SYSTEM_DIR"
	run --separate-stderr jouet plugin add "$BATS_TEST_TMPDIR/mine" --yes
	assert_failure 4
	[[ "$stderr" == *"there is already a plugin named acme, installed in $JOUET_SYSTEM_DIR/plugins/acme"* ]]
}

@test "of two plugins with one name, only the first loads, and the enforced one is first" {
	plugin_files "$BATS_TEST_TMPDIR/mine" 'mine'
	jouet plugin add "$BATS_TEST_TMPDIR/mine" --yes >/dev/null
	run --separate-stderr joue acme/hello
	assert_output 'mine'

	# The organisation installs a plugin of the same name afterwards.
	mkdir -p "$JOUET_SYSTEM_DIR/plugins"
	plugin_files "$JOUET_SYSTEM_DIR/plugins/acme" 'the organisation'"'"'s'
	chmod -R go-w "$JOUET_SYSTEM_DIR"

	run --separate-stderr jouet rule list
	assert_line --regexp '^acme/no-curl +enabled +deny +enforced:acme +'
	refute_line --regexp '^acme/no-curl +.* plugin:acme'
	assert_line --partial "note: plugin acme: the plugin acme installed in $JOUET_SYSTEM_DIR/plugins/acme comes first, so this one (pinned by the user plugins.toml) does not run; a plugin's name must be its own"
	run --separate-stderr jouet plugin ls
	assert_line --regexp '^acme  invalid  user  '
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(bash_payload 'curl x')"
	assert_output --partial "the organisation's"
	refute_output --partial 'mine'
	run --separate-stderr joue acme/hello
	assert_output "the organisation's"
}
