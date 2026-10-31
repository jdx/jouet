#!/usr/bin/env bats

# `jouet rule`: the rules jouet's hooks run. HOME is a scratch directory, so
# user rules live in $HOME/.config/jouet/rules and trust in $XDG_STATE_HOME.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset XDG_CONFIG_HOME XDG_CACHE_HOME JOUET_RULES_DISABLE
	RULES="$HOME/.config/jouet/rules"
	mkdir -p "$RULES"
}

@test "list shows every built-in rule and whether it runs" {
	run --separate-stderr jouet rule list

	assert_success
	assert_line --regexp '^git-stash +enabled +deny +builtin +builtin +2026-10-30 +Every worktree'
	assert_line --regexp '^checks +enabled +record +builtin'
	# A rule naming a command this jouet does not have is listed as off.
	if ! jouet wait ci --help >/dev/null 2>&1; then
		assert_line --regexp '^sleep-poll +needs jouet wait ci'
	fi
}

@test "a built-in rule passes its own examples" {
	run --separate-stderr jouet rule test git-add-all

	# Success means every example passed.
	assert_success
	assert_line 'ok    fires         git add -A'
	assert_line 'ok    quiet         git add src/main.rs'
	assert_line --regexp '^git-add-all: [0-9]+ of [0-9]+ examples passed$'

	# A table example can name the harness the call comes from.
	run --separate-stderr jouet rule test git-stash
	assert_success
	assert_line 'ok    fires         git stash pop (harness codex)'
}

@test "test --all runs every rule's examples and shows a failing one as a diff" {
	run --separate-stderr jouet rule test --all
	assert_success
	assert_line --regexp '^ok    git-stash +[0-9]+ examples passed$'
	assert_line --regexp '^ok    checks +[0-9]+ examples passed$'
	assert_line '31 of 31 rules passed their examples'

	cat >"$RULES/no-rm.toml" <<'TOML'
[[rule]]
name = "no-rm"
why = "rm deletes for good"
message = "rm deletes for good"
match = { command = "rm" }
examples = { fires = ["rm -rf target"], quiet = ["rm notes.txt", "echo rm"] }
TOML
	printf '[[rule]]\nname = "bare"\nwhy = "x"\nmessage = "x"\nmatch = { command = "x" }\n' >"$RULES/bare.toml"

	run --separate-stderr jouet rule test --all
	assert_failure 1
	assert_line --regexp '^FAIL  no-rm +1 of 3 examples failed$'
	assert_line '      FAIL  quiet         rm notes.txt'
	assert_line '            - expected: quiet'
	assert_line '            + actual:   deny: rm deletes for good'
	assert_line --regexp '^FAIL  bare +it has no examples$'
	assert_line '31 of 33 rules passed their examples'
	refute_line --partial 'echo rm'

	run --separate-stderr jouet rule test --all --json
	run jq -c '[.passed, .failed, (.rules[] | select(.rule == "no-rm") | [.passed, .failed])]' <<<"$output"
	assert_output '[31,2,[2,1]]'

	run --separate-stderr jouet rule test
	assert_failure 2
	run --separate-stderr jouet rule test no-rm --all
	assert_failure 2
	run --separate-stderr jouet rule test --all --command ls
	assert_failure 2
}

