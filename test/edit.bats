#!/usr/bin/env bats

# `jouet edit` as an agent runs it from a bash tool: a spec in a heredoc, the
# result on stdout, diagnostics on stderr and an exit code from the shared
# table (0 ok, 1 failed, 2 usage, 3 not found, 4 precondition).

setup() {
	load "test_helper/common_setup"
	_common_setup
}

@test "a spec piped through a heredoc is applied" {
	printf 'fn main() {\n    println!("hello");\n}\n' >main.rs

	run --separate-stderr jouet edit <<'EOF'
=== main.rs
<<<<<<< SEARCH
    println!("hello");
=======
    println!("hello, world");
>>>>>>> REPLACE
EOF

	assert_success
	assert_output $'ok          main.rs  1 edit  +1 -1\n1 of 1 file written'
	assert_equal "$stderr" ""
	assert_content main.rs $'fn main() {\n    println!("hello, world");\n}\n'
}

@test "a heredoc delimiter inside the content cuts the spec short, and nothing is written" {
	printf 'echo old\n' >greet.sh
	# What an agent wrote: the replacement holds a heredoc of its own that ends
	# at EOF, so bash ends jouet's stdin there, inside the block.
	cat >agent.sh <<'AGENT'
jouet edit <<'EOF'; echo "$?" >jouet.status
=== greet.sh
<<<<<<< SEARCH
echo old
=======
cat <<EOF
new
EOF
>>>>>>> REPLACE
EOF
AGENT

	run bash agent.sh

	assert_equal "$(cat jouet.status)" 2
	assert_output --partial "jouet: spec line 2: the spec ends inside the block opened here (no \`>>>>>>> REPLACE\` line)"
	assert_content greet.sh $'echo old\n'
}

@test "search/replace blocks edit several files and create one, all or nothing" {
	printf 'one\ntwo\n' >a.txt
	printf 'x\nx\nx\n' >b.txt

	run --separate-stderr jouet edit --json <<'EOF'
=== a.txt
<<<<<<< SEARCH
one
=======
1
>>>>>>> REPLACE
=== b.txt
<<<<<<< SEARCH count=all
x
=======
y
>>>>>>> REPLACE
=== new/c.txt
<<<<<<< SEARCH
=======
created
>>>>>>> REPLACE
EOF

	assert_success
	assert_equal "$(jq -r '.files_written' <<<"$output")" 3
	assert_equal "$(jq -r '[.files[].status] | join(",")' <<<"$output")" "written,written,created"
	assert_equal "$(jq -r '.files[1].edits[0].lines | join(",")' <<<"$output")" "1,2,3"
	assert_content a.txt $'1\ntwo\n'
	assert_content b.txt $'y\ny\ny\n'
	assert_content new/c.txt $'created\n'
}

@test "one edit that does not match leaves every file as it was and exits 4" {
	printf 'one\n' >a.txt
	printf 'two\ntwo\ntwo\n' >b.txt

	run --separate-stderr jouet edit <<'EOF'
=== a.txt
<<<<<<< SEARCH
one
=======
1
>>>>>>> REPLACE
=== b.txt
<<<<<<< SEARCH count=2
two
=======
2
>>>>>>> REPLACE
EOF

	assert_failure 4
	assert_line "not written a.txt  1 edit  +1 -1"
	assert_line "  edit 1: 3 matches, expected 2, at lines 1, 2, 3"
	assert_line "0 of 2 files written"
	assert_content a.txt $'one\n'
	assert_content b.txt $'two\ntwo\ntwo\n'
}

@test "a near miss shows the closest text in the file" {
	printf 'fn main() {\n    println!("hello");\n}\n' >main.rs

	run --separate-stderr jouet edit <<'EOF'
=== main.rs
<<<<<<< SEARCH
    println!("helo");
=======
    println!("bye");
>>>>>>> REPLACE
EOF

	assert_failure 4
	assert_line "  edit 1: no match"
	assert_line --partial "    closest at line 2"
	assert_line '    -    println!("helo");'
	assert_line '    +    println!("hello");'
}

