---
description: Search code like rg, with the matches grouped by the function they are in, cut to a byte budget with markers for what was left out, and matches the session already saw collapsed.
---

# jouet search

`jouet search` searches files the way `rg` does, and prints less. It groups the matches by the function or class they are in, and stops at a byte budget with a marker for what it left out:

```output
$ jouet search 'join_url|fetch\(' src/
src/lib.rs
  method download (6-9)
    7: let url = join_url(&self.base, path);
    8: fetch(&url)
  function join_url (12-14)
    12: fn join_url(base: &str, path: &str) -> String {
  function fetch (16-18)
    16: fn fetch(url: &str) -> String {
```

When the matches don't fit the budget (6,000 bytes by default), the output says how many were left out and where. A search for `url` in the same files, with both budgets cut to 100 bytes:

```output
$ jouet search --budget 100 --file-budget 100 -F url
src/lib.rs
  method download (6-9)
    7: let url = join_url(&self.base, path);
  ... 4 more in src/lib.rs
... 1 match in 1 more file: src/notes.rs (1) (narrow the search, or raise --budget)
```

Inside an agent session, a second search prints only what is new. Running the first search again gives:

```output
$ jouet search 'join_url|fetch\(' src/
src/lib.rs: 4 matches already shown, unchanged
```

Reads and searches are the largest share of what agents put in their context: 41% of tool output in the Claude Code sessions jouet was measured on, and 56% in the Codex ones. Most of that is the same lines read twice, or a search that printed every match in the tree when the agent needed a few.

```text
jouet search PATTERN [PATH]... [-F] [-i] [-w] [-g GLOB]... [--hidden] [-l]
             [-m NUM] [--budget BYTES] [--file-budget BYTES] [--again]
             [--structural --lang LANG [--callers-of PATH#SYMBOL | --impact PATH#SYMBOL]]
```

The pattern is a regular expression in ripgrep's syntax, or a literal with `-F`. `-m NUM` (`--max-count`) shows at most NUM matches from each file, as rg's does, counting the rest as left out, and `-n` is accepted for rg's sake, since line numbers are always shown: agents reach for both by habit. Every flag is listed in the [CLI reference](/cli/search).

## Searching like rg

jouet embeds ripgrep's own libraries (`ignore`, `grep-searcher` and `grep-regex`), so it walks a tree the same way: `.gitignore` and `.ignore` are respected, hidden and binary files are skipped, and files are searched in parallel. `-g GLOB` includes or excludes files as rg's `-g` does, and `--hidden` searches hidden files too.

Long lines are cut at 200 bytes, with the number of bytes left out. `-l` lists the matching files with their counts instead of the lines.

## Grouped by definition

Matches are grouped by file, then by the definition they are in. The heading names the function, method, class or module and its lines, so a match reads in context without opening the file, and [`jouet read FILE --symbol NAME`](/commands/read) or `--range 6:9` prints only that definition. Definitions come from tree-sitter for Rust, Python, JavaScript, TypeScript, Go, Ruby, Java, C and C++. Other files, and matches outside any definition, are listed under the file alone.

Each file's definitions are cached by its size and modification time in `$XDG_CACHE_HOME/jouet/defs/` (`~/.cache/jouet/defs/` without it), since parsing a large file takes tens of milliseconds and an agent searches the same files again and again. Cached definitions untouched for a month are removed.

## Budgets

By default jouet prints at most 6,000 bytes (about 1,500 tokens) in all, and 1,500 bytes from any one file. A file cut short ends with `... 4 more in src/x.rs`. Files the budget never reached are named on one closing line with their match counts, the first 10 by name and the rest as a count:

```output
$ jouet search TODO --budget 120
src/a.rs
  function a (1-3)
    2: todo!(); // TODO
src/b.rs
  function b (1-3)
    2: todo!(); // TODO
... 13 matches in 11 more files: src/c.rs (3), src/d.rs (1), src/e.rs (1), src/f.rs (1), src/g.rs (1), src/h.rs (1), src/i.rs (1), src/j.rs (1), src/k.rs (1), src/l.rs (1) and 1 other (narrow the search, or raise --budget)
```

`--budget` and `--file-budget` change the limits, and `0` removes one. Nothing is ever cut without a marker, and with `--json` every match is accounted for in the counts.

## Matches the session already saw {#sessions}

Inside an agent session, jouet remembers which lines of which files it showed. Searching again prints only what is new; a file whose matches were all shown, and which has not changed since, becomes one line, such as `src/lib.rs: 4 matches already shown, unchanged`. A file whose size or modification time changed is new again. `--again` shows everything anyway. Neither `--again` nor `-l` adds what it prints to the session's memory.

The session is `JOUET_SESSION_ID`, else the ID Claude Code (`CLAUDE_CODE_SESSION_ID`) or Codex (`CODEX_THREAD_ID`) sets for the commands it runs. Outside a session nothing is remembered. What a session was shown is kept in `$XDG_STATE_HOME/jouet/seen/` (`~/.local/state/jouet/seen/` without it) and is removed after a week untouched.

