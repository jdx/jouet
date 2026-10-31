---
description: Read a file by its outline, one definition or a range of lines instead of whole, and a map of a repository's most important definitions, sized to a token budget.
---

# jouet read

Reading a whole file to find one thing is the most common way agents fill their context. `jouet read` reads in steps instead: a file's outline, then the one definition needed, or a range of lines. `jouet read --map` does the same for a whole repository.

```text
jouet read FILE [--outline | --symbol NAME | --range A:B] [--max-lines N] [--again]
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

- **`--outline`** lists every definition with its line range, each named with its scope: the class, module or namespace it is in, a Rust `impl` block's type, or a Go method's receiver.
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

The built-in `large-read` [rule](/rules) warns when the agent reads 400 lines or more of a file at once, with Claude Code's Read and no `offset` or `limit`, or with `cat` or `nl`, and suggests the outline.

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

### How the map is built

The map is [Aider's repo map](https://aider.chat/docs/repomap.html), rebuilt in Rust. Aider's design and its weights are used under the Apache-2.0 licence.

1. **Tags.** tree-sitter reads every source file and its grammar's tags query names what each file defines (functions, methods, classes, modules, types) and what it refers to (calls, types, implementations). Rust, Python, JavaScript, TypeScript, Go, Ruby, Java, C and C++ are supported. Tags are cached by each file's size and modification time in `$XDG_CACHE_HOME/jouet/tags/`, so a second map of the same tree only reads the files that changed.
2. **A graph.** A file that refers to a name gets an edge to each file that defines it. Following Aider, an edge counts ×10 for a name you focused on, or a long distinctive one (snake, kebab or camel case, 8 characters or more), ×0.1 for a name starting with `_` or defined in more than 5 files, ×50 when the referring file is one you focused on, and by the square root of the number of references.
3. **Ranking.** Personalised PageRank ranks the files, favouring the ones you focused on, and each file's rank is shared among the definitions its edges point at.
4. **The budget.** A binary search finds how many of the top-ranked definitions fit the budget, estimated at 4 bytes a token. Files are listed in path order, each with its definitions in line order. What did not fit is counted on the last line.

jouet departs from Aider in three places:

- **A reference to a name defined in several files is shared among them,** rather than counted in full for each. Without this, a common method name such as `is_empty` or `collect`, called all over a Rust codebase and defined in a dozen places, crowds out the repository's own types.
- **Rust gets the type references its grammar's query leaves out,** and the type and function in a path call such as `Config::load()`. Go's own query captures type references the same way.
- **The definitions of a focused identifier come first,** since you named it.

Names are matched by text, as in Aider: tree-sitter does not know which `download` a call means. A language server does, which is where [Serena](https://github.com/oraios/serena) is more precise; the map needs no server.

### Focus

`--focus` takes a file you are working on, or an identifier you care about, and can be repeated. A value naming an existing file is a file; anything else is an identifier.

- What a focused file refers to ranks higher, and the file's own definitions are left out, since you already have it open.
- A focused identifier's definitions come first, references to it count more, and files with it as a path component (`src/download.rs` for `download`) rank higher.

The budget is 1,024 tokens with `--focus` and 8,192 without, when the map is the agent's whole view of the repository. These are Aider's defaults.

### Speed

On mise's repository (1,061 Rust and other source files) on a 32-core Linux machine, a map with nothing cached took 0.39 s, and with every file's tags cached 0.11 s, the mean of 10 and 20 runs. The cache for that repository is 5.4 MB.

Built without the `grammars` feature, jouet has no tree-sitter and `jouet read --map` exits 2 and says so. The release builds have it.

### JSON

`jouet read --map --budget 40 --json`, with nothing cached yet (`cached` counts the files whose tags came from the cache):

```json
{
  "files": [
    {
      "path": "src/lib.rs",
      "defs": [
        {
          "line": 1,
          "kind": "class",
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
  "files_scanned": 2,
  "more_defs": 2,
  "more_files": 1
}
```