@test "a search holding the start of a line says which line holds it" {
	# shellcheck disable=SC2016 # the backticks are Markdown, not command substitution
	printf '# Notes\n- **Docs.** `docs/benchmarks.md` now recommends hyperfine.\n' >NOTES.md

	run --separate-stderr jouet edit <<'EOF'
=== NOTES.md
<<<<<<< SEARCH
- **Docs.** `docs/benchmarks.md` now recommends
=======
- **Docs.** `docs/benchmarks.md` recommends
>>>>>>> REPLACE
EOF

	assert_failure 4
	# shellcheck disable=SC2016 # the backticks are jouet's output, not command substitution
	assert_line '  edit 1: no match as whole lines; the text occurs inside line 2 (SEARCH blocks hold whole lines, or use `jouet edit replace` for text inside a line)'
	assert_content NOTES.md $'# Notes\n- **Docs.** `docs/benchmarks.md` now recommends hyperfine.\n'

	run --separate-stderr jouet edit --json <<'EOF'
=== NOTES.md
<<<<<<< SEARCH
- **Docs.** `docs/benchmarks.md` now recommends
=======
x
>>>>>>> REPLACE
EOF

	assert_failure 4
	assert_equal "$(jq -c '.files[0].edits[0].near_miss' <<<"$output")" '{"kind":"inside_line","lines":[2]}'
}

@test "a search with other indentation says the whitespace differs" {
	printf 'fn main() {\n\tprintln!("hello");\n}\n' >main.rs

	run --separate-stderr jouet edit <<'EOF'
=== main.rs
<<<<<<< SEARCH
    println!("hello");
=======
    println!("bye");
>>>>>>> REPLACE
EOF

	assert_failure 4
	assert_line "  edit 1: no match; the text matches at line 2 once indentation and trailing whitespace are ignored (copy the file's whitespace exactly)"
}