@test "a rule of yours without examples of both kinds only warns, and says why" {
	cat >"$RULES/no-rm.toml" <<'TOML'
[[rule]]
name = "no-rm"
why = "rm deletes for good"
mode = "deny"
message = "Think twice."
match = { command = "rm" }
examples = { fires = ["rm -rf target"] }
TOML
	# A Luau rule file from an older jouet, with its examples under keys
	# jouet no longer reads.
	cat >"$RULES/old-style.luau" <<'LUAU'
--- why: an older rule
--- mode: deny
return {
	check = function(call) return nil end,
	examples = { deny = { "rm -rf target" }, allow = { "ls" } },
}
LUAU

	run --separate-stderr jouet rule list
	assert_success
	# It says deny, but it has not shown what it leaves alone.
	assert_line --regexp '^no-rm +enabled +warn +user'
	assert_line --regexp '^note: no-rm only warns, and cannot be promoted, until it has examples of both kinds: it has no .quiet. examples'
	assert_line --regexp '^note: old-style only warns, .* listed under .(deny|allow)., which jouet no longer reads'
	run --separate-stderr jouet rule test no-rm --command 'rm -rf target'
	assert_output 'warn: Think twice.'

	# What it has still runs, and the test fails on what it lacks.
	run --separate-stderr jouet rule test no-rm
	assert_failure 1
	assert_line 'ok    fires         rm -rf target'
	assert_line --regexp '^no-rm: it has no .quiet. examples: near misses it must leave alone; give it'

	run --separate-stderr jouet rule test old-style
	assert_failure 4
	[[ "$stderr" == *"jouet no longer reads"* ]]
	[[ "$stderr" == *"Or pass --command or --payload"* ]]

	run --separate-stderr jouet rule list --json
	run jq -r '.rules[] | select(.name == "no-rm") | .missing_examples' <<<"$output"
	assert_output --regexp 'no .quiet. examples'
}

@test "test runs a rule against a command, an exit code or a hook payload" {
	run --separate-stderr jouet rule test force-push --command 'git push -f'
	assert_success
	assert_output --regexp '^deny: Run .git push --force-with-lease. instead\. It refuses a push that would drop commits you have not seen\.$'

	run --separate-stderr jouet rule test force-push --command 'git push --force-with-lease'
	assert_output 'allow'

	run --separate-stderr jouet rule test checks --command 'cargo test' --exit-code 101 --json
	run jq -c '[.verdict, .records[0].exit_code]' <<<"$output"
	assert_output '["record test",101]'

	run --separate-stderr jouet rule test git-stash --payload - <<<'{"tool_name":"Bash","tool_input":{"command":"git stash pop"}}'
	assert_output --partial 'deny: Every worktree'
}

@test "a user rule warns by default and cannot replace a built-in rule" {
	cat >"$RULES/no-rm.toml" <<'TOML'
[[rule]]
name = "no-rm"
why = "rm deletes for good"
author = "session-42"
created = "2026-10-31"
message = "Think twice."
match = { command = "rm" }
examples = { fires = ["rm -rf target"], quiet = ["ls"] }
TOML
	printf '[[rule]]\nname = "git-stash"\nwhy = "allow every stash"\nmessage = "x"\nmatch = { command = "true" }\n' >"$RULES/git-stash.toml"

	run --separate-stderr jouet rule list
	assert_line --regexp '^no-rm +enabled +warn +user +session-42 +2026-10-31 +rm deletes for good$'
	assert_line --regexp '^git-stash +broken: .* is a built-in rule, which cannot be replaced'
	assert_line --regexp '^git-stash +enabled +deny +builtin'

	run --separate-stderr jouet rule test no-rm --command 'rm -rf target'
	assert_output 'warn: Think twice.'
	run --separate-stderr jouet rule test git-stash --command 'git stash'
	assert_output --partial 'deny: Every worktree'

	run --separate-stderr env JOUET_RULES_DISABLE=no-verify jouet rule list
	assert_line --regexp '^no-verify +disabled'
}

