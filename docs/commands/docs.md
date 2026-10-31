---
description: Look up a dependency's API, its signatures, doc comments and source, at the exact version the project's lockfile names, from the sources already installed.
---

# jouet docs

An agent writing code against a dependency usually recalls its API from training, which may describe another version, or opens files in the cargo registry or `node_modules` one by one to check. `jouet docs` answers from the sources the package manager already installed for the project, at the version its lockfile names, and says which version and file each answer came from.

```text
jouet docs PKG [SYMBOL] [--lang rust|ts|python|go] [--budget TOKENS] [--private]
```

Every flag is listed in the [CLI reference](/cli/docs).

## A package's outline

Without a symbol, it prints the package's public API, root first: each definition's path, kind, file and line, the first sentence of its doc, and its signature.

```output
$ jouet docs ureq
ureq 3.5.0 (rust, Cargo.lock) at ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.5.0
A simple, safe HTTP client.

struct ureq::Agent (src/agent.rs:90, 18 members): Agents keep state between requests.
  pub struct Agent
trait ureq::AsSendBody (src/send_body.rs:257, 1 member): Trait for common types to send in POST, PUT or PATCH.
  pub trait AsSendBody: Private
...
function ureq::get (src/lib.rs:657): Make a GET request.
  pub fn get<T>(uri: T) -> RequestBuilder<WithoutBody> where Uri: TryFrom<T>, <Uri as TryFrom<T>>::Error: Into<http::Error>
...
... 24 more definitions (raise --budget, or `jouet docs ureq MODULE` for one module)
```

The outline is cut to `--budget` tokens (2048 by default, estimated at 4 bytes each), with a marker saying how many definitions were left out. A module's path as the symbol (`jouet docs reqwest blocking`) prints that module's outline.

## One definition

With a symbol, it prints each definition the symbol names: where it is, its signature, its doc comment, the start of its body, and for a type its methods, fields or variants.

```output
$ jouet docs ureq Agent::get
ureq 3.5.0 (rust, Cargo.lock) at ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.5.0

method ureq::Agent::get  ~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.5.0/src/agent.rs:326-332
pub fn get<T>(&self, uri: T) -> RequestBuilder<WithoutBody>
where
    Uri: TryFrom<T>,
    <Uri as TryFrom<T>>::Error: Into<http::Error>

  Make a GET request using this agent.

330	    {
331	        RequestBuilder::<WithoutBody>::new(self.clone(), Method::GET, uri)
332	    }
```

The symbol is a path, with `::` or `.` between the parts: `Agent`, `Agent::get`, `ureq::Agent::get`, `click.Context.invoke`, `Job.Run`. A path from the package's root matches first, with or without the package's name, so `jouet docs tokio spawn` is `tokio::spawn`, not `tokio::process::Command::spawn`, and `reqwest Client::get` leaves out `reqwest::blocking::Client::get` until `blocking::` is given. Only when no path matches from the root do paths that end with the symbol match. Matches are shown shortest path first: the first three in full, the rest listed by path. A symbol that matches nothing exits 3 with similar names; one that matches only a private definition says `--private` shows it.

## Where the version and the sources come from

The nearest lockfile from the current directory up to the repository's root decides the version. Nothing is downloaded, and no package manager is run: a package that is locked but not installed, or installed at another version than the lockfile's, exits 4 with the command that installs it.

| Ecosystem | Version from | Sources read |
|---|---|---|
| Rust | `Cargo.lock`; the workspace's own dependency when several versions are locked | `$CARGO_HOME/registry/src/*/NAME-VERSION`, a `vendor/` directory, or a git checkout in `$CARGO_HOME/git/checkouts` |
| TypeScript and JavaScript | `package-lock.json`, `pnpm-lock.yaml`, `yarn.lock` or `bun.lock`, checked against `node_modules/PKG/package.json` | The package's `.d.ts` files, the entry its `package.json` names first; else `node_modules/@types/PKG`; else its JavaScript |
| Python | `uv.lock` or `poetry.lock`, checked against the installed `.dist-info`; without a lockfile, the `.dist-info` alone | The virtualenv's site-packages (`$VIRTUAL_ENV`, `.venv` or `venv`), a `.pyi` stub before its `.py` |
| Go | `go.mod`'s `require` and `replace` | `$GOMODCACHE` (or `$GOPATH/pkg/mod`), or `vendor/` |

