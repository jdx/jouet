#!/usr/bin/env bats

# `--callers-of` and `--impact` on `jouet search --structural` and `jouet
# edit --pattern`: the code graph decides which matches a pattern touches.

# The patterns' `$X` are ast-grep metavariables, single-quoted so the shell
# leaves them alone.
# shellcheck disable=SC2016

setup() {
	load "test_helper/common_setup"
	_common_setup
	init_repo
	unset XDG_CACHE_HOME
	export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
	# Two functions named `load`; only config's is the one to change.
	mkdir -p src tests web
	cat >src/lib.rs <<'RS'
pub mod cache;
pub mod config;

pub fn start() -> String {
    let c = config::load("jouet.toml");
    let k = cache::load("jouet.toml");
    println!("{}", config::load("other.toml"));
    format!("{c}{k}")
}
RS
	cat >src/config.rs <<'RS'
pub fn load(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

pub fn reload() -> String {
    load("again.toml")
}
RS
	cat >src/cache.rs <<'RS'
pub fn load(key: &str) -> String {
    key.to_string()
}
RS
	cat >tests/start.rs <<'RS'
#[test]
fn starts() {
    let _ = demo::start();
}
RS
	cat >web/config.ts <<'TS'
export function load(path: string): string {
  return path;
}
TS
	cat >web/cache.ts <<'TS'
export function load(key: string): string {
  return key;
}
TS
	cat >web/app.ts <<'TS'
import { load } from "./config";
import * as cache from "./cache";

export function start(registry: any): string {
  const c = load("jouet.toml");
  const k = cache.load("jouet.toml");
  const d = registry.load("dynamic");
  return c + k + d;
}
TS
	cat >web/app.test.ts <<'TS'
import { start } from "./app";

test("start", () => {
  expect(start({ load: (x: string) => x })).toBe("x");
});
TS
}

@test "edit --callers-of rewrites only the calls of that definition (Rust)" {
	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite '$F($P, None)' --lang rust --callers-of src/config.rs#load
	assert_success
	assert_line 'scope: 2 matches in calls of function load (src/config.rs:1); 2 other matches left alone'
	assert_line '2 of 2 files written'
	run cat src/lib.rs
	assert_line '    let c = config::load("jouet.toml", None);'
	assert_line '    let k = cache::load("jouet.toml");'
	run cat src/config.rs
	assert_line '    load("again.toml", None)'
	run cat src/cache.rs
	assert_line 'pub fn load(key: &str) -> String {'
}

@test "edit --callers-of rewrites only the calls of that definition (TypeScript)" {
	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite '$F($P, {})' --lang typescript --callers-of web/cache.ts#load web
	assert_success
	run cat web/app.ts
	assert_line '  const c = load("jouet.toml");'
	assert_line '  const k = cache.load("jouet.toml", {});'
	assert_line '  const d = registry.load("dynamic");'
}

@test "calls the graph cannot attribute are listed as unresolved and left alone" {
	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite '$F($P, {})' --lang typescript --callers-of web/config.ts#load web --json
	assert_success
	run jq -c '[.scope.matched, [.scope.unresolved[] | [.path, .line, .reason]]]' <<<"$output"
	assert_output '[1,[["web/app.ts",7,"unlinked"]]]'
	run grep -c 'registry.load("dynamic")' web/app.ts
	assert_output 1

	# A call inside a macro is seen by neither the pattern nor the graph, and
	# a call the pattern does not match is listed too.
	run --separate-stderr jouet search --structural --lang rust 'load($$$A)' --callers-of src/config.rs#load
	assert_success
	assert_line '  in_macro: inside a macro, which neither the pattern nor the graph parses'
	assert_line '    src/lib.rs:7  println!("{}", config::load("other.toml"));'
	assert_line '  not_matched: the pattern does not match these calls'
	assert_line '    src/lib.rs:5  let c = config::load("jouet.toml");'
}

@test "--dry-run lists the rewrites and the tests that depend on them, and writes nothing" {
	cp web/app.ts "$BATS_TEST_TMPDIR/before.ts"
	run --separate-stderr jouet edit --dry-run --pattern '$F($P)' --rewrite '$F($P, {})' --lang typescript --callers-of web/config.ts#load web
	assert_success
	assert_line '    web/app.ts:5  load("jouet.toml")  ->  load("jouet.toml", {})'
	assert_line 'tests that depend on the changed files: 1'
	assert_line '    web/app.test.ts'
	assert_line 'dry run: 1 file would be written'
	run cmp web/app.ts "$BATS_TEST_TMPDIR/before.ts"
	assert_success
}

@test "--impact limits matches to the symbol and the code depending on it" {
	run --separate-stderr jouet search --structural --lang rust '$X.to_string()' --impact src/cache.rs#load --json
	assert_success
	run jq -c '[.matches, .scope.matched]' <<<"$output"
	assert_output '[1,1]'

	# config.rs calls functions too, but nothing there depends on cache.
	run --separate-stderr jouet search --structural --lang rust '$F($$$A)' --impact src/cache.rs#load --json
	assert_success
	run jq -c '[.files[].path]' <<<"$output"
	assert_output '["src/cache.rs","src/lib.rs"]'
}

@test "a scope with nothing in it, or a rewrite that would not parse, writes nothing and exits 4" {
	cp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	run --separate-stderr jouet edit --pattern 'never($P)' --rewrite 'x' --lang rust --callers-of src/config.rs#load
	assert_failure 4
	assert_line 'nothing was written: no match of the pattern is in scope'

	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite '$F($P' --lang rust --callers-of src/config.rs#load
	assert_failure 4
	assert_line --partial 'does not parse'
	run cmp src/lib.rs "$BATS_TEST_TMPDIR/before.rs"
	assert_success
}

@test "the symbol must name exactly one definition, and the flags their command" {
	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite 'x' --lang rust --callers-of load
	assert_failure 2
	assert_stderr --partial 'load names 4 symbols; pass one by PATH#NAME or #ID'

	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite 'x' --lang rust --callers-of src/config.rs#missing
	assert_failure 3

	run --separate-stderr jouet edit --pattern '$F($P)' --rewrite 'x' --lang rust --callers-of src/config.rs#load --impact src/config.rs#load
	assert_failure 2
	run --separate-stderr jouet search load --callers-of src/config.rs#load
	assert_failure 2
	run --separate-stderr jouet edit --callers-of src/config.rs#load <<<'=== src/lib.rs'
	assert_failure 2
}
