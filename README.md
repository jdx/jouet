<p align="center">
  <img src="docs/public/logo.svg" alt="The jouet logo, a coral wind-up tin robot with a brass key on top" width="160">
</p>

<h1 align="center">jouet</h1>

<p align="center">
  <strong>Wind-up tools for coding agents.</strong><br>
  The shell chores agents keep scripting by hand, as one command each.
</p>

<p align="center">
  <a href="https://jouet.jdx.dev/getting-started">Get started</a> ·
  <a href="https://jouet.jdx.dev">Documentation</a> ·
  <a href="https://jouet.jdx.dev/why">Why jouet</a> ·
  <a href="https://github.com/jdx/jouet/releases">Releases</a>
</p>

jouet (pronounced "zhweh", French for "toy") is a command-line toolkit for
coding agents. Agents rebuild the same few chores in every session: patching
files from Python heredocs, sleep-polling CI, hand-writing GraphQL for review
threads. The scripts look right, usually work, and fail in ways nobody
notices. jouet turns each chore into one command that never prompts, never
cuts output silently, and exits with a code an agent can act on.

> [!CAUTION]
> **jouet is in early development.** `jouet edit`, `jouet pr status`,
> `jouet pr threads` and `jouet wait` have shipped. Commands, flags, exit codes
> and JSON output may change incompatibly between releases.

## Install

```sh
mise use -g jouet
```

The short name needs mise 2026.10.25 or later; with an older mise, run
`mise use -g github:jdx/jouet`. Or use Cargo:

```sh
cargo install jouet --locked
```

Release archives for Linux, macOS (Apple Silicon) and Windows are on the
[releases page](https://github.com/jdx/jouet/releases), each with a signed
[packslip](https://packslip.dev). The glibc Linux archives need glibc 2.39 or newer; on older
distributions use the static musl archive, which mise picks automatically.
[All installation options →](https://jouet.jdx.dev/getting-started)

## Before and after

An agent makes two edits, but `Cargo.toml` changed since it was read. The
heredoc agents write today stops halfway:

```sh
python3 - <<'EOF'
from pathlib import Path
for path, old, new in [
    ("src/main.rs", "let x = 1;", "let x = 2;"),
    ("Cargo.toml", 'version = "0.1.0"', 'version = "0.2.0"'),
]:
    s = Path(path).read_text()
    assert s.count(old) == 1, path
    Path(path).write_text(s.replace(old, new))
EOF
# AssertionError: Cargo.toml, and src/main.rs is already rewritten
```

`jouet edit` checks every edit before it writes any file:

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
not written src/main.rs  1 edit  +1 -1
failed      Cargo.toml   1 edit
  edit 1: no match
    closest at line 3 (94% similar), - spec / + file:
    -version = "0.1.0"
    +version = "0.1.1"
0 of 2 files written
```

Nothing changed, exit code `4` says the request was refused, and the closest
match shows what moved.

## Commands

| Command | Status | Replaces |
|---|---|---|
| [`jouet edit`](https://jouet.jdx.dev/commands/edit) | Available | heredoc edits with a hand-written `assert s.count(old) == 1` |
| [`jouet wait`](https://jouet.jdx.dev/commands/wait) | Available | `sleep` and `until` loops around `gh pr checks`, log files, ports, URLs and processes |
| [`jouet pr status`](https://jouet.jdx.dev/commands/pr-status) | Available | `for` loops over `gh pr view` |
| [`jouet pr threads`](https://jouet.jdx.dev/commands/pr-threads) | Available | hand-written `reviewThreads` GraphQL |

Every command follows the same
[conventions](https://jouet.jdx.dev/conventions): no prompts and no TTY,
output that adapts to whoever is reading, `--json` that prints exactly one
document, and one table of exit codes.

After the commands, jouet will help agents reach for them: hooks that catch a
costly pattern and point to the command that replaces it, and rules you write
yourself. See [what comes next](https://jouet.jdx.dev/why#what-comes-next).

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup. Report
suspected vulnerabilities through the private process in
[SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