[`jouet setup`](/setup)'s hooks add to the same memory: the lines Claude Code's Read tool returned, and shell reads with `cat`, `head`, `tail`, `sed -n` and `nl`. The built-in `reread` rule warns when the agent reads lines it already has, from a file that has not changed.

## Structural search

`--structural --lang LANG` matches the pattern as code, with [ast-grep](https://ast-grep.github.io)'s matcher, instead of as a regular expression. `$X` matches one node and `$$$X` any number, and only files in that language are searched. The results are grouped, budgeted and remembered like any search:

```output
$ jouet search --structural --lang rust '$X.unwrap()'
src/lib.rs
  function load (1-6)
    2: std::fs::read_to_string(path).unwrap()
```

Quote the pattern in single quotes, or the shell expands `$X`. `-F`, `-i` and `-w` are for text patterns and are refused with `--structural`. [`jouet edit --pattern`](/commands/edit#structural-rewrites) rewrites what a pattern matches, and [lists the rules for writing patterns](/commands/edit#writing-patterns).

`--callers-of PATH#SYMBOL` keeps only the matches at calls the [code graph](/commands/graph) attributes to that symbol, so a pattern like `load($$$A)` leaves other functions named `load` out. `--impact PATH#SYMBOL` keeps the matches inside the symbol and the code that depends on it. Both need `--structural`. After the matches comes what the graph made of the rest: the matches it cannot attribute, and the symbol's calls the pattern did not match, as [scoped rewrites](/commands/edit#scoped-by-the-code-graph) list them.

## Rules

Two built-in [rules](/rules) steer reads and searches. Both only warn.

| Rule | Warns about |
|---|---|
| `unbudgeted-search` | `rg` or `grep -r` over the whole working directory with no `--max-count`, no `-l` or `-c`, and not piped to `head`, `tail`, `wc`, `less` or `more`; and Claude Code's Grep tool printing matching lines over the whole tree with no `head_limit`, `path`, `glob` or `type`. It stays quiet in a work tree of fewer than 200 tracked files, where a search of all of it stays small. |
| `reread` | Reading lines this session already read, from a file unchanged since: Claude Code's Read tool, or a shell read with `cat`, `head`, `tail`, `sed -n` or `nl`. |

jouet's hooks see Claude Code's built-in Read and Grep tools as well as shell commands, so these rules steer the agent's own tools too.

## JSON

`jouet search --json --budget 100 --file-budget 100 -F url`, the search above:

```json
{
  "files": [
    {
      "path": "src/lib.rs",
      "matches": 5,
      "shown": [
        {
          "line": 7,
          "text": "let url = join_url(&self.base, path);",
          "symbol": {
            "kind": "method",
            "name": "download",
            "start": 6,
            "end": 9
          }
        }
      ],
      "more": 4
    }
  ],
  "matches": 6,
  "files_matched": 2,
  "files_searched": 3,
  "omitted": [
    {
      "path": "src/notes.rs",
      "matches": 1
    }
  ]
}
```

- `matches` counts every match, shown or not; `more` counts a file's matches left out by a budget, and `already_shown` (absent here) those this session saw before.
- `omitted` lists the files the total budget never reached.
- With `--callers-of` or `--impact`, a `scope` object says what the graph made of the matches, as for [`jouet edit`](/commands/edit#scoped-by-the-code-graph).
- As everywhere in jouet, fields that are empty or zero are left out.

## Exit codes

`0` matches were found · `1` no match · `2` a bad pattern or glob · `3` a path that does not exist

## Speed

Search itself is ripgrep's, and runs as fast. On mise's repository (5,048 files searched), with ripgrep 15.2.0 on a 32-core Linux machine, output piped, the mean of 30 [hyperfine](https://github.com/sharkdp/hyperfine) runs:

| Search | `rg -n` | `jouet search` |
|---|---|---|
| `-F 'fn install_version'`: 40 matches in 35 files | 11.0 ms | 10.8 ms |
| `Settings`: 2,299 matches in 346 files | 10.8 ms | 11.3 ms, printing 6 KB of rg's 198 KB |
| `Settings`, `--budget 0 --file-budget 0` | | 12.4 ms |

Those runs read definitions from the cache. The first search to show those 35 files parses them all, and took 188 ms.

## Compared with other tools

- **rg** prints every match, in whatever order its threads finish, and it has no memory. Use it when you need all of them; `jouet search` is for the more common case of an agent looking for a few.
- **[rtk](https://github.com/rtk-ai/rtk)** compresses the output of commands an agent already ran. jouet narrows what is asked for in the first place: a search grouped by definition, cut to a budget, and without what the session already saw. The two compose, and jouet's rules see through the `rtk` prefix.
- **[Serena](https://github.com/oraios/serena)** reads and edits by symbol through language servers, behind an MCP server. Its language-server precision is real: it resolves which `download` a call means, where tree-sitter only knows names. jouet takes the same ideas through tree-sitter instead, so there is no server to run and no language server to keep alive, nothing to register per client, and no tool schemas sitting in the context on every turn. It works in any harness with a shell, composes with pipes and `jq`, and its hooks can steer the agent's other tool calls, which an MCP server cannot see.