`PKG@VERSION` picks one of several versions a lockfile locks. `--lang` picks the ecosystem when the package is in more than one lockfile in the same directory.

## What public means

- **Rust:** items declared plain `pub` (not `pub(crate)`), without `#[doc(hidden)]` or `#[cfg(test)]`, that a path of `pub mod`s and `pub use`s reaches from the crate root. Re-exports are followed through any number of the crate's own modules, so `reqwest::Client` resolves to `src/async_impl/client.rs`, but never into another crate: those are listed as `reexport`. Modules and re-exports are read from the source text, so ones declared inside macros (`cfg_if!`) count, and every `#[cfg]` is taken as enabled. A type's members are its inherent `pub` methods, a trait's methods and an enum's variants.
- **TypeScript:** exported declarations, and the members of exported classes and interfaces that are not `private` or `protected`.
- **Python:** modules and names without a leading `_`, or the names in a module's `__all__` when it has one; a package's `from .x import Y` re-exports `Y`. A class's members are its public methods and `__init__`.
- **Go:** capitalised names in the package's directory, without `_test.go` files.

`--private` includes everything else, each under the path where it is defined.

Doc comments are read from the source: Rust's `///` above any attributes (lines rustdoc hides from an example are left out), JSDoc's `/** */`, Python docstrings and Go's `//` comments.

## Caching

Each package version is parsed once, with the tree-sitter grammars `jouet read` and the code graph use, and its index is cached under jouet's cache directory (`$XDG_CACHE_HOME/jouet/docs`, else `~/.cache/jouet/docs`), keyed by name, version and where it is installed. A second lookup reads one file.

## JSON

`jouet docs ureq Agent::new_with_defaults --json` prints:

```json
{
  "package": "ureq",
  "version": "3.5.0",
  "language": "rust",
  "lockfile": "Cargo.lock",
  "source": "/home/coder/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.5.0",
  "symbol": "Agent::new_with_defaults",
  "matches": [
    {
      "kind": "method",
      "path": "ureq::Agent::new_with_defaults",
      "file": "/home/coder/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/ureq-3.5.0/src/agent.rs",
      "line": 103,
      "end_line": 109,
      "signature": "pub fn new_with_defaults() -> Self",
      "doc": "Creates an agent with defaults.",
      "excerpt": "104\t        Self::with_parts_inner(\n105\t            Config::default(),\n106\t            Box::new(DefaultConnector::default()),\n107\t            DefaultResolver::default(),\n108\t        )\n109\t    }\n",
      "excerpt_line": 104
    }
  ]
}
```

An outline has `items` (each with `kind`, `path`, `signature`, `summary`, `file`, `line` and `members`, the number of members) and `more_items`. A match beyond the first three is in `more_matches`, and any beyond those are counted in `unlisted_matches`.

## In a routine

[Routines](/routines) call it as `jouet.docs.lookup{pkg, symbol}`, with the `fs.read` capability. It returns `{ ok, exit_code, json, error, command }`, `json` being the document above.

## Exit codes

| Code | When |
|---|---|
| `0` | The outline or the definitions were printed |
| `2` | `--lang` is not `rust`, `ts`, `python` or `go` |
| `3` | No lockfile from here up names the package, the lockfile does not lock the version asked for, or the package has no definition matching the symbol |
| `4` | The package is locked but not installed, or installed at another version; or it is a package of this workspace rather than a dependency |
