#!/usr/bin/env bats

# `jouet read FILE` by outline, symbol and range, and `jouet edit` by symbol,
# as an agent runs them.

setup() {
	load "test_helper/common_setup"
	_common_setup
	unset CLAUDE_CODE_SESSION_ID CODEX_THREAD_ID JOUET_SESSION_ID XDG_CACHE_HOME
	export XDG_CONFIG_HOME="$BATS_TEST_TMPDIR/config"
	export XDG_CACHE_HOME="$BATS_TEST_TMPDIR/cache"
	mkdir -p src
	cat >src/lib.rs <<'RS'
pub struct Client {
    base: String,
}

impl Client {
    pub fn download(&self, path: &str) -> String {
        let url = join_url(&self.base, path);
        fetch(&url)
    }
}

fn join_url(base: &str, path: &str) -> String {
    format!("{base}/{path}")
}

fn fetch(url: &str) -> String {
    url.to_string()
}
RS
	cat >src/main.rs <<'RS'
fn main() {
    let client = Client { base: String::new() };
    client.download("index.html");
}
RS
}

@test "read --outline lists definitions with their scope and lines" {
	for i in $(seq 1 100); do printf '// %d\n' "$i"; done >>src/lib.rs
	run --separate-stderr jouet read src/lib.rs --outline
	assert_success
	assert_output "src/lib.rs: 118 lines, 4 definitions
  1-3    class Client
  6-9    method Client::download
  12-14  function join_url
  16-18  function fetch"
}

