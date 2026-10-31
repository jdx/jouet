#!/usr/bin/env bats
# shellcheck disable=SC2016 # backticks in expected output are Markdown

# `jouet docs PKG [SYMBOL]` against the fixture projects in
# test/fixtures/docs: a Cargo.lock with a crate in a fake cargo registry, an
# npm project with node_modules, a uv project with a virtualenv, and a Go
# module with a fake module cache. Nothing is downloaded.

setup() {
	load "test_helper/common_setup"
	_common_setup
	cp -R "$PROJECT_ROOT/test/fixtures/docs/." .
	# The work tree's root: lockfiles above it are not the project's.
	mkdir .git
	export CARGO_HOME="$PWD/cargo-home"
	export GOMODCACHE="$PWD/gomod"
	export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
	unset VIRTUAL_ENV
}

@test "a crate's outline is its public API, re-exports followed to where things are defined" {
	cd rust
	run --separate-stderr jouet docs demo-http
	assert_success
	assert_line --regexp '^demo-http 0\.4\.2 \(rust, Cargo\.lock\) at .*/registry/src/index\.crates\.io-0000000000000000/demo-http-0\.4\.2$'
	assert_line 'A small HTTP client, for jouet'"'"'s tests.'
	# Two `pub use`s away, in src/client/builder.rs.
	assert_line 'struct demo_http::ClientBuilder (src/client/builder.rs:2, 1 member): Builds a [`Client`](super::Client).'
	assert_line 'struct demo_http::Client (src/client.rs:7, 2 members): Sends requests and keeps connections open between them.'
	assert_line '  pub fn get(url: &str) -> Result<String, Error>'
	assert_line 'reexport demo_http::Url (src/lib.rs:12)'
	assert_line 'module demo_http::blocking (src/lib.rs:7): A client that waits for each response.'
	# pub(crate), #[doc(hidden)], #[cfg(test)] and pub items of a private
	# module are not API.
	refute_output --partial internal
	refute_output --partial hidden
	refute_output --partial helper
	refute_output --partial not_exported
}

@test "a symbol prints its signature, doc, source and where it came from" {
	cd rust
	run --separate-stderr jouet docs demo-http Client::get
	assert_success
	assert_line --regexp '^method demo_http::Client::get  .*/demo-http-0\.4\.2/src/client\.rs:22-25$'
	assert_line 'pub fn get(&self, url: &str) -> Result<String, crate::Error>'
	assert_line '  Fails when the server answers with an error status.'
	assert_line '24	        Ok(url.to_string())'
	# A path from the crate's root is exact, so the blocking client's `get`
	# is left out until its module is named.
	refute_output --partial blocking
	run --separate-stderr jouet docs demo-http blocking::Client::get
	assert_success
	assert_line --regexp '^method demo_http::blocking::Client::get  .*/src/blocking/mod\.rs:8-10$'
	# A name that only ends paths matches each, the shortest first.
	run --separate-stderr jouet docs demo-http timeout
	assert_success
	assert_line --index 1 --regexp '^method demo_http::ClientBuilder::timeout  .*/src/client/builder\.rs:8-11$'
}

@test "a type lists its public methods, and a doc above #[derive] is kept" {
	cd rust
	run --separate-stderr jouet docs demo-http demo_http::Client
	assert_success
	assert_line --regexp '^struct demo_http::Client  .*/src/client\.rs:7-9$'
	assert_line '  Sends requests and keeps connections open between them.'
	assert_line 'members:'
	assert_line '  pub fn new() -> Self  // A client with the default settings.'
	assert_line '  pub fn get(&self, url: &str) -> Result<String, crate::Error>  // Sends a GET request to `url`.'
	refute_output --partial private_helper
	refute_output --partial test_only
}

@test "a module's path prints that module's outline" {
	cd rust
	run --separate-stderr jouet docs demo-http blocking
	assert_success
	assert_line 'A client that waits for each response.'
	assert_line 'struct demo_http::blocking::Client (src/blocking/mod.rs:4, 1 member): Waits for each response.'
	refute_output --partial 'demo_http::ClientBuilder'
}

@test "--private includes what the public API leaves out" {
	cd rust
	run --separate-stderr jouet docs demo-http not_exported
	assert_failure 3
	assert_output ''
	assert_stderr --partial 'has no public definition matching `not_exported`'
	assert_stderr --partial '--private shows it'
	run --separate-stderr jouet docs demo-http not_exported --private
	assert_success
	assert_line --regexp '^function demo_http::util::not_exported \(private\)  .*/src/util\.rs:1-1$'
}

@test "an outline over its budget ends with how much it left out" {
	cd rust
	run --separate-stderr jouet docs demo-http --budget 20
	assert_success
	assert_line --regexp '^\.\.\. [0-9]+ more definitions \(raise --budget, or `jouet docs demo-http MODULE` for one module\)$'
}

@test "a locked crate that is not installed exits 4 with the command that installs it" {
	cd rust
	run --separate-stderr jouet docs demo-json
	assert_failure 4
	assert_stderr --partial 'Cargo.lock locks demo-json 1.0.0, but its source is not in'
	assert_stderr --partial 'hint: `cargo fetch` downloads it'
}

@test "an unknown package, symbol or version exits 3" {
	cd rust
	run --separate-stderr jouet docs no-such-crate
	assert_failure 3
	assert_stderr --partial 'names `no-such-crate`'
	run --separate-stderr jouet docs demo-http Cleint
	assert_failure 3
	assert_stderr --partial 'demo-http 0.4.2 has no definition matching `Cleint`'
	run --separate-stderr jouet docs demo-http@0.3.0
	assert_failure 3
	assert_stderr --partial 'Cargo.lock locks demo-http 0.4.2, not 0.3.0'
	run --separate-stderr jouet docs demo-http --lang cobol
	assert_failure 2
}

