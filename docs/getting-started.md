---
description: Install jouet with mise, cargo or a release archive, make a first edit, and tell your agent about it.
---

# Getting started

jouet is one static binary with no runtime dependencies. It runs on Linux, macOS (Apple Silicon) and Windows.

::: warning Early development
How much jouet helps depends on your language, test runner and agent, and your setup may need some configuration, or a fix in jouet, before you see any savings. Commands, flags, exit codes and JSON output may change incompatibly between releases until 1.0. See [what to expect](/why#early-development).
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

## Shell completions

`jouet completion SHELL` prints a completion script for bash, zsh, fish or PowerShell. The script asks the `jouet` on your `PATH` for each completion, so it needs nothing else installed and keeps up with upgrades without being regenerated.

::: code-group

```bash [Bash]
mkdir -p ~/.local/share/bash-completion/completions
jouet completion bash > ~/.local/share/bash-completion/completions/jouet
```

```zsh [Zsh]
mkdir -p ~/.zfunc
jouet completion zsh > ~/.zfunc/_jouet
```

```fish [Fish]
mkdir -p ~/.config/fish/completions
jouet completion fish > ~/.config/fish/completions/jouet.fish
```

```powershell [PowerShell]
jouet completion powershell | Out-String | Invoke-Expression
```

:::

Bash needs `bash-completion` loaded, or source the file from `~/.bashrc`. For zsh, add `~/.zfunc` to `fpath` before `compinit` in `~/.zshrc`:

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit
compinit
```

For PowerShell, put that line in your `$PROFILE`. Start a new shell afterwards.

Each release also carries the scripts (`jouet.bash`, `jouet.zsh`, `jouet.fish`, `jouet.powershell`) and a man page, `jouet.1`, beside the archives. With `mise use -g jouet` and [mise activated](https://mise.jdx.dev/shell-setup.html), you need none of this: mise installs jouet from its signed packslip, loads its completions in bash, zsh, fish and PowerShell, and makes `man jouet` open the active version's page. An install through `github:jdx/jouet` or cargo does not get them.

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

For Claude Code, Codex, Cursor, Gemini CLI or Copilot, install the hooks and project onboarding skill:

```sh
jouet setup claude   # or: codex, cursor, gemini, copilot
```

See [agent setup and hooks](/setup) for what that changes and how to turn each part off. For another harness, add the snippet `jouet skill --format agents-md` prints to its `AGENTS.md`; it points the agent at `jouet agent-help`, which always describes the installed version.

## Initialize your project

Start a new agent session in the project and invoke **jouet-init**:
`/jouet-init` in Claude Code, `$jouet-init` in Codex, or ask:

```text
Use the jouet-init skill to configure jouet for this project.
```

The skill inspects your project instructions, build and test workflows, existing
configuration and timing history. It helps the agent choose useful settings,
avoid unnecessary expensive runs, and verify what it changes. If no extra
configuration is needed, it says so. It leaves repository trust for you to
review. [What onboarding does →](/project-setup)

jouet notices when an agent runs it and switches to compact output by itself. See [display modes](/conventions#display-modes).

## Next steps

- [`jouet edit`](/commands/edit): spec formats, previews, `--check` and stale-read guards.
- [`jouet wait`](/commands/wait): CI, pull requests, logs, ports, URLs, processes and commands.
- [`jouet pr status`](/commands/pr-status) and [`jouet pr threads`](/commands/pr-threads): pull requests and their review threads.
- [Shared conventions](/conventions): display modes, JSON output and the exit codes every command shares.
- [Agent setup and hooks](/setup) and [rules](/rules): what `jouet setup` installs, and the rules behind its hooks.