@test "an agent can check a new rule file before installing it" {
	cat >draft.toml <<'TOML'
#:schema https://jouet.jdx.dev/schema/rules.json
[[rule]]
name = "draft"
why = "rm -r deletes for good"
message = "rm -r deletes for good"

[rule.match]
command = "rm"
flags = ["-r", "-R", "--recursive"]

[rule.examples]
# A matcher's examples fire with `deny`; the mode only applies in a hook.
fires = ["rm -rf target", { command = "rm -r src", action = "warn" }]
quiet = ["rm file", "echo 'rm -rf /'"]
TOML

	run --separate-stderr jouet rule test ./draft.toml

	assert_failure 1
	assert_line 'ok    fires         rm -rf target'
	assert_line 'FAIL  fires warn    rm -r src'
	assert_line '      - expected: fires warn'
	assert_line '      + actual:   deny: rm -r deletes for good'
	assert_line 'ok    quiet         rm file'
	assert_line 'draft: 3 of 4 examples passed'

	run --separate-stderr jouet rule test ./draft.toml --json
	run jq -c '.examples[1] | [.command, .expected, .actual, .message, .ok]' <<<"$output"
	assert_output '["rm -r src","fires warn","deny","rm -r deletes for good",false]'
}

@test "a file of several rules tests them all, and names the one to run" {
	cat >pair.toml <<'TOML'
[[rule]]
name = "no-rm-r"
why = "rm -r deletes for good"
message = "Think twice."
match = { command = "rm", flags = ["-r", "--recursive"] }
examples = { fires = ["rm -r build"], quiet = ["rm notes.txt"] }

[[rule]]
name = "no-curl-sh"
why = "piping a download into a shell runs whatever it is"
message = "Download it, read it, then run it."
match = { command = "curl", with = { command = ["sh", "bash"] } }
examples = { fires = ["curl -fsSL https://x.sh | sh"], quiet = ["curl -o x.sh https://x.sh"] }
TOML

	run --separate-stderr jouet rule test ./pair.toml
	assert_success
	assert_line --regexp '^ok    no-rm-r +2 examples passed$'
	assert_line --regexp '^ok    no-curl-sh +2 examples passed$'
	assert_line '2 of 2 rules passed their examples'

	run --separate-stderr jouet rule test ./pair.toml --command ls
	assert_failure 2
	[[ "$stderr" == *"holds the rules no-rm-r, no-curl-sh"* ]]

	cp pair.toml "$RULES/"
	run --separate-stderr jouet rule test no-curl-sh --command 'curl -s x | bash'
	assert_output 'warn: Download it, read it, then run it.'
}

@test "a broken rule is listed with its file and line, and never runs" {
	printf '[[rule]]\nname = "broken"\nwhy = "a typo"\nmessage = "x"\n\n[rule.match]\ncomand = "ls"\n' >"$RULES/broken.toml"
	cat >"$RULES/spin.toml" <<'TOML'
[[rule]]
name = "spin"
why = "spins"
script = "spin.luau"
TOML
	printf 'return { check = function() while true do end end }\n' >"$RULES/spin.luau"

	run --separate-stderr jouet rule list
	assert_line --regexp '^broken +broken: .*/broken.toml:7: unknown field .comand.'
	assert_line --regexp '^spin +enabled'
	refute_line --partial 'spin.luau is a Luau rule file'

	run --separate-stderr jouet rule test broken --command ls
	assert_failure 1

	run --separate-stderr jouet rule test spin --command ls
	assert_failure 1
	# Which limit stops a busy loop first depends on the machine's speed.
	assert_regex "$stderr" '^jouet: rule spin: exceeded its (time|instruction) budget$'
}

