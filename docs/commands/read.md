---
description: A map of a repository's most important definitions, sized to a token budget, so an agent can find its way around without reading whole files.
---

# jouet read

`jouet read --map` prints a map of a repository: for the files that matter most, the line of each of their most important definitions, sized to fit a token budget. An agent starting on unfamiliar code reads the map instead of listing directories and opening whole files to see what is in them.

```text
jouet read --map [PATH] [--budget TOKENS] [--focus FILE|NAME]...
```

```output
$ jouet read --map --focus src/main.rs --budget 200
src/lib.rs
  1: pub struct Client {
  6:     pub fn download(&self, path: &str) -> String {
  12: fn join_url(base: &str, path: &str) -> String {
... 4 more definitions in 2 more files (raise --budget, or --focus a file or name)
```

Every flag is listed in the [CLI reference](/cli/read).

## How the map is built

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

## Focus

`--focus` takes a file you are working on, or an identifier you care about, and can be repeated. A value naming an existing file is a file; anything else is an identifier.

- What a focused file refers to ranks higher, and the file's own definitions are left out, since you already have it open.
- A focused identifier's definitions come first, references to it count more, and files with it as a path component (`src/download.rs` for `download`) rank higher.

The budget is 1,024 tokens with `--focus` and 8,192 without, when the map is the agent's whole view of the repository. These are Aider's defaults.

## Speed

{{MAP_BENCH}}

Built without the `grammars` feature, jouet has no tree-sitter and `jouet read --map` exits 2 and says so. The release builds have it.

## JSON

```json
{
  "files": [
    { "path": "src/lib.rs", "defs": [{ "line": 1, "kind": "class", "name": "Client", "text": "pub struct Client {" }] }
  ],
  "tokens": 61,
  "files_scanned": 3,
  "cached": 3,
  "more_defs": 4,
  "more_files": 2
}
```