@test "--json prints one document, with the version and where it came from" {
	cd rust
	run --separate-stderr jouet docs demo-http Client::new --json
	assert_success
	run jq -c '{package, version, language, lockfile, m: .matches[0] | {kind, path, line, signature, doc}}' <<<"$output"
	assert_output '{"package":"demo-http","version":"0.4.2","language":"rust","lockfile":"Cargo.lock","m":{"kind":"method","path":"demo_http::Client::new","line":13,"signature":"pub fn new() -> Self","doc":"A client with the default settings."}}'
	run --separate-stderr jouet docs demo-json --json
	assert_failure 4
	run jq -r '.error.code' <<<"$output"
	assert_output precondition
}

@test "the index is cached by name and version" {
	cd rust
	run --separate-stderr jouet docs demo-http
	assert_success
	run ls "$XDG_CACHE_HOME/jouet/docs/rust"
	assert_output --regexp '^demo-http@0\.4\.2-[0-9a-f]{12}\.json$'
}

@test "an npm package is read from its .d.ts files" {
	cd npm
	run --separate-stderr jouet docs demo-ui
	assert_success
	assert_line --regexp '^demo-ui 2\.1\.0 \(ts, package-lock\.json\) at .*/node_modules/demo-ui$'
	assert_line 'class Button (dist/button.d.ts:8, 3 members): A clickable button.'
	assert_line '  export declare function render(component: Button, node: Element): void'
	refute_output --partial internalHelper
	run --separate-stderr jouet docs demo-ui Button
	assert_success
	assert_line '  render(): string  // Draws the button and returns its HTML.'
	refute_output --partial secret
}

@test "an npm package without types is read from its @types package" {
	cd npm
	run --separate-stderr jouet docs plain-js greet
	assert_success
	assert_line 'note: plain-js ships no types; these are from @types/plain-js 1.0.3'
	assert_line 'export declare function greet(name: string): string'
}

@test "node_modules at another version than the lockfile's exits 4" {
	cd npm
	run --separate-stderr jouet docs demo-old
	assert_failure 4
	assert_stderr --partial 'node_modules has demo-old 0.9.0, but package-lock.json locks 1.0.0'
	assert_stderr --partial '`npm ci`'
	run --separate-stderr jouet docs demo-missing
	assert_failure 4
	assert_stderr --partial 'package-lock.json locks demo-missing 3.0.0, but it is not in node_modules'
}

@test "a Python package's re-exports and __all__ decide its API" {
	cd python
	run --separate-stderr jouet docs demo-req
	assert_success
	assert_line --regexp '^demo-req 2\.3\.1 \(python, uv\.lock\) at .*/\.venv/lib/python3\.12/site-packages$'
	assert_line 'class demo_req.Session (demo_req/sessions.py:1, 2 members): Keeps cookies and connections across requests.'
	assert_line 'function demo_req.get (demo_req/api.py:4): Sends a GET request with a new session.'
	refute_output --partial secret
	refute_output --partial _internal
	run --separate-stderr jouet docs demo-req requests.Session.get
	assert_failure 3
	run --separate-stderr jouet docs demo-req demo_req.Session.get
	assert_success
	assert_line 'also: demo_req.sessions.Session.get'
	assert_line 'def get(self, url: str, **kwargs) -> "Response"'
	assert_line '  Returns the response.'
	# The body after the docstring, not the docstring again.
	assert_line '12	        return Response(url)'
	refute_output --partial _prepare
}

@test "a Python package locked but not in the virtualenv exits 4" {
	cd python
	run --separate-stderr jouet docs demo-missing
	assert_failure 4
	assert_stderr --partial 'uv.lock locks demo-missing 1.0.0, but it is not installed in'
	assert_stderr --partial '`uv sync`'
}

@test "a Go package is read from the module cache at go.mod's version" {
	cd go
	run --separate-stderr jouet docs example.com/demo/kit
	assert_success
	assert_line --regexp '^example\.com/demo/kit v1\.2\.0 \(go, go\.mod\) at .*/gomod/example\.com/demo/kit@v1\.2\.0$'
	assert_line 'Package kit runs small jobs, for jouet'"'"'s tests.'
	assert_line 'function kit.New (kit.go:19): New makes a job with this name.'
	refute_output --partial helper
	run --separate-stderr jouet docs example.com/demo/kit Job.Run
	assert_success
	assert_line 'func (j *Job) Run() (int, error)'
	assert_line '  Run runs the job and reports how long it took.'
	run --separate-stderr jouet docs example.com/demo/missing
	assert_failure 4
	assert_stderr --partial '`go mod download example.com/demo/missing`'
}

@test "a routine looks a definition up through jouet.docs.lookup" {
	unset XDG_CONFIG_HOME
	mkdir -p "$HOME/.config/jouet/routines"
	cat >"$HOME/.config/jouet/routines/sig.luau" <<'LUAU'
--- about: Print a definition's signature
--- capabilities: fs.read
--- usage: arg "<pkg>" help="The package"
--- usage: arg "<symbol>" help="The definition"
return {
	run = function(args)
		local ran = jouet.docs.lookup({ args.pkg, args.symbol })
		return { exit_code = ran.exit_code, version = ran.json.version, signature = ran.json.matches[1].signature }
	end,
}
LUAU
	cd rust
	run --separate-stderr jouet run sig demo-http ClientBuilder::timeout --json
	assert_success
	run jq -cS . <<<"$output"
	assert_output '{"exit_code":0,"signature":"pub fn timeout(mut self, secs: u64) -> Self","version":"0.4.2"}'
}