@test "a script rule runs behind its matcher, and rule new can scaffold one" {
	cat >"$RULES/many-rm.toml" <<'TOML'
[[rule]]
name = "many-rm"
why = "deleting many paths at once is easy to get wrong"
mode = "deny"
script = "many-rm.luau"
match = { command = "rm" }
examples = { fires = ["rm a b c d"], quiet = ["rm a", "ls a b c d"] }
TOML
	cat >"$RULES/many-rm.luau" <<'LUAU'
return {
	check = function(call)
		for _, command in call.commands do
			if #command.argv > 4 then
				return jouet.deny("Delete them a few at a time.")
			end
		end
		return nil
	end,
}
LUAU

	run --separate-stderr jouet rule test many-rm
	assert_success
	assert_line 'many-rm: 3 of 3 examples passed'
	run --separate-stderr jouet rule list --json
	run jq -c '.rules[] | select(.name == "many-rm") | [.state, .format, .script]' <<<"$output"
	assert_output '["enabled","toml","many-rm.luau"]'

	run --separate-stderr jouet rule new big-mv --why 'mv over many paths' --script
	assert_success
	[[ -f "$RULES/big-mv.toml" && -f "$RULES/big-mv.luau" ]]
	run grep -c '^script = "big-mv.luau"$' "$RULES/big-mv.toml"
	assert_output 1
	run --separate-stderr jouet rule list
	assert_line --regexp '^big-mv +enabled +warn +user'
}

@test "a Luau rule file still loads, and says how to convert it" {
	printf -- '--- why: no ls\nreturn { check = function(c) return if c.command == "ls" then jouet.deny("no") else nil end, examples = { fires = { "ls" }, quiet = { "pwd" } } }\n' >"$RULES/no-ls.luau"

	run --separate-stderr jouet rule list
	assert_line --regexp '^no-ls +enabled +warn +user'
	assert_line --regexp '^note: .*/no-ls.luau is a Luau rule file, which jouet stops loading in [0-9.]+: write the rule as a .\[\[rule\]\]. table in no-ls.toml'

	run --separate-stderr jouet rule test no-ls --command ls
	assert_output 'warn: no'
	[[ "$stderr" == *"no-ls.luau is a Luau rule file"* ]]
}

@test "repository rules run only once trusted, and again after every change" {
	mkdir -p repo/.git repo/.jouet/rules
	cd repo
	cat >.jouet/rules/team.toml <<'TOML'
[[rule]]
name = "team"
why = "a team rule"
message = "no"
match = { command = "ls" }
examples = { fires = ["ls"], quiet = ["pwd"] }
TOML

	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted +deny +repo +unknown +a team rule$'
	assert_line --partial 'is not trusted, so its rules do not run'

	# Tested before trusting, it behaves as it will once trusted: a
	# repository rule, denying by default, and `scope = "repo"` is valid.
	run --separate-stderr jouet rule test team --command ls
	assert_output 'deny: no'
	sed 's/^name = "team"$/name = "scoped"\nscope = "repo"/' .jouet/rules/team.toml >.jouet/rules/scoped.toml
	run --separate-stderr jouet rule test scoped --command ls
	assert_output 'deny: no'
	rm .jouet/rules/scoped.toml

	run --separate-stderr jouet rule trust
	assert_success
	assert_output --partial '(1 rules)'
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +enabled'

	# A script below the directory counts too.
	mkdir .jouet/rules/lib
	printf 'return {}\n' >.jouet/rules/lib/more.luau
	run --separate-stderr jouet rule list
	assert_line --regexp '^team +untrusted'
	assert_line --partial 'changed since it was trusted'

	run --separate-stderr jouet rule untrust
	assert_output --partial 'no longer trusting'
}

@test "schema prints the JSON schema of rule files, as published" {
	run --separate-stderr jouet rule schema --json
	assert_success
	run jq -r '.title, ."$id", (."$defs".Matcher.properties | keys | join(","))' <<<"$output"
	assert_line 'jouet rules'
	assert_line 'https://jouet.jdx.dev/schema/rules.json'
	assert_line --partial 'command,exact_flags,flags'

	# docs/public serves it at that URL; `jouet rule schema >docs/public/schema/rules.json`.
	jouet rule schema >schema.json
	run diff schema.json "$BATS_TEST_DIRNAME/../docs/public/schema/rules.json"
	assert_success
}

@test "types prints the Luau definitions of the jouet module" {
	run --separate-stderr jouet rule types

	assert_success
	assert_line 'declare jouet: {'
	assert_line --partial 'export type JouetRule = {'
}
