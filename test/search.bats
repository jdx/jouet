#!/usr/bin/env bats

# `jouet search` and `jouet read --map` as an agent runs them, and the hook
# rules that steer reads and searches towards them.

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

@test "search groups matches by file and by the function they are in" {
	run --separate-stderr jouet search 'join_url|fetch\(' src/lib.rs

	assert_success
	assert_output "src/lib.rs
  method download (6-9)
    7: let url = join_url(&self.base, path);
    8: fetch(&url)
  function join_url (12-14)
    12: fn join_url(base: &str, path: &str) -> String {
  function fetch (16-18)
    16: fn fetch(url: &str) -> String {"
}

@test "search exits 1 with no match, 2 for a bad pattern and 3 for a missing path" {
	run --separate-stderr jouet search 'no such text'
	assert_failure 1
	assert_output --regexp '^no matches \([0-9]+ files searched\)$'

	run --separate-stderr jouet search '('
	assert_failure 2
	assert_stderr --partial 'invalid pattern'

	run --separate-stderr jouet search x missing/
	assert_failure 3
}

@test "search says what a budget left out" {
	run --separate-stderr jouet search --file-budget 40 -F 'url' src/lib.rs
	assert_success
	assert_line --regexp '^  \.\.\. [0-9]+ more in src/lib.rs$'

	run --separate-stderr jouet search --budget 30 'String'
	assert_success
	assert_line --index 0 src/lib.rs
	assert_line --regexp '^\.\.\. [0-9]+ match(es)? in 1 more file: src/main.rs \([0-9]+\) \(narrow the search, or raise --budget\)$'
}

@test "search collapses matches this session already saw, until the file changes" {
	export JOUET_SESSION_ID=s1
	run --separate-stderr jouet search download
	assert_success
	assert_output --partial '6: pub fn download'

	run --separate-stderr jouet search download
	assert_success
	assert_output "src/lib.rs: 1 match already shown, unchanged
src/main.rs: 1 match already shown, unchanged"

	run --separate-stderr jouet search --again download src/main.rs
	assert_output --partial 'client.download("index.html");'

	JOUET_SESSION_ID=s2 run --separate-stderr jouet search download src/main.rs
	assert_output --partial 'client.download("index.html");'

	printf '// download later\n' >>src/main.rs
	run --separate-stderr jouet search download src/main.rs
	assert_line --index 0 src/main.rs
	assert_line --partial '5: // download later'
}

@test "search --json is one document with every match accounted for" {
	run --separate-stderr jouet search --json -F 'url' src/lib.rs
	assert_success
	run jq -c '[.matches, .files_matched, .files[0].path, .files[0].shown[0].symbol.name]' <<<"$output"
	assert_output '[5,1,"src/lib.rs","download"]'
}

@test "search -l lists files with their counts" {
	run --separate-stderr jouet search -l Client
	assert_success
	assert_output "src/lib.rs (2)
src/main.rs (1)"
}

@test "read --map shows the definitions that matter, within the budget" {
	run --separate-stderr jouet read --map
	assert_success
	assert_line --index 0 src/lib.rs
	assert_line '  1: pub struct Client {'
	assert_line '  6:     pub fn download(&self, path: &str) -> String {'

	run --separate-stderr jouet read --map --budget 10
	assert_success
	assert_line --regexp '^\.\.\. [0-9]+ more definitions in [0-9]+ more files \(raise --budget, or --focus a file or name\)$'

	run --separate-stderr jouet read --map --focus src/main.rs --json
	assert_success
	run jq -r '[.files[].path] | join(",")' <<<"$output"
	assert_output src/lib.rs
}

@test "read --map caches tags, and needs --map" {
	run --separate-stderr jouet read --map --json
	run jq '.cached // 0' <<<"$output"
	assert_output 0
	run --separate-stderr jouet read --map --json
	run jq '.cached' <<<"$output"
	assert_output 2

	run --separate-stderr jouet read
	assert_failure 2
}

# A Claude Code hook payload in session `s1`.
payload() {
	jq -cn --arg event "$1" --arg tool "$2" --argjson input "$3" --arg cwd "$PWD" \
		'{session_id: "s1", hook_event_name: $event, tool_name: $tool, tool_input: $input, cwd: $cwd}'
}

@test "reading lines the session already read, unchanged, warns" {
	read_payload="$(payload PreToolUse Read '{"file_path": "src/lib.rs"}')"
	run --separate-stderr jouet hook claude pre-tool-use <<<"$read_payload"
	assert_success
	assert_output ""

	jouet hook claude post-tool-use <<<"$(payload PostToolUse Read '{"file_path": "src/lib.rs"}')"
	run --separate-stderr jouet hook claude pre-tool-use <<<"$read_payload"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial 'This session already read these lines'
	assert_output --partial '[jouet rule: reread]'

	# Part of what was read is seen too, in the shell as with the Read tool.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Bash '{"command": "sed -n 2,5p src/lib.rs"}')"
	assert_output --partial '[jouet rule: reread]'

	# Another file, or a changed one, is not.
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Bash '{"command": "cat src/main.rs"}')"
	assert_output ""
	printf '// more\n' >>src/lib.rs
	run --separate-stderr jouet hook claude pre-tool-use <<<"$read_payload"
	assert_output ""
}

@test "an unbudgeted search of the whole tree warns, and a narrowed one does not" {
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Bash '{"command": "rg -n download"}')"
	run jq -r '.hookSpecificOutput.additionalContext' <<<"$output"
	assert_output --partial 'jouet search PATTERN'
	assert_output --partial '[jouet rule: unbudgeted-search]'

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Grep '{"pattern": "download", "output_mode": "content"}')"
	assert_output --partial '[jouet rule: unbudgeted-search]'

	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Bash '{"command": "rg -n download | head -20"}')"
	assert_output ""
	run --separate-stderr jouet hook claude pre-tool-use <<<"$(payload PreToolUse Grep '{"pattern": "download", "output_mode": "content", "head_limit": 20}')"
	assert_output ""
}
