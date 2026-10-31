---
description: Read a file by its outline, one definition or a range of lines instead of whole, and a map of a repository's most important definitions, sized to a token budget.
---

# jouet read

Reading a whole file to find one thing is the most common way agents fill their context. `jouet read` reads in steps instead: a file's outline, then the one definition needed, or a range of lines. `jouet read --map` does the same for a whole repository.

```text
jouet read FILE [--outline | --symbol NAME | --range A:B] [--max-lines N] [--again] [--why]
jouet read --map [PATH] [--budget TOKENS] [--focus FILE|NAME]...
```

Every flag is listed in the [CLI reference](/cli/read).

## Reading a file

```output
$ jouet read src/lib.rs --outline
src/lib.rs: 18 lines, 4 definitions
  1-3    class Client
  6-9    method Client::download
  12-14  function join_url
  16-18  function fetch
$ jouet read src/lib.rs --symbol Client::download
src/lib.rs: method Client::download (6-9)
6	    pub fn download(&self, path: &str) -> String {
7	        let url = join_url(&self.base, path);
8	        fetch(&url)
9	    }
```

- **`--outline`** lists every definition with its line range, each named with its scope: the class, module or namespace it is in, a Rust `impl` block's type, or a Go method's receiver. A file of up to 100 lines is printed whole instead, under a line saying so, because its outline saves little and reading it after the outline costs another call; `--json` then has both the outline and the file, and `short: true`. A short file this session was already shown gets only its outline.
- **`--symbol NAME`** prints the definitions `NAME` matches. `NAME` is a name (`download`), a kind and a name (`fn download`, `class Cart`, `trait Backend`), or a name with its scope (`Client::download`, `Shop.Cart`). The same selectors pick definitions for [`jouet edit`'s symbol targets](/commands/edit#symbol-targets).
- **`--range A:B`** prints lines `A` to `B`; `A:` runs to the end and `:B` starts at the beginning.
- **With none of them,** a file of up to 300 lines is printed whole, and a longer one as its outline, with a line saying so.

Code is printed with line numbers, at most `--max-lines` lines (400 by default; `0` is no limit), and an outline at most that many definitions. A cut ends with a marker, such as `... 16 more lines (--range 3:18, or raise --max-lines)`.

Inside an agent session, lines this session was already shown, from a file unchanged since, collapse to one line, as in [`jouet search`](/commands/search#sessions); `--again` prints them. The hooks remember what Claude Code's Read tool showed too.

Definitions come from tree-sitter, for Rust, Python, JavaScript, TypeScript, Go, Ruby, Java, C and C++. An outline or symbol of another kind of file exits 2; `--range` works on any text file. A missing file or symbol exits 3, and a missing symbol's error lists what the file defines.

With `--json`, `jouet read src/lib.rs --symbol fetch --json` prints:

```json
{
  "path": "src/lib.rs",
  "lines": 18,
  "sections": [
    {
      "symbol": "function fetch (16-18)",
      "start": 16,
      "end": 18,
      "text": "fn fetch(url: &str) -> String {\n    url.to_string()\n}"
    }
  ]
}
```

An outline is an `outline` list of `kind`, `name`, `start` and `end`, with `more_definitions` when it was cut. A section has `more` for lines a cut left out, and `already_shown` instead of `text` for lines the session already has.

## Where lines came from, with `--why`

In a repository with [Entire](https://entire.io), `--why` with `--symbol` or `--range` adds a line saying where the lines came from, by `entire why FILE --json`: which agent session and checkpoint wrote them, with the commit and the start of the prompt, and how many lines people wrote and in how many commits.

```text
$ jouet read test/proxy_mcp.bats --range 145:152 --why
145		# closes as soon as the child is reaped (seen failing on macOS)
...
152			sleep 0.1
origin (entire why): 7 lines (145-148, 150-152) by Claude Code session 8d2ac41d, checkpoint 01M66E9JMCAY (commit b7140d9a): "why did this fail https://github.com/jdx/fnox/pull/1087#issuecomment-6520951953"; 1 line (149) by Claude Code session 8d2ac41d, checkpoint 01M66H17P11G (commit 12fa1abf): "why did this fail https://github.com/jdx/fnox/pull/1087#issuecomment-6520951953"
```

It is off by default because `entire why` is slow where the history is long. Measured with entire 0.11.4 on one machine, a file took 44 to 300 ms in repositories with a few hundred commits, and 22 to 124 seconds in one with about 11,000. `[entire] why_timeout_ms` (15 seconds by default) bounds the wait. A timeout, a file Entire cannot place, or a repository without Entire is a `note:` line, and the read still prints. With `--json`, a section has `origin`: `agent` (one entry per checkpoint: `checkpoint_id`, `session_id`, `agent`, `commit`, `lines` and `prompt`), `human_lines`, `human_commits` and `uncommitted_lines`. Prompts are cut to their first line and 100 characters, with credentials redacted.

The built-in `large-read` [rule](/rules) warns when the agent reads 400 lines or more of source code at once, with Claude Code's Read and no `offset` or `limit`, or with `cat` or `nl`, and suggests the outline. Only files the outline can read count: a log, a command's saved output or a document read whole does not.

## The repository map

`jouet read --map` prints a map of a repository: for the files that matter most, the line of each of their most important definitions, sized to fit a token budget. An agent starting on unfamiliar code reads the map instead of listing directories and opening whole files to see what is in them.

```output
$ jouet read --map --budget 40
src/lib.rs
  1: pub struct Client {
  12: fn join_url(base: &str, path: &str) -> String {
  16: fn fetch(url: &str) -> String {
... 2 more definitions in 1 more file (raise --budget, or --focus a file or name)
```

With a `PATH`, only files under that directory are listed, though they are still ranked by the whole repository. Paths are printed from the current directory.

### How the map is built

The map is [Aider's repo map](https://aider.chat/docs/repomap.html) on jouet's [code graph](/commands/graph). Aider's design and its weights are used under the Apache-2.0 licence.

1. **Uses.** The graph has already linked every call, construction, type, import and inheritance to the definition it means. The map counts, in one query, how often each file uses each definition. A use of a field or an enum variant counts for the type that holds it.
2. **A graph of files.** A file that uses a definition gets an edge to the file defining it. Following Aider, an edge counts ×10 for a name you focused on, or a long distinctive one (snake, kebab or camel case, 8 characters or more), ×0.1 for a name starting with `_` or defined in more than 5 files, ×50 when the using file is one you focused on, and by the square root of the number of uses. A definition nothing uses gets a self-edge of 0.1.
3. **Ranking.** Personalised PageRank ranks the files, favouring the ones you focused on, and each file's rank is shared among the definitions its edges point at.
4. **The budget.** A binary search finds how many of the top-ranked definitions fit the budget, estimated at 4 bytes a token. Files are listed in path order, each with its definitions in line order. The last line counts what did not fit.

The map lists functions, methods, types (structs, classes, enums, interfaces, traits, type aliases), modules, macros and exported variables. It leaves out locals, fields and enum variants.

jouet departs from Aider in three places:

- **Uses are resolved, not matched by name.** Aider counts a call to `load` as evidence for every file that defines a `load`. The graph knows which `load` it is, so a common method name such as `is_empty` or `try_from`, defined in dozens of places, no longer crowds out a repository's own types.
- **A file's rank is shared over at least one unit of weight.** A file that uses nothing would otherwise pass all of its rank to its unused definitions' 0.1 self-edges, and a helper nothing calls would outrank what everything uses.
- **The definitions of a focused identifier come first,** since you named it.

### Focus

`--focus` takes a file you are working on, or an identifier you care about, and can be repeated. A value naming an existing file is a file; anything else is an identifier.

- What a focused file uses ranks higher, and the file's own definitions are left out, since you already have it open.
- A focused identifier's definitions come first, uses of it count more, and files with it as a path component (`src/download.rs` for `download`) rank higher.

The budget is 1,024 tokens with `--focus` and 8,192 without, when the map is the agent's whole view of the repository. These are Aider's defaults.

### Speed

The map reads the code graph, so the first map of a repository builds the graph (and a new worktree starts from a sibling's graph, when there is one). After that, only files whose size or modification time changed are read again, as before any graph query. A map takes about 155 ms on mise's repository (1,061 source files) and 35 ms on jouet's, on a 32-core Linux machine. Building mise's graph from nothing took 2.3 s.

### JSON

`jouet read --map --budget 40 --json` (`files_ranked` counts the files the graph has indexed):

```json
{
  "files": [
    {
      "path": "src/lib.rs",
      "defs": [
        {
          "line": 1,
          "kind": "struct",
          "name": "Client",
          "text": "pub struct Client {"
        },
        {
          "line": 12,
          "kind": "function",
          "name": "join_url",
          "text": "fn join_url(base: &str, path: &str) -> String {"
        },
        {
          "line": 16,
          "kind": "function",
          "name": "fetch",
          "text": "fn fetch(url: &str) -> String {"
        }
      ]
    }
  ],
  "tokens": 32,
  "files_ranked": 2,
  "more_defs": 2,
  "more_files": 1
}
```

`jouet read --map` exits 4 outside a git work tree, since the graph is kept per work tree, and 3 for a `PATH` that is not a directory.
