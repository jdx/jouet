---
description: A code graph of symbols, calls, imports and inheritance, built with tree-sitter and kept fresh by jouet's hooks, with no daemon or MCP server.
---

# jouet graph

`jouet graph` keeps a code graph of a repository: its functions, classes, methods, fields and imports, with the files they live in. Later commands answer who calls a function, what a change could break and which tests a change reaches, without the agent grepping its way there.

```text
jouet graph index [--full]
jouet graph status
```

The flags are listed in the [CLI reference](/cli/graph/).

## Languages

Rust, Python, TypeScript and TSX, JavaScript and JSX, Go, Ruby, Java, C and C++. A `.h` file that looks like C++ is read as C++, and a JavaScript file with a `// @flow` header is read as TSX.

## Where the graph lives

The graph is a SQLite database in `$XDG_CACHE_HOME/jouet/graph/` (`~/.cache/jouet/graph/` by default), one per work tree, named by the same key as jouet's [ledger](/winding): a hash of the work tree's root. Nothing is written into the repository, so there is nothing to ignore or commit, and deleting the file only costs a re-index. `jouet graph status` prints its path and size.

## Keeping it fresh without a daemon

There is no file watcher and no background process:

- **`jouet graph index`** lists the work tree's source files, honouring `.gitignore`, and extracts only the files whose content changed since the last index. A file whose timestamp changed but whose content did not is only re-stamped. `--full` drops the database and starts again.
- **Every query** runs the same check first, so a graph that was built once stays current.
- **jouet's post-tool-use hook** re-indexes the files an agent just edited, through Claude Code's file tools or a Codex patch, and `jouet edit` re-indexes what it wrote. Both spend at most a quarter of a second; anything left over is picked up by the next query. The hook only updates a graph that exists, and never builds one. Turn it off with `hooks.graph = false` or `JOUET_HOOKS_GRAPH=0`.

Files larger than 1 MiB, or not UTF-8, are recorded without symbols. Directories such as `node_modules`, `vendor` and `target` are skipped even when nothing ignores them. A file with syntax errors is still indexed, as tree-sitter recovered it, and `status` counts such files.

## Credit

The code graph builds on [codegraph](https://github.com/colbymchenry/codegraph)'s design and extraction. Its tree-sitter walkers are ported from codegraph's Rust extraction kernel, and its storage follows codegraph's schema. codegraph is MIT-licensed; its notice is in jouet's `NOTICE` file.
