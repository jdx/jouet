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
mise install
mise run build
```

mise provides the repository's tools. Rust is not among them: the toolchain is
whichever one rustup makes current, and any stable release from 1.91, the
crate's `rust-version`, builds it.

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

## Check your work

```sh
mise run lint-fix
mise run ci
```

[hk](https://hk.jdx.dev) runs the same rustfmt and Clippy checks from `hk.pkl`,
plus jactionlint on the workflows: `mise x hk -- hk check --all` to verify,
`mise x hk -- hk fix` to fix, or `mise x hk -- hk install --mise` to run them on
commit.
