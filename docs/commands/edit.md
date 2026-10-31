---
description: Exact, checked, all-or-nothing text edits across files, from search/replace blocks, a Codex-style patch or JSON.
---

# jouet edit

`jouet edit` makes exact, checked, all-or-nothing text edits across files. Use it where an agent would write a Python heredoc with `str.replace` and a hand-written `assert s.count(old) == 1`. It also accepts the patch format OpenAI Codex uses, from any agent.

```text
jouet edit [--spec FILE] [--format blocks|patch|json] [--dry-run] [--diff]
           [--check CMD] [--diag] [--revert-on-fail] [--expect-hash PATH=SHA256]...
jouet edit replace PATH... (--old TEXT --new TEXT)... [--count N | --all]
jouet edit hash PATH...
```

`jouet edit` reads a spec from `--spec FILE` or from stdin. The flags are listed in the [CLI reference](/cli/edit/), and [Why jouet](/why#editing-two-files) compares it with the heredoc it replaces.

## How an edit runs

- **All or nothing.** Every edit in every file is applied in memory first. If any edit fails, no file is written and the output says so: `0 of 2 files written`. Each file is written to a temporary file beside it and renamed into place, keeping its permissions, and if a write fails part way, the files already written are restored.
- **In order.** Edits to one file apply in the order given, and each one sees the result of the one before. Two spellings of the same file, such as `a.rs` and `sub/../a.rs` or its absolute path, are the same file.
- **Exact.** Search text matches exactly, whitespace included. In a file with CRLF line endings, line breaks in the search and replacement text are treated as CRLF.
- **Diagnostics, not guesses.** jouet never applies a near match. When search text matches nothing, it shows the closest region of the file and a short diff against what you searched for, which shows at a glance when a formatter re-wrapped a line. When the text does occur, but inside a line or only with other indentation, it says so and where. When it matches too often, it lists the line of every match, so you can add context or set a count.
- **Every problem at once.** A failed edit does not stop the others from being checked, so one run reports everything wrong with a spec.

## Spec formats

The format is detected from the first non-blank line: `=== PATH` for blocks, `*** Begin Patch` for a patch, `[` or `{` for JSON. `--format` overrides the detection.

### Search/replace blocks

The format most models already write reliably:

```spec
=== src/backend/vfox.rs
<<<<<<< SEARCH
        let resp = client.get(url).send()?;
=======
        let resp = client
            .get(url)
            .timeout(settings.http_download_timeout())
            .send()?;
>>>>>>> REPLACE

=== src/lockfile/generate.rs
<<<<<<< SEARCH count=2
use crate::http::HTTP;
=======
use crate::http::HTTP_FETCH;
>>>>>>> REPLACE
```

- `=== PATH` starts a file's section. Every block belongs to the section above it, and a section can appear more than once.
- Block search text matches whole lines, indentation included. A search that holds the start of a line that goes on matches nothing; use `jouet edit replace` for text inside a line.
- A block must match exactly once, unless it sets a count: `count=N` needs exactly `N` matches, and `count=all` replaces every match but still needs at least one.
- An empty search creates the file with the replacement as its content. The file must not exist yet.
- When the text being edited contains conflict markers itself, lengthen the fence. A block opened with ten `<` must be closed with ten `>`, and its separator is ten `=`:

  ```spec
  === src/merge.rs
  <<<<<<<<<< SEARCH
  <<<<<<< HEAD
  =======
  ==========
  resolved
  >>>>>>>>>> REPLACE
  ```

- Anything else outside a block, such as a line of prose, is an error.

### Symbol targets

A section headed `=== PATH#SYMBOL` replaces a whole definition, found with tree-sitter rather than by its text, so a formatter that reflowed it cannot break the match. The new text runs to the next `===` line or the end of the spec:

```spec
=== src/backend/vfox.rs#Vfox::download
pub fn download(&self, url: &str, retries: u32) -> Result<()> {
    let resp = self.client.get(url).send()?;
    retry(resp, retries)
}

=== src/lockfile/generate.rs
<<<<<<< SEARCH
use crate::http::HTTP;
=======
use crate::http::HTTP_FETCH;
>>>>>>> REPLACE
```

- `SYMBOL` is a name (`download`), a kind and a name (`fn download`, `class Cart`, `trait Backend`), or a name with its scope (`Vfox::download`, `Shop.Cart`), as [`jouet read FILE --outline`](/commands/read) lists them. It must match exactly one definition; when it matches none, the report lists the file's definitions, and when it matches several, each of them.
- The definition is its signature and body. Comments and attributes above it stay as they are.
- Write the new text at any indentation: it is moved to where the definition sits.
- An edit that would leave the file with more syntax errors than it has is refused (`does_not_parse`). This also catches new text cut short.
- When the edit changes the definition's first line, its signature in most languages, its callers may need changing too. If the work tree has a [code graph](/commands/graph), the report lists the definition's callers in files the edit does not change (`callers` in JSON). Otherwise, or when the graph does not know the definition, it notes the change (`signature` in JSON).

Symbol targets work for Rust, Python, JavaScript, TypeScript, Go, Ruby, Java, C and C++. In JSON, give `symbol` instead of `old`: `{ "path": "src/a.rs", "symbol": "fn download", "new": "..." }`.

### Structural rewrites

`jouet edit --pattern PAT --rewrite REPL --lang LANG [PATH]...` rewrites every match of a code pattern under the paths (the current directory by default), instead of reading a spec. Matching is [ast-grep](https://ast-grep.github.io)'s (`ast-grep-core`, MIT), on the same grammars as symbol targets:

```output
$ jouet edit --pattern 'print($$$ARGS)' --rewrite 'log.info($$$ARGS)' --lang python app
ok          app/main.py  1 edit  +2 -2
1 of 1 file written
```

- **All or nothing, as any edit.** `--dry-run`, `--diff`, `--check` and `--revert-on-fail` work as with a spec. A rewrite that would add syntax errors to a file is refused, and so is a file that changed after the pattern matched it.
- **No match, no write.** A pattern that matches nothing exits 4 and says how many files of the language it read.
- **Languages:** `rust`, `python`, `javascript`, `typescript`, `tsx`, `go`, `ruby`, `java`, `c` and `cpp`. Only files in that language are read.

#### Scoped by the code graph

A pattern such as `load($$$A)` matches every function named `load`. The [code graph](/commands/graph) knows which `load` each call means, so two flags narrow a rewrite to one symbol, given as `PATH#NAME` (or `#ID` from `jouet graph symbols`):

- `--callers-of PATH#SYMBOL` rewrites only the calls the graph attributes to that symbol. A match counts when it starts where such a call starts and names the symbol, so write the pattern for the whole call, receiver included: `$F($P)` or `$R.load($$$A)`, not `load` alone.
- `--impact PATH#SYMBOL` rewrites only inside the symbol and the code that depends on it, three hops back, as `jouet graph impact` counts them.

```output
$ jouet edit --dry-run --pattern '$F($P)' --rewrite '$F($P, {})' --lang typescript --callers-of web/config.ts#load
scope: 1 match in calls of function load (web/config.ts:1); 4 other matches left alone
    web/app.ts:5  load("jouet.toml")  ->  load("jouet.toml", {})
unresolved, left alone: 1 (check them by hand)
  unlinked: the graph could not tell which definition these call
    web/app.ts:7  registry.load("dynamic")
tests that depend on the changed files: 1
    web/app.test.ts
ok          web/app.ts  1 edit  +1 -1
dry run: 1 file would be written
```

- **Nothing the graph cannot attribute is rewritten, or dropped without a word.** A match is listed as unresolved, and left alone, when the graph recorded the call but could not tell which definition it means (`unlinked`, as with dynamic dispatch), linked it to more than one (`ambiguous`), has not read the file (`not_indexed`), or records no call starting there (`no_site`). A call written inside a macro, which neither the pattern nor the graph parses, is listed as `in_macro`.
- **Calls the rewrite did not reach are listed too:** the symbol's calls the pattern does not match (`not_matched`), and those outside the paths given (`not_searched`).
- **The tests to run afterwards.** The report lists each rewrite and the test files that depend on the changed files (the graph's `affected`), with `--dry-run` before anything is written, and after a write.
- **The same refusals.** A rewrite that adds syntax errors, or a file that changed after it was matched, is refused. A scope with no match in it writes nothing and exits 4. The symbol must name exactly one definition (exit 2 lists the candidates; exit 3 when there is none).
- **The graph stays current.** It is brought up to date before matching, and the files written are re-indexed afterwards, as after any `jouet edit`.

`jouet search --structural` takes the same two flags, to see the scope before rewriting. In JSON, both commands add a `scope` object with `matched`, `skipped`, `unresolved`, `unmatched`, and for edits `changes` and `tests`.

Agents often write patterns that match nothing. The rules that matter most:

- **Quote patterns in single quotes.** In double quotes the shell expands `$X`, so `"$X.unwrap()"` becomes `.unwrap()`.
- **`$X` matches one node, `$$$X` any number.** `foo($A)` matches only calls with one argument; `foo($$$ARGS)` matches every call. Metavariable names are uppercase, and `$_` matches without capturing.
- **A pattern is whole code in the language.** `client.get($URL)` matches, `get($URL` is refused. A Rust call pattern matches the call itself, not a `?` or `;` after it, so `$C.get($URL).send()?` matches the whole `?` expression.
- **A name used twice must match the same text:** `$A == $A`.
- **The rewrite is text** with the pattern's metavariables filled in. Its indentation follows the match.
- **Try it first** with `jouet search --structural --lang LANG 'PAT'`, which shows each match under the function it is in.

These rules and worked examples are also in `jouet agent-help edit`.

### Codex-style patch

The format used by OpenAI Codex's `apply_patch`, accepted as is:

```spec
*** Begin Patch
*** Add File: docs/new.md
+# A new page
*** Delete File: docs/old.md
*** Update File: src/a.rs
*** Move to: src/b.rs
@@ fn main() {
     let x = 1;
-    let y = 2;
+    let y = 3;
*** End Patch
```

- `*** Add File:` creates a file from the `+` lines that follow. `*** Delete File:` removes one. `*** Update File:` changes one, and an optional `*** Move to:` straight after it renames it, keeping its mode.
- Each hunk is a run of context lines (starting with a space), removed lines (`-`) and added lines (`+`).
- `@@ TEXT` is an anchor: the hunk is searched for after the first line equal to `TEXT` (ignoring surrounding whitespace if no line is exactly equal). Several `@@` lines narrow the search one after another.
- Hunks are searched for in order, each after the one before, and must match exactly once in what remains.
- `*** End of File` pins the hunk above it to the end of the file.
- A patch without its `*** End Patch` line is refused, because it may have been cut short.

### JSON

For specs that programs generate:

```json
[
  { "path": "src/a.rs", "old": "let y = 2;", "new": "let y = 3;" },
  { "path": "src/a.rs", "old": "HTTP", "new": "HTTP_FETCH", "count": "all" },
  { "path": "docs/new.md", "new": "# A new page\n" }
]
```

- `old` matches anywhere in the file, like `str::replace`, not only whole lines.
- `count` is a positive number or `"all"`, and defaults to `1`.
- `symbol` replaces a whole definition, as a [symbol target](#symbol-targets) does, and takes `new` only.
- An entry without `old` creates the file.

## Edit from the command line

`jouet edit replace` makes replacements without a spec. Like a JSON edit, `--old` matches anywhere in a file, and it must match exactly once in each file unless `--count N` or `--all` says otherwise. A refusal for the wrong number of matches names both flags with the count it found (`add --all to replace every match, or --count 3 to require exactly 3`), in the text and as the edit's `message` in `--json`:

```sh
jouet edit replace src/main.rs --old 'let x = 1;' --new 'let x = 2;'
jouet edit replace src/main.rs --old 'HTTP' --new 'HTTP_FETCH' --all
```

It takes several files, and `--old` and `--new` repeat: the Nth `--new` replaces the Nth `--old`, and every replacement applies to every file. A rename across files is one call, and like any edit it lands in every file or in none:

```sh
jouet edit replace shipit/config.py shipit/cli.py --old retry_limit --new max_attempts --all
jouet edit replace app.py --old 'reverse=True)' --new ')' --old 'math.floor' --new 'math.ceil'
```

The edit flags below work with `replace` too.

## Preview with `--dry-run` and `--diff`

`--dry-run` checks every edit and reports what would change, without writing anything. `--diff` adds a unified diff of each file to the report, with or without `--dry-run`:

```output
$ jouet edit replace src/main.rs --old x --new y --all --diff
ok          src/main.rs  1 edit  +2 -2
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,4 @@
 fn main() {
-    let x = 2;
-    println!("{x}");
+    let y = 2;
+    println!("{y}");
 }
1 of 1 file written
```

## Check, and revert on failure

`--check CMD` runs a command through the shell (`sh -c`, or `cmd /C` on Windows) after the files are written, for example `cargo check -q`. Its output goes to a [log file](/conventions#full-logs-are-kept), and the report shows the last 30 lines when it fails. A failing check makes the exit code `1`.

With `--revert-on-fail`, a failing check also restores every edited file to its previous bytes and permissions, and removes the files the edit created:

```output
$ jouet edit replace src/main.rs --old 'let y = 2' --new 'let y = 5' \
    --check 'grep -q "y = 2" src/main.rs' --revert-on-fail
reverted    src/main.rs  1 edit  +1 -1
check failed (exit 1): grep -q "y = 2" src/main.rs
  full log: /home/me/.local/state/jouet/logs/edit-check-1793373365910-3807454.log
  every edited file was restored
0 of 1 file written
```

If the check cannot start at all, for example because the log directory cannot be written or there is no shell, that also counts as a failure: the exit code is `1`, the error says whether the edits were kept, and with `--revert-on-fail` every edited file is restored the same way:

```output
$ jouet edit replace c.txt --old 'a = 1' --new 'a = 2' --check true --revert-on-fail
jouet: running --check: No such file or directory (os error 2); every edited file was restored
```

This is what mutation testing needs: an edit that does not apply, or does not compile, can never be mistaken for a surviving mutation.

## Fail only on new diagnostics with `--diag`

`--diag` checks the edited files with the project's hk checks before writing them and again after, and fails (exit `1`) only when the edit **introduced** a finding, or made a step that reports no diagnostics fail. Findings that were already there, including ones the edit moved to other lines, do not count; [`jouet diag delta`](/commands/diag#only-what-an-edit-introduced) explains the matching. With `--revert-on-fail`, a new finding restores the files:

```output
$ jouet edit replace notes.txt --old zero --new 'TODO zero' --diag --revert-on-fail
reverted    notes.txt  1 edit  +1 -1
0 of 1 file written
new: notes.txt:1:1: warning: TODO left in [todo] (todo)
diag: 1 new, 1 kept, 0 fixed
```

When the files were checked at exactly their current contents before, by `jouet diag check` or an earlier `--diag`, that result is the baseline and only the check after writing runs. When hk refuses to check (`--safe` rejects a command whose effect is unknown or destructive) or nothing can check the files, nothing is written and the exit code is `4`. If the check after writing cannot run, for example because the edit created a file no checker covers, the exit code is also `4`: the edit is unverified, and `--revert-on-fail` restores the files.

With `[edit] fix_after = true` in the [config file](/setup#settings), the project's hk fixers format the written files first, so a formatting-only finding never fails the edit; see [formatting after edits](/commands/diag#formatting-after-edits).

## Guard against stale reads with `--expect-hash`

An agent that read a file a while ago may be editing a version that has since changed. `jouet edit hash` prints SHA-256 hashes to pass back later:

```output
$ jouet edit hash Cargo.toml
5824660dfdeab9bf3325439f7f04fa368f4472e3208a0738ed4a60c0bd8c0d4f  Cargo.toml
```

`--expect-hash PATH=SHA256` refuses the whole edit, writing nothing, when that file's contents no longer match. It can be given once per file:

```output
$ jouet edit replace Cargo.toml --old demo --new demo2 \
    --expect-hash Cargo.toml=5824660dfdeab9bf3325439f7f04fa368f4472e3208a0738ed4a60c0bd8c0d4f
failed      Cargo.toml  1 edit  +1 -1
  expected sha256 5824660dfdeab9bf3325439f7f04fa368f4472e3208a0738ed4a60c0bd8c0d4f, found f119381ae221d8cce35641a6025d54be21a794214f876277733fa6041f825908; the file changed since it was read
0 of 1 file written
```

## When an edit does not apply

Nothing is written, and the report says why. Files that were fine but not written because of another file are marked `not written`:

```output
failed      src/main.rs  1 edit
  edit 1: no match
    closest at line 2 (93% similar), - spec / + file:
    -    let x = 3;
    +    let x = 2;
not written Cargo.toml   1 edit  +1 -1
0 of 2 files written
```

Too many matches list where they are:

```output
failed      src/main.rs  1 edit
  edit 1: 2 matches, expected 1, at lines 2, 3
0 of 1 file written
```

## Heredocs

Pass specs through a quoted heredoc with a delimiter that cannot appear in the content, such as `<<'JOUET_EDIT'`. Quoting stops the shell expanding `$` and backticks, and a unique delimiter stops the spec being cut short.

If a spec ends inside an open block or patch anyway, `jouet edit` refuses it as a usage error rather than applying the part that survived.

## JSON output

With `--json`, the report is one document. Fields that are empty, `false` or `null` are left out:

```json
{"files_written":0,"files":[{"path":"src/main.rs","status":"failed","edits":[{"index":0,"status":"no_match","expected":1,"found":0,"closest":{"line":2,"similarity":0.93,"diff":"-    let x = 3;\n+    let x = 2;\n"}}]}]}
```

| Field | Meaning |
|---|---|
| `applied` | The files were written |
| `dry_run` | `--dry-run` was given |
| `files_written` | How many files were written |
| `files[].status` | `written`, `created`, `deleted`, `moved`, `not_written`, `failed` or `reverted` |
| `files[].moved_to` | The new path of a moved file |
| `files[].added`, `files[].removed` | Lines added and removed |
| `files[].error` | Why a file failed: `code` is `not_found`, `already_exists`, `hash_mismatch` or `not_text`, with a `message` |
| `files[].diff` | The unified diff, with `--diff` |
| `files[].edits[]` | One entry per edit: `index` (from 0), `status` (`ok`, `no_match`, `too_many_matches`, `too_few_matches`, `anchor_not_found`, `unsupported` or `does_not_parse`), `expected`, `found`, the 1-based `lines` of each match, the missing `anchor`, the `closest` region for a search that matched nothing, and, for a block or patch search that matched nothing, a `near_miss`: `{"kind": "inside_line", "lines": [13]}` when the text occurs inside a line (with `multiline` when it spans several lines), or `{"kind": "whitespace", "lines": [13]}` when it matches once indentation and trailing whitespace are ignored. A symbol edit adds its `symbol`, the `candidates` it could have meant, a `message` when refused, and a `signature` (`name`, `before`, `after`) when it changed the definition's first line |
| `callers` | For each definition whose signature changed, from the code graph: its `symbol`, its callers' `references` (`path`, `line`) in files the edit does not change, and how many `more` were left out of the list |
| `check` | The `--check` result: `command`, `exit_code`, `passed`, the last lines of `output`, `output_truncated`, `log_path` and `reverted` |
| `diag` | The `--diag` result: `status` (`clean`, `introduced`, `refused` or `unavailable`), the `new` findings, `new_failed_steps`, and the `kept` and `fixed` counts |
| `fix` | What `[edit] fix_after` did: `status`, the `formatted` files, `failed_steps` |

`jouet edit hash --json` prints `{"files":[{"path":"…","sha256":"…"}]}`.

## Exit codes

| Code | When |
|---|---|
| 0 | Every edit applied, and the check passed if there was one, and `--diag` found nothing new |
| 1 | The check failed or could not start, or `--diag` found a new finding (the files are restored with `--revert-on-fail` and kept otherwise), or reading or writing a file failed |
| 2 | A malformed spec or bad flags |
| 3 | Every failure was a missing file to update or delete |
| 4 | Nothing was written: a match count was wrong, an anchor was missing, a hash did not match, a file to create already exists, a file is not UTF-8 text, or `--diag` could not check the files before writing. Also when `--diag` could not check them after writing: the files are restored with `--revert-on-fail` and kept otherwise |

These are the [shared exit codes](/conventions#exit-codes).
