---
description: Install jouet, connect it to your coding agent with jouet setup, and see what changes in the next session.
---

# Getting started

jouet is one static binary with no runtime dependencies. It runs on Linux, macOS (Apple Silicon) and Windows. This page installs it, connects it to your agent, and shows what to look at once it is running.

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

Unpack the archive and put `jouet` (`jouet.exe` on Windows) on your `PATH`, then check it runs:

```sh
jouet --version
```

## Connect your agent

For Claude Code, Codex, Cursor, Gemini CLI or Copilot, `jouet setup` adds jouet's hooks to the harness's settings file:

```sh
jouet setup claude   # or: codex, cursor, gemini, copilot
```

`--dry-run` shows the change without writing it, and `--uninstall` takes jouet's entries out again. Codex skips new hooks until you trust them, so after `jouet setup codex`, run `/hooks` in Codex and trust jouet's.

From the next session on:

- The agent starts with a short primer: jouet's commands by family, how to edit and commit with them, the rules that deny and what each suggests instead, and, once jouet has timed a few runs, the slowest checks in this repository.
- Before each shell command, jouet's [rules](/rules) run. A handful of hand-rolled patterns, such as a `sleep` loop around `gh pr checks` or a Python heredoc that edits a file, are denied with one line naming the command to use instead.
- Test, lint and build runs are timed for [`jouet cost`](/commands/cost). If the agent tries to finish while its last run failed, or after changing files with no run passing since, it is sent back once.

[Agent setup and hooks](/setup) covers each harness, installing for one repository only, and turning each part off.

For another harness, add the snippet that `jouet skill --format agents-md` prints to its `AGENTS.md`. It points the agent at `jouet agent-help`, which always describes the installed version. The agent gets the commands that way, but no primer, rules or stop check.

## See where your sessions go

[`jouet audit`](/commands/audit) needs no setup. It reads the Claude Code and Codex transcripts already on your machine and reports where they spent input tokens and time, with the jouet command or rule that addresses each costly category:

```sh
jouet audit --since 7d
```

Once the hooks have run for a while, [`jouet cost`](/commands/cost) lists how long each check takes on this machine, and [`jouet stats`](/commands/stats) estimates what jouet saved, against your history from before setup. [Self-winding](/winding) starts proposing rules and mise tasks when the same failure or the same commands keep coming up.

## Try an edit

`jouet edit replace` changes one string, and refuses unless the old text matches exactly once:

```sh
jouet edit replace src/main.rs --old 'let x = 1;' --new 'let x = 2;'
```

For several replacements, or several files, hand `jouet edit` a spec on stdin. It checks every edit before it writes any file, so this changes both files or neither:

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

[`jouet edit`](/commands/edit) covers the other spec formats, `--check`, `--revert-on-fail` and `--expect-hash`.

jouet notices when an agent runs it and switches to compact output by itself; see [display modes](/conventions#display-modes).

## Shell completions

With `mise use -g jouet` and [mise activated](https://mise.jdx.dev/shell-setup.html), there is nothing to do: mise installs jouet from its signed packslip, loads its completions in bash, zsh, fish and PowerShell, and makes `man jouet` open the active version's page. An install through `github:jdx/jouet` or cargo does not get them.

Otherwise, `jouet completion SHELL` prints a completion script for bash, zsh, fish or PowerShell. The script asks the `jouet` on your `PATH` for each completion, so it needs nothing else installed and keeps up with upgrades without being regenerated.

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

Each release also carries the scripts (`jouet.bash`, `jouet.zsh`, `jouet.fish`, `jouet.powershell`) and a man page, `jouet.1`, beside the archives.

## Next steps

- [Agent setup and hooks](/setup): what `jouet setup` installs for each harness, and the settings that turn each part off.
- [Self-winding](/winding) and [rules](/rules): how jouet drafts rules and tasks from what keeps going wrong, and how to write your own.
- [`jouet test --affected`](/commands/test), [`jouet wait`](/commands/wait) and [`jouet pr status`](/commands/pr-status): running only the tests a change reaches, waiting on CI and other conditions, and the state of many pull requests at once.
- [Shared conventions](/conventions): display modes, JSON output and the exit codes every command shares.