@test "read --outline prints a short file whole, with its outline in --json" {
	run --separate-stderr jouet read src/main.rs --outline
	assert_success
	assert_output "src/main.rs: 4 lines, short enough to print whole instead of its outline
1	fn main() {
2	    let client = Client { base: String::new() };
3	    client.download(\"index.html\");
4	}"

	run --separate-stderr jouet read src/lib.rs --outline --json
	assert_success
	assert_equal "$(jq -c '[.short, (.outline | length), .sections[0].end]' <<<"$output")" '[true,4,18]'

	# Already shown this session, it gets only the outline it asked for.
	JOUET_SESSION_ID=s1 run --separate-stderr jouet read src/main.rs --outline
	assert_line --index 0 --partial 'short enough to print whole'
	JOUET_SESSION_ID=s1 run --separate-stderr jouet read src/main.rs --outline
	assert_output "src/main.rs: 4 lines, 1 definitions
  1-4  function main"
}

@test "read --symbol prints the definition with line numbers, and --range prints lines" {
	run --separate-stderr jouet read src/lib.rs --symbol Client::download
	assert_success
	assert_line --index 0 'src/lib.rs: method Client::download (6-9)'
	assert_line --index 1 --regexp '^6	    pub fn download'
	assert_line --index 4 --regexp '^9	    }$'

	run --separate-stderr jouet read src/lib.rs --range 12:13
	assert_output "12	fn join_url(base: &str, path: &str) -> String {
13	    format!(\"{base}/{path}\")"
}

@test "read exits 3 for a missing symbol, naming what the file defines, and 2 for a file it cannot outline" {
	run --separate-stderr jouet read src/lib.rs --symbol upload
	assert_failure 3
	assert_stderr --partial 'it defines: class Client (1-3), method Client::download (6-9)'

	for i in $(seq 1 101); do printf 'note %d\n' "$i"; done >notes.txt
	run --separate-stderr jouet read notes.txt --outline
	assert_failure 2
	printf 'hello\n' >short.txt
	run --separate-stderr jouet read short.txt --outline
	assert_success
	assert_line --index 1 "1	hello"
	run --separate-stderr jouet read short.txt
	assert_success
	assert_output "1	hello"
}

@test "read collapses lines this session already read" {
	export JOUET_SESSION_ID=s1
	run --separate-stderr jouet read src/lib.rs --symbol fetch
	assert_line --index 0 'src/lib.rs: function fetch (16-18)'
	run --separate-stderr jouet read src/lib.rs --range 16:18
	assert_output 'src/lib.rs:16-18 already shown, unchanged (--again prints it)'
	run --separate-stderr jouet read src/lib.rs --range 16:18 --again
	assert_line --index 0 --regexp '^16	fn fetch'
}

@test "read caps code at --max-lines and says what it left out" {
	run --separate-stderr jouet read src/lib.rs --max-lines 2
	assert_line --index 1 ' 2	    base: String,'
	assert_line --index 2 '... 16 more lines (--range 3:18, or raise --max-lines)'
}

@test "edit by symbol survives a reformat, indents the new text and notes a changed signature" {
	# A formatter wraps the call; a search block for the old text would miss.
	cat >src/lib.rs <<'RS'
pub struct Client {
    base: String,
}

impl Client {
    pub fn download(&self, path: &str) -> String {
        let url = join_url(
            &self.base,
            path,
        );
        fetch(&url)
    }
}

fn join_url(base: &str, path: &str) -> String {
    format!("{base}/{path}")
}

fn fetch(url: &str) -> String {
    url.to_string()
}
RS
	run --separate-stderr jouet edit <<'JOUET_EDIT'
=== src/lib.rs#fn download
pub fn download(&self, path: &str, retries: u32) -> String {
    let _ = retries;
    fetch(&join_url(&self.base, path))
}
JOUET_EDIT
	assert_success
	assert_line --index 0 --regexp '^ok +src/lib.rs  1 edit'
	assert_line 'note: the signature of download changed; check its callers'
	run jouet read src/lib.rs --symbol download --again
	assert_output "src/lib.rs: method Client::download (6-9)
6	    pub fn download(&self, path: &str, retries: u32) -> String {
7	        let _ = retries;
8	        fetch(&join_url(&self.base, path))
9	    }"
}

@test "edit by symbol refuses a missing or ambiguous name, or new text that does not parse, and writes nothing" {
	cp src/lib.rs before.rs
	run --separate-stderr jouet edit <<'JOUET_EDIT'
=== src/lib.rs#upload
fn upload() {}
JOUET_EDIT
	assert_failure 4
	assert_line --partial 'edit 1: no definition matches'
	assert_line '      function join_url (12-14)'

	run --separate-stderr jouet edit <<'JOUET_EDIT'
=== src/lib.rs#fetch
fn fetch(url: &str) -> String {
    url.to_string(
JOUET_EDIT
	assert_failure 4
	assert_line --partial 'does not parse'
	run cmp src/lib.rs before.rs
	assert_success

	run --separate-stderr jouet edit --json <<<'[{"path": "src/lib.rs", "symbol": "class fetch", "new": "struct X;"}]'
	assert_failure 4
	run jq -r '.files[0].edits[0].status' <<<"$output"
	assert_output no_match
}

# A Claude Code hook payload in session `s1`.
payload() {
	jq -cn --arg event "$1" --arg tool "$2" --argjson input "$3" --arg cwd "$PWD" \
		'{session_id: "s1", hook_event_name: $event, tool_name: $tool, tool_input: $input, cwd: $cwd}'
}

@test "reading a long file whole warns, and a short or targeted read does not" {
	for i in $(seq 1 500); do printf 'fn f%d() {}\n' "$i"; done >src/long.rs
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Read '{"file_path": "src/long.rs"}')"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial 'jouet read FILE --outline'
	assert_output --partial '[jouet rule: large-read]'

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Read '{"file_path": "src/long.rs", "offset": 100, "limit": 40}')"
	assert_output ""
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Read '{"file_path": "src/lib.rs"}')"
	assert_output ""
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Bash '{"command": "sed -n 1,50p src/long.rs"}')"
	assert_output ""
}

@test "read --map shows the definitions that matter, from the code graph, within the budget" {
	init_repo
	run --separate-stderr jouet read --map
	assert_success
	assert_line --index 0 src/lib.rs
	assert_line '  1: pub struct Client {'
	assert_line '  6:     pub fn download(&self, path: &str) -> String {'

	run --separate-stderr jouet read --map --budget 10
	assert_success
	assert_line --regexp '^\.\.\. [0-9]+ more definitions? in [0-9]+ more files? \(raise --budget, or --focus a file or name\)$'

	# A focused file's own definitions are left out: it is already open.
	run --separate-stderr jouet read --map --focus src/lib.rs --json
	assert_success
	run jq -c '[.files[] | select(.defs) | .path]' <<<"$output"
	assert_output '["src/main.rs"]'
}

@test "read --map needs a work tree, a directory, and its own flags" {
	run --separate-stderr jouet read --map
	assert_failure 4
	assert_stderr --partial 'not inside a git work tree'

	init_repo
	run --separate-stderr jouet read --map nowhere
	assert_failure 3
	run --separate-stderr jouet read --map --outline
	assert_failure 2
	run --separate-stderr jouet read src/lib.rs --budget 10
	assert_failure 2
	run --separate-stderr jouet read
	assert_failure 2
}
