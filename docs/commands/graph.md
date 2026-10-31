---
description: A code graph of symbols, calls, imports and inheritance, built with tree-sitter and kept fresh by jouet's hooks, with no daemon or MCP server.
---

# jouet graph

`jouet graph` keeps a code graph of a repository: its functions, classes, methods, fields and imports, and the calls, imports and inheritance between them. It answers who calls a function, what a change could break, which tests a change reaches and where the code for a question lives, without the agent grepping its way there.

```text
jouet graph explore QUESTION... [--budget BYTES] [--source-budget BYTES] [--page N]
jouet graph symbols [QUERY] [--kind KIND] [--path PATH] [--budget BYTES] [--page N]
jouet graph callers|callees|impact SYMBOL [--depth N] [--budget BYTES] [--page N]
jouet graph affected FILE...
jouet graph index [--full]
jouet graph status
```

The flags are listed in the [CLI reference](/cli/graph/).

## Queries

```console
$ jouet graph callers ToolVersion::install_path
callers of method ToolVersion::install_path (src/toolset/tool_version.rs:544): 4
  src/toolset/tool_version.rs:430 method ToolVersion::strip_install_path_identity (calls)
  src/toolset/tool_version.rs:524 method ToolVersion::legacy_aube_install_path_version (calls)
  src/toolset/tool_version.rs:601 method ToolVersion::runtime_path (calls)
  src/toolset/tool_version.rs:736 method ToolVersion::state_key (calls)
```

- **`explore QUESTION`** ranks symbols by how well their names (split into words, so `processPayment` matches `payment`), qualified names, docstrings and signatures match the question, and prints each with its docstring, callers, callees and the start of its source. Tests come after the code they test unless the question is about tests.
- **`symbols QUERY`** finds symbols: an exact name first, then a qualified name (`Client::push` or `Client.push`), then words. Each line ends with the symbol's `#ID`.
- **`callers`, `callees` and `impact SYMBOL`** take a name, a qualified name, `PATH#NAME`, `PATH:LINE` (the innermost symbol there) or an `#ID`. `impact` follows every edge but containment, from the symbol and everything it contains, up to three hops, and is grouped by file.
- **`affected FILE...`** lists the test files that depend on the changed files, through imports, calls, references and inheritance, with the chain that reaches each. A file the graph cannot speak for is listed as unknown, and the exit code is 1: run the whole suite. `jouet test --affected` uses a language's own tool where there is one; this is the fallback.

Every answer is budgeted, 6,000 bytes by default, and paged rather than cut: a long answer ends with a line such as `... 43 more impact on 5 more pages (jouet graph impact struct:7af2… --page 2)`. A dense answer stays in the agent's context for the rest of the session, so it pays to ask for the next page only when it is needed. `--json` prints the same page as one document, with `total`, `page` and `pages`.

## Linking

A call, import or base class is linked to its definition in this order: a name the file imports, a member of the type a `self`/`this` or a type qualifier names, the same file, the files the file imports, the same directory (a Go or Java package), and finally the only definition of that name in the project. Imports are resolved the way each language does: JS/TS specifiers with extensions and `index` files, Python modules from any package root, Go import paths through `go.mod`, Rust `use` paths through the crate's module files, Java imports by package directory, Ruby `require` and C `#include` relative to the file, then by path.

Where the receiver's type is unknown (`x.get()`), a common method name is never linked by name alone, and a name the file imports from a library (`Path`, `os.path`) is never linked to a project symbol that shares it. A wrong edge is worse than a missing one.

Not done yet: framework routing (Express, Rails, Spring and the like), receiver type inference, tsconfig path aliases, C include directories from build files, and callbacks passed through function pointers.

### Coverage

codegraph measures its graph by the share of source files that at least one other file depends on (imports, calls, references or inherits from). The same measure on the same repositories, counting non-test source files (tests, examples, benchmarks, docs and build scripts left out), measured on 2026-10-31:

| Language | Repository | codegraph | jouet |
|---|---|---|---|
| TypeScript / JavaScript | colbymchenry/codegraph | 95.8% | 96.3% |
| Python | psf/requests | 100% | 100% |
| Go | gin-gonic/gin | 96.6% | 100% |
| Rust | BurntSushi/ripgrep | 86.7% | 95.7% |
| Java | google/gson | 93.3% | 89.6% |
| Ruby | sidekiq/sidekiq | 100% | 85.1% |
| C | redis/redis | 92.2% | 92.7% |
| C++ | google/leveldb | 94.8% | 93.0% |

codegraph's exact denominator isn't published, so the columns compare closely but not exactly. Ruby's gap is sidekiq's Rails example app, which only framework routing reaches. Java's gap is gson's reflection-loaded adapters and benchmarks.

## Languages

Rust, Python, TypeScript and TSX, JavaScript and JSX, Go, Ruby, Java, C and C++. A `.h` file that looks like C++ is read as C++, and a JavaScript file with a `// @flow` header is read as TSX.

## Where the graph lives

The graph is a SQLite database in `$XDG_CACHE_HOME/jouet/graph/` (`~/.cache/jouet/graph/` by default), one per work tree, named by the same key as jouet's [ledger](/winding): a hash of the work tree's root. Nothing is written into the repository, so there is nothing to ignore or commit, and deleting the file only costs a re-index. `jouet graph status` prints its path and size.

## Keeping it fresh without a daemon

There is no file watcher and no background process:

- **`jouet graph index`** lists the work tree's source files, honouring `.gitignore`, and extracts only the files whose content changed since the last index. A file whose timestamp changed but whose content did not is only re-stamped. `--full` drops the database and starts again.
- **Every query** runs the same check first, and builds the graph if there is none, so a graph that was built once stays current.
- **jouet's post-tool-use hook** re-indexes the files an agent just edited, through Claude Code's file tools or a Codex patch, and `jouet edit` re-indexes what it wrote. Both spend at most a quarter of a second; anything left over is picked up by the next query. The hook only updates a graph that exists, and never builds one. Turn it off with `hooks.graph = false` or `JOUET_HOOKS_GRAPH=0`.

Files larger than 1 MiB, or not UTF-8, are recorded without symbols. Directories such as `node_modules`, `vendor` and `target` are skipped even when nothing ignores them. A file with syntax errors is still indexed, as tree-sitter recovered it, and `status` counts such files.

## Credit

The code graph builds on [codegraph](https://github.com/colbymchenry/codegraph)'s design and extraction. Its tree-sitter walkers are ported from codegraph's Rust extraction kernel, and its storage follows codegraph's schema. codegraph is MIT-licensed; its notice is in jouet's `NOTICE` file.