@test "a Codex patch adds, updates, moves and deletes files" {
	printf 'fn a() {\n    1\n}\n' >a.rs
	printf 'old\n' >old.rs
	printf 'gone\n' >gone.rs

	run --separate-stderr jouet edit --json <<'EOF'
*** Begin Patch
*** Update File: a.rs
@@ fn a() {
-    1
+    2
*** Update File: old.rs
*** Move to: src/new.rs
@@
-old
+new
*** Add File: added.rs
+fresh
*** Delete File: gone.rs
*** End Patch
EOF

	assert_success
	assert_equal "$(jq -r '[.files[].status] | join(",")' <<<"$output")" "written,moved,created,deleted"
	assert_content a.rs $'fn a() {\n    2\n}\n'
	assert_content src/new.rs $'new\n'
	assert_content added.rs $'fresh\n'
	[[ ! -e old.rs ]] || false
	[[ ! -e gone.rs ]] || false
}

@test "a Codex patch without its End Patch line is refused as cut short" {
	printf 'x\n' >a.rs

	run --separate-stderr jouet edit <<'EOF'
*** Begin Patch
*** Update File: a.rs
-x
+y
EOF

	assert_failure 2
	assert_output ""
	assert_equal "$stderr" "jouet: the patch has no \`*** End Patch\` line, so it may have been cut short; nothing was applied"
	assert_content a.rs $'x\n'
}

@test "a JSON spec replaces text anywhere in a line" {
	printf 'let a = foo(foo);\n' >a.rs

	run --separate-stderr jouet edit --json <<'EOF'
[
  {"path": "a.rs", "old": "foo", "new": "bar", "count": "all"},
  {"path": "b.rs", "new": "created\n"}
]
EOF

	assert_success
	assert_equal "$(jq -r '.files[0].edits[0].found' <<<"$output")" 2
	assert_content a.rs $'let a = bar(bar);\n'
	assert_content b.rs $'created\n'
}

@test "--dry-run --diff reports the change without writing it" {
	printf 'one\n' >a.txt

	run --separate-stderr jouet edit replace a.txt --old one --new 1 --dry-run --diff

	assert_success
	assert_line "--- a/a.txt"
	assert_line "+++ b/a.txt"
	assert_line "-one"
	assert_line "+1"
	assert_line "dry run: 1 file would be written"
	assert_content a.txt $'one\n'
}

@test "--check that passes keeps the edits and reports the check" {
	printf 'one\n' >a.txt

	run --separate-stderr jouet edit replace a.txt --old one --new 1 --json --check 'grep -qx 1 a.txt'

	assert_success
	assert_equal "$(jq -r '.check.passed' <<<"$output")" true
	assert_equal "$(jq -r '.check.exit_code' <<<"$output")" 0
	assert_content a.txt $'1\n'
}

@test "--check that fails with --revert-on-fail restores every file and exits 1" {
	printf 'one\n' >a.txt
	printf 'two\n' >b.txt

	run --separate-stderr jouet edit --check 'echo "tests failed" >&2; exit 3' --revert-on-fail <<'EOF'
=== a.txt
<<<<<<< SEARCH
one
=======
1
>>>>>>> REPLACE
=== b.txt
<<<<<<< SEARCH
two
=======
2
>>>>>>> REPLACE
=== c.txt
<<<<<<< SEARCH
=======
three
>>>>>>> REPLACE
EOF

	assert_failure 1
	assert_line "reverted    a.txt  1 edit  +1 -1"
	assert_line 'check failed (exit 3): echo "tests failed" >&2; exit 3'
	assert_line "  tests failed"
	assert_line "  every edited file was restored"
	assert_line "0 of 3 files written"
	assert_content a.txt $'one\n'
	assert_content b.txt $'two\n'
	[[ ! -e c.txt ]] || false

	# The full log is kept where the report says.
	local log
	log="$(sed -n 's/^  full log: //p' <<<"$output")"
	[[ "$log" == "$JOUET_LOG_DIR"/* ]] || false
	assert_content "$log" $'tests failed\n'
}

@test "--check that fails without --revert-on-fail keeps the edits and exits 1" {
	printf 'one\n' >a.txt

	run --separate-stderr jouet edit replace a.txt --old one --new 1 --json --check 'exit 5'

	assert_failure 1
	assert_equal "$(jq -r '.check.exit_code' <<<"$output")" 5
	assert_equal "$(jq -r '.check.reverted // false' <<<"$output")" false
	assert_content a.txt $'1\n'
}

@test "edit replace needs exactly one match unless --count or --all says otherwise" {
	printf 'a a a\n' >a.txt

	run --separate-stderr jouet edit replace a.txt --old a --new b
	assert_failure 4
	assert_line "  edit 1: 3 matches, expected 1, at lines 1, 1, 1"

	run --separate-stderr jouet edit replace a.txt --old a --new b --count 2
	assert_failure 4

	run --separate-stderr jouet edit replace a.txt --old a --new b --count 3
	assert_success
	assert_content a.txt $'b b b\n'

	run --separate-stderr jouet edit replace a.txt --old b --new c --all
	assert_success
	assert_content a.txt $'c c c\n'
}

@test "edit replace takes several files and pairs, and writes none unless all match" {
	printf 'retry_limit = 3\n' >config.py
	printf 'def go(retry_limit):\n    return retry_limit\n' >client.py
	printf 'no setting here\n' >other.py

	run --separate-stderr jouet edit replace config.py client.py other.py --old retry_limit --new max_attempts --all
	assert_failure 4
	assert_line --regexp '^failed +other\.py +1 edit$'
	assert_line '0 of 3 files written'
	assert_content config.py $'retry_limit = 3\n'

	run --separate-stderr jouet edit replace config.py client.py --old retry_limit --new max_attempts --all
	assert_success
	assert_line '2 of 2 files written'
	assert_content client.py $'def go(max_attempts):\n    return max_attempts\n'

	# The Nth --new replaces the Nth --old.
	run --separate-stderr jouet edit replace config.py --old max_attempts --old 3 --new attempts --new 5
	assert_success
	assert_content config.py $'attempts = 5\n'

	run --separate-stderr jouet edit replace config.py --old a --old b --new c
	assert_failure 2
	assert_stderr --partial 'give one --new for each --old (got 2 --old and 1 --new)'
	run --separate-stderr jouet edit replace --old a --new b
	assert_failure 2
}

@test "edit hash and --expect-hash refuse to edit a file that changed since it was read" {
	printf 'one two\n' >a.txt

	run --separate-stderr jouet edit hash a.txt --json
	assert_success
	local hash
	hash="$(jq -r '.files[0].sha256' <<<"$output")"
	assert_equal "$hash" "$(sha256_of a.txt)"

	run --separate-stderr jouet edit hash a.txt
	assert_output "$hash  a.txt"

	run --separate-stderr jouet edit replace a.txt --old two --new 2 --expect-hash "a.txt=$hash"
	assert_success
	assert_content a.txt $'one 2\n'

	# The same hash is stale now.
	run --separate-stderr jouet edit replace a.txt --old one --new 1 --expect-hash "a.txt=$hash" --json
	assert_failure 4
	assert_equal "$(jq -r '.files[0].error.code' <<<"$output")" hash_mismatch
	assert_content a.txt $'one 2\n'
}

@test "usage errors exit 2 with the reason on stderr and nothing on stdout" {
	printf 'one\n' >a.txt

	run --separate-stderr jouet edit --bogus </dev/null
	assert_failure 2
	assert_output ""
	[[ "$stderr" == *"unexpected argument '--bogus'"* ]] || false

	run --separate-stderr jouet edit <<<"just some text"
	assert_failure 2
	assert_output ""
	[[ "$stderr" == "jouet: could not tell the spec's format from its first line"* ]] || false

	run --separate-stderr jouet edit --format yaml <<<"=== a.txt"
	assert_failure 2
	assert_equal "$stderr" "jouet: --format must be blocks, patch or json, not \`yaml\`"

	run --separate-stderr jouet edit replace a.txt --old one --new 1 --expect-hash a.txt
	assert_failure 2
	assert_equal "$stderr" "jouet: --expect-hash takes PATH=SHA256, not \`a.txt\`"

	run --separate-stderr jouet edit replace a.txt --old one --new 1 --revert-on-fail
	assert_failure 2
	assert_equal "$stderr" "jouet: --revert-on-fail needs --check or --diag"

	assert_content a.txt $'one\n'
}

@test "a missing file exits 3" {
	run --separate-stderr jouet edit <<'EOF'
=== missing.txt
<<<<<<< SEARCH
x
=======
y
>>>>>>> REPLACE
EOF
	assert_failure 3
	assert_line --partial "missing.txt"

	run --separate-stderr jouet edit hash missing.txt
	assert_failure 3
	assert_equal "$stderr" "jouet: missing.txt: no such file"
}

@test "--json prints exactly one document for every exit code" {
	printf 'one\n' >a.txt
	local spec=$'=== a.txt\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n'
	local cut=$'=== a.txt\n<<<<<<< SEARCH\none\n=======\n'
	local missing=$'=== b.txt\n<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n'

	run --separate-stderr jouet edit --json --dry-run <<<"$spec"
	assert_success
	assert_equal "$(jq -s length <<<"$output")" 1
	assert_equal "$(jq -r '.dry_run' <<<"$output")" true

	run --separate-stderr jouet edit --json --check false <<<"$spec"
	assert_failure 1
	assert_equal "$(jq -s length <<<"$output")" 1
	assert_equal "$(jq -r '.check.exit_code' <<<"$output")" 1

	run --separate-stderr jouet edit --json <<<"$cut"
	assert_failure 2
	assert_equal "$(jq -s length <<<"$output")" 1
	assert_equal "$(jq -r '.error.code' <<<"$output")" usage
	assert_equal "$stderr" ""

	run --separate-stderr jouet edit --json <<<"$missing"
	assert_failure 3
	assert_equal "$(jq -s length <<<"$output")" 1
	assert_equal "$(jq -r '.files[0].error.code' <<<"$output")" not_found

	run --separate-stderr jouet edit --json <<<"${spec//one/zero}"
	assert_failure 4
	assert_equal "$(jq -s length <<<"$output")" 1
	assert_equal "$(jq -r '.files[0].edits[0].status' <<<"$output")" no_match

	# Fields that are false or empty are left out rather than printed.
	assert_equal "$(jq -r 'has("applied")' <<<"$output")" false
}
