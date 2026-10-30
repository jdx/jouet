---
description: Search code like rg, with the matches grouped by the function they are in, cut to a byte budget with markers for what was left out, and matches the session already saw collapsed.
---

# jouet search

`jouet search` searches files the way `rg` does, and prints less. Reads and searches are the largest share of what agents put in their context: 41% of tool output in the Claude Code sessions jouet was measured on, and 56% in the Codex ones. Most of that is the same lines read twice, or a search that printed every match in the tree when the agent needed a few.

```text
jouet search PATTERN [PATH]... [-F] [-i] [-w] [-g GLOB]... [--hidden] [-l]
             [--budget BYTES] [--file-budget BYTES] [--again]
```

The pattern is a regular expression in ripgrep's syntax, or a literal with `-F`. Every flag is listed in the [CLI reference](/cli/search).

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

## What it adds to rg

jouet embeds ripgrep's own libraries (`ignore`, `grep-searcher` and `grep-regex`), so it walks a tree the same way: `.gitignore` and `.ignore` are respected, hidden and binary files are skipped, and files are searched in parallel. On top of that:

- **Matches are grouped by file, then by the definition they are in.** The heading names the function, method, class or module and its lines, so a match reads in context without opening the file, and reading just that definition takes its line range. Definitions come from tree-sitter for Rust, Python, JavaScript, TypeScript, Go, Ruby, Java, C and C++. Other files list their matches under the file alone.
- **Output stops at a budget, and says so.** By default jouet prints at most 6,000 bytes (about 1,500 tokens) in all, and 1,500 bytes from any one file. A file cut short ends with `... 4 more in src/x.rs`. Files the budget never reached are named on one closing line, with their match counts:

  ```output
  ... 37 matches in 9 more files: src/a.rs (12), src/b.rs (8), ... (narrow the search, or raise --budget)
  ```

  `--budget` and `--file-budget` change the limits, and `0` removes one. Nothing is ever cut without a marker, and with `--json` every match is accounted for in the counts.
- **Matches the session already saw collapse to one line.** Inside an agent session, jouet remembers which lines of which files it showed. Searching again prints only what is new; a file whose matches were all shown, and which has not changed since, becomes one line:

  ```output
  src/lib.rs: 3 matches already shown, unchanged
  ```

  A file that changed is new again. `--again` shows everything anyway.

Each file's definitions are cached by its size and modification time in `$XDG_CACHE_HOME/jouet/defs/`, since parsing a large file takes tens of milliseconds and an agent searches the same files again and again. Long lines are cut at 200 bytes, with the number of bytes left out. `-l` lists the matching files with their counts instead of the lines.

## Sessions

The session is `JOUET_SESSION_ID`, else the ID Claude Code (`CLAUDE_CODE_SESSION_ID`) or Codex (`CODEX_THREAD_ID`) sets for the commands it runs. Outside a session nothing is remembered. What a session was shown is kept in `$XDG_STATE_HOME/jouet/seen/` and is removed after a week untouched.

[`jouet setup`](/setup)'s hooks add to the same memory: the lines Claude Code's Read tool returned, and shell reads with `cat`, `head`, `tail`, `sed -n` and `nl`. The built-in `reread` rule warns when the agent reads lines it already has, from a file that has not changed.

## Rules

Two built-in [rules](/rules) steer reads and searches. Both only warn.

| Rule | Warns about |
|---|---|
| `unbudgeted-search` | `rg` or `grep -r` over the whole working directory with no `--max-count`, no `-l` or `-c`, and no `\| head`; and Claude Code's Grep tool printing matching lines over the whole tree with no `head_limit`. |
| `reread` | Reading lines this session already read, from a file unchanged since: Claude Code's Read tool, or a shell read with `cat`, `head`, `tail`, `sed -n` or `nl`. |

jouet's hooks see Claude Code's built-in Read and Grep tools as well as shell commands, so these rules steer the agent's own tools too.

## JSON

```json
{
  "files": [
    {
      "path": "src/lib.rs",
      "matches": 5,
      "shown": [
        { "line": 7, "text": "let url = join_url(&self.base, path);", "symbol": { "kind": "method", "name": "download", "start": 6, "end": 9 } }
      ],
      "more": 4
    },
    { "path": "src/main.rs", "matches": 2, "already_shown": 2 }
  ],
  "matches": 9,
  "files_matched": 3,
  "files_searched": 41,
  "omitted": [{ "path": "tests/x.rs", "matches": 2 }]
}
```

- `matches` counts every match, shown or not; `more` counts a file's matches left out by a budget, and `already_shown` those this session saw before.
- `omitted` lists the files the total budget never reached.
- As everywhere in jouet, fields that are empty or zero are left out.

## Exit codes

`0` matches were found · `1` no match · `2` a bad pattern or glob · `3` a path that does not exist

## Speed

{{SEARCH_BENCH}}

## Compared with other tools

- **rg** prints every match, in whatever order its threads finish, and it has no memory. Use it when you need all of them; `jouet search` is for the more common case of an agent looking for a few.
- **[rtk](https://github.com/rtk-ai/rtk)** compresses the output of commands an agent already ran. jouet narrows what is asked for in the first place: a search grouped by definition and cut to a budget, a [map](/commands/read) instead of whole files. The two compose, and jouet's rules see through the `rtk` prefix.
- **[Serena](https://github.com/oraios/serena)** reads and edits by symbol through language servers, behind an MCP server. Its language-server precision is real: it resolves which `download` a call means, where tree-sitter only knows names. jouet takes the same ideas through tree-sitter instead, so there is no server to run and no language server to keep alive, nothing to register per client, and no tool schemas sitting in the context on every turn. It works in any harness with a shell, composes with pipes and `jq`, and its hooks can steer the agent's other tool calls, which an MCP server cannot see.
