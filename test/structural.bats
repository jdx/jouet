#!/usr/bin/env bats

# `jouet search --structural` and `jouet edit --pattern` as an agent runs
# them: ast-grep patterns, with jouet's budgets and all-or-nothing writes.

# The patterns' `$X` are ast-grep metavariables, single-quoted so the shell
# leaves them alone.
# shellcheck disable=SC2016

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset CLAUDE_CODE_SESSION_ID CODEX_THREAD_ID JOUET_SESSION_ID XDG_CACHE_HOME
	export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
	mkdir -p src app
	cat >src/lib.rs <<'RS'
fn load(path: &str) -> String {
    let text = std::fs::read_to_string(path).unwrap();
    let n = text.len();
    text.trim().to_string().clone().unwrap_or_default();
    format!("{n}")
}
RS
	cat >app/main.py <<'PY'
def run():
    print("hello", 1)
    print()
PY
}

@test "search --structural matches code, grouped by function, in that language only" {
	run --separate-stderr jouet search --structural --lang rust '$X.unwrap()'
	assert_success
	assert_output "src/lib.rs
  function load (1-6)
    2: std::fs::read_to_string(path).unwrap()"

	run --separate-stderr jouet search --structural --lang python 'print($$$ARGS)' --json
	run jq -c '[.files_searched, .matches]' <<<"$output"
	assert_output '[1,2]'
}

@test "search --structural refuses an unbalanced pattern, an unknown language and a missing --lang" {
	run --separate-stderr jouet search --structural --lang rust 'read_to_string($P'
	assert_failure 2
	assert_stderr --partial 'does not parse as rust'

	run --separate-stderr jouet search --structural --lang cobol 'x'
	assert_failure 2
	run --separate-stderr jouet search --structural 'x'
	assert_failure 2
}

@test "edit --pattern rewrites every match, all or nothing" {
	run --separate-stderr jouet edit --pattern 'print($$$ARGS)' --rewrite 'log.info($$$ARGS)' --lang python app
	assert_success
	assert_line --regexp '^ok +app/main.py  1 edit  \+2 -2$'
	run cat app/main.py
	assert_output 'def run():
    log.info("hello", 1)
    log.info()'
}

@test "edit --pattern keeps --dry-run, --check and --revert-on-fail" {
	cp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	run --separate-stderr jouet edit --dry-run --pattern '$X.len()' --rewrite '$X.chars().count()' --lang rust
	assert_success
	assert_line 'dry run: 1 file would be written'
	run cmp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	assert_success

	run --separate-stderr jouet edit --pattern '$X.len()' --rewrite '$X.chars().count()' --lang rust --check false --revert-on-fail
	assert_failure 1
	run cmp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	assert_success
}

@test "edit --pattern writes nothing when nothing matches, or the rewrite would not parse" {
	cp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	run --separate-stderr jouet edit --pattern 'never_called($A)' --rewrite 'x' --lang rust
	assert_failure 4
	assert_stderr --partial 'the pattern matched nothing in 1 rust file'

	run --separate-stderr jouet edit --pattern 'format!($$$A)' --rewrite 'format!($$$A' --lang rust
	assert_failure 4
	assert_line --partial 'does not parse'
	run cmp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	assert_success
}

@test "paths and --rewrite belong to --pattern" {
	run --separate-stderr jouet edit src/lib.rs <<<'=== src/lib.rs'
	assert_failure 2
	run --separate-stderr jouet edit --pattern 'f($A)' --lang rust
	assert_failure 2
}
