---
description: Install jouet with mise, cargo or a release archive, make a first edit, and tell your agent about it.
---

# Getting started

jouet is one static binary with no runtime dependencies. It runs on Linux, macOS (Apple Silicon) and Windows.

::: warning Early development
[`jouet edit`](/commands/edit), [`jouet pr status`](/commands/pr-status), [`jouet pr threads`](/commands/pr-threads) and [`jouet wait`](/commands/wait) have shipped. Commands, flags, exit codes and JSON output may still change incompatibly between releases.
:::

## Install

### With mise

```sh
mise use -g jouet
```

The short name needs mise 2026.10.25 or later. With an older mise, name the repository instead:

```sh
mise use -g github:jdx/jouet
```

### With cargo

```sh
cargo install jouet --locked
```

To build the latest unreleased code instead:

```sh
cargo install --git https://github.com/jdx/jouet --locked
```

### From a release

Each [GitHub release](https://github.com/jdx/jouet/releases) has an archive per target, a `SHA256SUMS` file, and a signed [packslip](https://packslip.dev) that records how every archive was built:

| Platform | Archive |
|---|---|
| Linux x64 (glibc) | `jouet-x86_64-unknown-linux-gnu.tar.gz` |
| Linux x64 (musl) | `jouet-x86_64-unknown-linux-musl.tar.gz` |
| Linux arm64 (glibc) | `jouet-aarch64-unknown-linux-gnu.tar.gz` |
| Linux arm64 (musl) | `jouet-aarch64-unknown-linux-musl.tar.gz` |
| macOS (Apple Silicon) | `jouet-aarch64-apple-darwin.tar.gz` |
| Windows x64 | `jouet-x86_64-pc-windows-msvc.zip` |
| Windows arm64 | `jouet-aarch64-pc-windows-msvc.zip` |

The glibc archives need glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40). On older distributions, use the musl archive for your architecture: it is static and runs anywhere. mise picks it for you on those hosts.

Unpack the archive and put `jouet` (`jouet.exe` on Windows) on your `PATH`.

## Check it works

```sh
jouet --version
jouet edit --help
```

## Make a first edit

Hand `jouet edit` a spec on stdin. It checks every edit before it writes any file, so this changes both files or neither:

```spec
jouet edit <<'JOUET_EDIT'
=== src/main.rs
<<<<<<< SEARCH
    let x = 1;
=======
    let x = 2;
>>>>>>> REPLACE
=== Cargo.toml
<<<<<<< SEARCH
version = "0.1.0"
=======
version = "0.2.0"
>>>>>>> REPLACE
JOUET_EDIT
```

```output
ok          src/main.rs  1 edit  +1 -1
ok          Cargo.toml   1 edit  +1 -1
2 of 2 files written
```

If `Cargo.toml` had moved on to `0.1.1` since you read it, neither file would change, and the report would show the closest match:

```output
not written src/main.rs  1 edit  +1 -1
failed      Cargo.toml   1 edit
  edit 1: no match
    closest at line 3 (94% similar), - spec / + file:
    -version = "0.1.0"
    +version = "0.1.1"
0 of 2 files written
```

For a single replacement, skip the spec:

```sh
jouet edit replace src/main.rs --old 'let x = 2;' --new 'let x = 3;'
```

[`jouet edit`](/commands/edit) covers the other spec formats, `--check`, `--revert-on-fail` and `--expect-hash`.

## Tell your agent

Agents use what their instructions mention. For Claude Code or Codex, let jouet install a skill and hooks that steer the agent toward its commands:

```sh
jouet setup claude   # or: jouet setup codex
```

See [agent setup and hooks](/setup) for what that changes and how to turn each part off. For another harness, add the snippet `jouet skill --format agents-md` prints to its `AGENTS.md`; it points the agent at `jouet agent-help`, which always describes the installed version.

jouet notices when an agent runs it and switches to compact output by itself. See [display modes](/conventions#display-modes).

## Next steps

- [`jouet edit`](/commands/edit): spec formats, previews, `--check` and stale-read guards.
- [`jouet wait`](/commands/wait): CI, pull requests, logs, ports, URLs, processes and commands.
- [`jouet pr status`](/commands/pr-status) and [`jouet pr threads`](/commands/pr-threads): pull requests and their review threads.
- [Shared conventions](/conventions): display modes, JSON output and the exit codes every command shares.
- [Agent setup and hooks](/setup) and [rules](/rules): what `jouet setup` installs, and the Luau rules behind its hooks.
