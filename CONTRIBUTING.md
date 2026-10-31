# Contributing

> [!CAUTION]
> **AI replies to Discussions and Issues are restricted.** Only use AI to reply to a thread if you
> created it, opened a PR that fixes it, or have already had a contribution, attributed to your GitHub account, merged into the default branch of jouet.
> Everyone else is not allowed to, including with lightly edited, reviewed, or disclosed model
> output. Doing this is an instant ban across all of jdx's projects.
> Using AI to help write and file your own Discussion or Issue is fine. Review it before posting, and
> disclose that AI contributed.

For a suspected vulnerability, use the process in [SECURITY.md](SECURITY.md).

## Set up the repository

Install [mise](https://mise.jdx.dev) and a Rust toolchain through
[rustup](https://rustup.rs), then run from the repository root:

```sh
git submodule update --init --recursive
mise install
mise run build
```

The submodules are [bats](https://github.com/bats-core/bats-core) and its
helpers, pinned so that `mise run test` uses the same test runner as CI.

On Windows, `mise run test` runs the [Pester](https://pester.dev) tests in
`e2e-win/` instead of bats. They need PowerShell 7 (`winget install
Microsoft.PowerShell`) and Pester 5, which Windows does not ship (it has
Pester 3): `pwsh -Command "Install-Module Pester -MinimumVersion 5.0 -Scope
CurrentUser -Force -SkipPublisherCheck"`.

mise provides the repository's tools. Rust is not among them: the toolchain is
whichever one rustup makes current, and any stable release from 1.91, the
workspace's `rust-version`, builds it.

## mbx build cache

mise wraps `cargo` with [mbx](https://mr-boxington.jdx.dev), so compiled work is
shared across checkouts. `mise run` tasks and `mise exec -- cargo …` use the
wrapper; plain `cargo` does too once mise is [activated in your
shell](https://mise.jdx.dev/getting-started.html#activate-mise). Builds that set
`MBX_DISABLE=1`, including CI, skip the cache.

## Make a change

Keep the change focused and explain the behavior it improves. Use conventional
commit subjects and pull request titles such as `feat(edit): add --dry-run` or
`fix(wait): stop waiting on a conflicted PR`. See [AGENTS.md](AGENTS.md) for the
allowed types and repository rules.

Do not bump the crate version or edit the changelog in ordinary pull requests.
release-plz generates both.

## Breaking changes

[Stability and deprecations](docs/stability.md) lists what counts as breaking:
removing or renaming a command, flag, JSON field, config key, environment
variable or built-in rule, changing a field's type, or changing what an exit
code means.

Before 1.0, none of this applies: change what you need and remove the old
code, with no alias, fallback or warning.

From 1.0 on, keep the old form working where you can, and warn where it is
read:

```rust
jouet_output::deprecated_at!(
    "1.4.0",     // the first release that warns: the next one
    "2.0.0",     // the release that removes it
    "wait-poll", // a stable kebab-case id, with no values in it
    "`--poll` is now `--interval`"
);
```

The note goes to stderr once per agent session, and the hook ledger records
it. Once jouet's version reaches `remove_at`, a debug build fails the first
time that path runs, so the release that gets there removes the code. Pick
`remove_at` as the first major release at least 12 months after `warn_at`.
Document the deprecation in the command's page and use a `feat` or `fix`
commit, so the changelog carries it; the removal is a `!` commit with a
`BREAKING CHANGE:` footer.

Temporary code that has no user to warn, such as a fallback that reads a state
file an older jouet wrote, gets `jouet_output::remove_by!("2.0.0", "id")`
instead, which fails the build itself once jouet reaches that version.

## Check your work

```sh
mise run lint-fix
mise run ci
```

`mise run lint` includes `lint:deps`, which runs
[cargo-shear](https://github.com/Boshen/cargo-shear) and fails on a dependency
no code uses, including one left in `[workspace.dependencies]`, and on a `.rs`
file no crate compiles. `cargo-shear --fix` removes unused dependencies. If one
is needed though no code names it (it is there only to turn on a feature, for
example), add it to `ignored` under the crate's
`[package.metadata.cargo-shear]` with a comment saying why; test fixtures go in
`ignored-paths`.

[hk](https://hk.jdx.dev) runs the same rustfmt, Clippy and cargo-shear checks
from `hk.pkl`, plus jactionlint on the workflows: `mise x hk -- hk check --all` to verify,
`mise x hk -- hk fix` to fix, or `mise x hk -- hk install --mise` to run them on
commit.

## Documentation

The site at [jouet.jdx.dev](https://jouet.jdx.dev) is built with VitePress from
`docs/` and deployed from `main`. `docs/cli` is generated from the usage spec
the binary serves, so change a command's help in `src/main.rs`, never in those
files.

```sh
mise run render       # regenerate docs/cli
mise run docs         # local server with live reload
mise run docs:build   # what CI builds
```
