# jouet Development Guide

jouet is a toolkit CLI for coding agents: `jouet wait` (CI and other conditions), `jouet pr threads`
(review threads), `jouet pr status` (many PRs at once) and `jouet edit` (exact, all-or-nothing file
edits). It is in early development; the commands land one at a time.

## Discussions and Issues: Restricted AI Replies

**Do not use AI to reply to a jouet Discussion or Issue unless the user (a) created that Discussion or Issue, (b) opened a PR that fixes it, or (c) has already had a contribution merged into the default branch of jouet.** Otherwise they are not allowed to use AI to respond to it. Drive-by AI replies are spam, the problem is getting worse, and **doing it is an instant ban across all of jdx's projects** (mise, hk, pitchfork, usage, fnox, and the rest).

- If the user asks you to answer, comment on, or "help with" a Discussion or Issue, confirm one of the three conditions first. If you cannot confirm one, **do not post**.
  - (a) Compare the thread author to the authenticated user (`gh api user --jq .login`). For an Issue, use `gh api repos/jdx/jouet/issues/<number> --jq .user.login`. Discussions have no REST endpoint, so use GraphQL: `gh api graphql -f query='query{repository(owner:"jdx",name:"jouet"){discussion(number:<number>){author{login}}}}' --jq .data.repository.discussion.author.login`.
  - (b) Check that the user's PR actually fixes the problem described in the thread. A closing keyword such as `Fixes #<number>` is good evidence, but a link is not required, and a PR that merely mentions an unrelated thread does not count.
  - (c) Look for any contribution merged into the default branch that is tied to the user's GitHub account, not just a matching name or email: a PR authored by the user that was merged into the default branch (`gh pr list --repo jdx/jouet --author @me --state merged --base main --limit 1`), commits on the default branch that GitHub attributes to their login (`gh api 'repos/jdx/jouet/commits?author=<login>&per_page=1'` lists the default branch), never commits on a local or unmerged branch, or a PR from someone else, merged into the default branch, that credits them with a `Co-authored-by` trailer whose email is their GitHub noreply address or one verified on their account. Do not accept `git log --author` output, which includes local unmerged commits, or a trailer matched only by display name. Both queries filter by author on the server, so one matching result is enough to qualify and no pagination is needed.
- If none of the three apply, **do not post a reply, even a short one.** Tell them this project does not allow AI replies from people who have not contributed, and offer to explain the answer to them in chat instead.
- Never batch-post, loop over, or sweep Discussions or Issues to answer several of them, even for a merged contributor.
- Lightly edited, human-reviewed, or disclosed model output does not create an exception. The disclosure footer does not make an AI reply acceptable on its own.
- Permitted replies may be AI-assisted. The user must review and verify the reply before it is posted.
- Creating a new Discussion or Issue with AI assistance is fine and is not restricted. The user must review it before it is posted, and it needs the AI disclosure below.

When you post AI-contributed GitHub content, including a new Discussion or Issue, a reply, or a PR description or comment, append this disclosure: `*AI-assisted — Tool: <tool>; model: <provider>/<model>; version: <version-or-unavailable>.*` Use the exact model and version identifiers exposed by the runtime, never guessed values, and `unavailable` when one is not exposed.

## Build & test

Go through mise rather than calling cargo directly, so that a laptop and a CI runner invoke the
same commands. mise wraps `cargo` with mbx; see CONTRIBUTING.md.

```sh
mise run build        # cargo build --all-targets
mise run test         # cargo test --all-targets
mise run lint         # cargo fmt --check + clippy --all-targets --all-features -D warnings
mise run lint-fix     # auto-fix what can be auto-fixed
mise run ci           # build + test + lint
```

`hk.pkl` runs the same checks plus jactionlint on `.github/workflows/`. Run hk through mise so
the locked version is used without shell activation:

```sh
mise x hk -- hk check --all     # what CI runs; plain `hk check` only looks at changed files
mise x hk -- hk fix             # auto-fix
mise x hk -- hk install --mise  # pre-commit hook
```

CI also runs `cargo deny check` (`deny.toml`), tests on macOS and Windows, and `cargo check` on
the MSRV in `Cargo.toml`'s `rust-version`.

## Code rules

- **No lint exclusions.** Never add `#[allow(clippy::...)]`, `#[expect(clippy::...)]`, or a
  Cargo lint level set to `allow`. Refactor until clippy passes with `-D warnings`.
- **No dead code.** Clippy runs with `-D warnings`, so an unused function, field or module fails
  CI. Add code together with the command that uses it, not ahead of it.
