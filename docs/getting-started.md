---
description: Install jouet with mise, cargo or a release binary, and make your first edit.
---

# Getting started

jouet is one static binary with no runtime dependencies. It runs on Linux, macOS (Apple Silicon) and Windows.

::: warning Early development
jouet is in early development, and only [`jouet edit`](/commands/edit) is available so far. Its commands, flags, exit codes and JSON output are not finalized and may change incompatibly between releases.
:::

## Install

### With mise

```sh
mise use --global github:jdx/jouet
```

### With cargo

```sh
cargo install jouet
```

To build the latest unreleased code instead:

```sh
cargo install --git https://github.com/jdx/jouet
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

Unpack the archive and put `jouet` (`jouet.exe` on Windows) somewhere on your `PATH`.

## Check it works

```sh
jouet --help
jouet edit --help
```

## A first edit

Hand `jouet edit` a spec on stdin. Every edit is checked before any file is written, so this either changes both files or neither:

```sh
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

```text
ok          src/main.rs  1 edit  +1 -1
ok          Cargo.toml   1 edit  +1 -1
2 of 2 files written
```

For a single replacement, skip the spec:

```sh
jouet edit replace src/main.rs --old 'let x = 2;' --new 'let x = 3;'
```

See [`jouet edit`](/commands/edit) for the other spec formats, `--check`, `--revert-on-fail` and `--expect-hash`.

## Tell your agent

Agents reach for whatever their instructions mention. For Claude Code or Codex, let jouet install a skill and hooks that steer the agent toward its commands:

```sh
jouet setup claude   # or: jouet setup codex
```

See [agent setup and hooks](/setup) for what that changes and how to turn each part off. For another harness, add the snippet `jouet skill --format agents-md` prints to its `AGENTS.md`; it points the agent at `jouet agent-help`, which always describes the installed version.

jouet notices when an agent is running it and switches to compact output on its own; see [shared conventions](/conventions#display-modes).
