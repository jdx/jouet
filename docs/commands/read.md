---
description: Read a file by its outline, one definition or a range of lines, instead of whole.
---

# jouet read

Reading a whole file to find one thing is the most common way agents fill their context. `jouet read` reads in steps instead: a file's outline, then the one definition needed, or a range of lines.

```text
jouet read FILE [--outline | --symbol NAME | --range A:B] [--max-lines N] [--again]
```

Every flag is listed in the [CLI reference](/cli/read).

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