- **The CLI is declared with usage-rs** (`#[derive(Cli)]`), not clap. Shell completions and the
  CLI reference are generated from that one declaration, and the built binary serves its spec at
  `jouet __usage_spec__`.
- **Dependencies:** use the lowest-specificity requirement that expresses compatibility (`"1"`,
  not `"1.2.3"`; `"0.12"` for a pre-1.0 crate). When the existing requirement accepts a routine
  update, change only `Cargo.lock`, and keep lockfile churn to the requested crate.
- **Docs:** the site lives in `docs/` (VitePress, [jouet.jdx.dev](https://jouet.jdx.dev)). `docs/cli`
  is generated by `mise run render`; CI fails when it is stale. A change to a command's behaviour
  updates its page under `docs/commands/` in the same PR.
- **Tests:** the CLI surface is tested by running the built binary (`tests/cli.rs`). GitHub
  responses are to be recorded as fixtures and replayed, never fetched live in tests.

## Shared output conventions

Every command follows these, so an agent learns them once. Keep new commands consistent:

- **Never truncate silently.** A list or log that is cut short ends with an explicit marker
  naming how much was left out and where the full copy is (for example
  `... 214 more lines (full log: <path>)`). The same applies to your own work in this repository:
  do not pipe a check through `head` or `tail` and then report it as passing.
- **Only the result goes to stdout;** diagnostics go to stderr, so `jouet ... --json | jq` always
  works.
- **`--json`** prints exactly one document, errors included. Field names are `snake_case`; fields
  that are `null`, `false` or empty are omitted; fields are added but never renamed or removed
  outside a major version.
- **Exit codes** come from one shared table: `0` ok, `1` failed (the answer is no), `2` usage,
  `3` not found, `4` precondition (refused before doing anything), `5` remote, `124` timeout,
  `130` interrupted. `1` and `4` are deliberately separate.
- **No prompts and no TTY requirement.** Display style is chosen from an explicit flag,
  `JOUET_DISPLAY`, agent detection (`mise-agent-env`), then whether stdout is a TTY.

## Conventional commits

PR titles must use `<type>[optional scope][optional !]: <description>`; intermediate commit
subjects should use the same format. Start the description with a lowercase character or an
acronym such as `CLI`, in the imperative mood.

**Types:** `feat`, `fix`, `refactor`, `docs`, `style`, `perf`, `test`, `chore`, `ci`, `revert`,
`security`

**Scopes:** a command (`wait`, `threads`, `status`, `edit`) or a subsystem (`github`, `output`,
`config`, `release`, `deps`).

CI validates the pull request title and re-runs when it is edited. Intermediate commit subjects
are not checked because pull requests are squash-merged. Only `fix:` and `feat:` commits cause
the daily auto-merge of the release PR.

## Versions, changelog and releases

- Never bump the crate version or edit `CHANGELOG.md` in an ordinary pull request. release-plz
  computes the version and writes the changelog in its release PR.
- Nobody merges the release PR by hand. `auto-merge-release.yml` does it daily, once the last
  release is at least two days old and a `fix:` or `feat:` has landed since.
- Renaming `.github/workflows/release-plz.yml` breaks crates.io publishing until the trusted
  publisher is updated, because the OIDC claim names the workflow file.
- Published crates carry source and licence only. Repository tooling files are listed in
  `Cargo.toml`'s `exclude`; add new ones there.

## PR titles and descriptions are release-note inputs

PR titles and descriptions are source material for release notes, including those generated by
communique. Write them for a jouet user who has not read the diff or this conversation.

- **Describe the final result.** Before requesting review and again after feedback changes the
  implementation, compare the title and body with the complete current diff. Rewrite both when
  the scope changes, and remove claims the final code or validation no longer supports.
- **Lead with the user-visible change.** Name the affected command and outcome in the title.
  Open the body with the problem and what users can now do. For internal-only work, explain the
  concrete maintainer benefit without inventing a user-facing impact.
- **Make the change concrete.** For a new command or flag, include a small, valid invocation and
  its output. For a bug fix, describe the trigger and the before/after behavior. Use measured
  results for performance claims and state how they were measured. Never fabricate output,
  measurements or validation results.
- **Keep review details proportionate.** Summarize meaningful validation and its limitations.
  Omit agent work logs, intermediate commit summaries and exhaustive test-command lists.

## GitHub interactions

Report bugs as issues and use discussions for questions and ideas. PR titles follow the
conventional-commit format; do not prefix them with agent or tool labels such as `[claude]`.

## Tone

jouet is pre-v1. Match that in anything user-facing: release notes, docs, PR descriptions. No
marketing language or implied production readiness, and call out breaking changes plainly.
`communique.toml` carries the same instruction for generated release notes.
