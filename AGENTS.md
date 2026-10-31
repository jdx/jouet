# jouet Development Guide

jouet is a toolkit CLI for coding agents: `jouet wait` (CI and other conditions), `jouet pr threads`
(review threads), `jouet pr status` (many PRs at once), `jouet pr next` and `jouet pr land` (what a
PR needs next, and merging one that is ready), `jouet edit` (exact, all-or-nothing file edits),
`jouet search` and `jouet read --map` (budgeted search and a repository map), `jouet audit` (where
your agent sessions spend tokens and time), `jouet stats` (what jouet saved), `jouet test
--affected` (only the tests a change affects), and the git recipes `jouet commit`,
`jouet review-package` and `jouet handoff`/`pickup`. It is in
early development; the commands land one at a time.

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
mise run build        # cargo build --workspace --all-targets
mise run test         # cargo test --workspace --all-targets, then the bats tests in test/
mise run test:bats    # only the bats tests
mise run lint         # cargo fmt --all --check + clippy --workspace --all-targets --all-features -D warnings
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

CI also runs `cargo deny check` (`deny.toml`), the same tests on macOS, and `cargo check` on the
MSRV in `Cargo.toml`'s `rust-version`. Windows runs only clippy and the Pester tests in `e2e-win/`.

## Crates

The repository is a Cargo workspace. The `jouet` package at the root is the binary: the usage-rs
CLI declaration in `src/main.rs`, the command glue in `src/cmd/`, the config file, the git
helpers, the setup, hook, ledger and agent-guide code, and the test selection behind `jouet test
--affected` (`src/affected/`: the changed set, the fallbacks, and one adapter per test runner). The rest lives in library crates under
`crates/`:

- `jouet-output`: the shared conventions: exit codes, display modes, `Output` (results and the
  error document), time parsing and formatting, and the log and state directories.
- `jouet-github`: the GitHub client (token lookup, retries, GraphQL and REST, ETags,
  `JOUET_GITHUB_API_URL`), TLS, and the `pr` model shared by `pr status`, `pr threads` and `wait`.
- `jouet-edit`: the edit engine behind `jouet edit`.
- `jouet-rules`: the Luau rule engine and the built-in rules behind `jouet hook` and `jouet rule`,
  and its read-only probes (`probe/`).
- `jouet-routines`: routines, the Luau programs `joue` (`jouet run`) runs: headers and the usage
  spec (`routine.rs`), the sandboxed runtime and `joue` module (`runtime.rs`), the primitives
  (`prims/`), `joue test` and its fake GitHub (`testing.rs`), and the built-in routines and their
  fixtures (`routines/`). `jouet pr next` is the `pr-next` routine.
- `jouet-wait`: the conditions and polling behind `jouet wait`.
- `jouet-audit`: the transcript readers (Claude Code, Codex), command shapes, redaction,
  categories, patterns and the analyzer behind `jouet audit`, kept reusable for `jouet wind`, and
  `stats`, the ledger reader and estimates behind `jouet stats`.
- `jouet-search`: `jouet search` (ripgrep's walker and searcher, with budgets and grouping by
  enclosing symbol), the session memory of what an agent was shown, tree-sitter symbols for the
  languages the `grammars` feature compiles in, the repo map behind `jouet read --map`, and
  structural search and rewrites on `ast-grep-core`, which shares tree-sitter with the grammars.
  Without the feature (`--no-default-features`), search is plain text and there is no map.
- `jouet-git`: the git recipes behind `jouet commit`, `jouet review-package`, `jouet handoff` and
  `jouet pickup`. It shells out to `git` with literal pathspecs; the PR lookup for `handoff` stays
  in the binary, so the crate makes no network calls.

The libraries export only what the binary or a sibling crate uses; `unreachable_pub` is denied
workspace-wide, so everything else is `pub(crate)`. Shared fields go in `[workspace.package]`,
dependency requirements in `[workspace.dependencies]`, and each crate inherits both. Every crate
shares one version (see `release-plz.toml`). The subprocess tests in `tests/` and the bats and
Pester tests stay at the root, since they run the binary.

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
- **Agent guide and hooks:** `jouet agent-help`, the skill and the session-start primer are
  generated from the usage-rs declaration. A new command adds one line to `WHEN` in
  `src/guide.rs` (a test fails until it does). If it replaces a pattern agents hand-roll, add a
  built-in rule with `needs` naming the command. Hooks must fail open: `jouet hook` always exits
  0, never 2.
- **Rules:** the hooks' steering rules are `[[rule]]` tables in TOML, one file per built-in rule
  in `crates/jouet-rules/rules/`: a matcher (`match`), a `message` and `suggest`, and `why`,
  `mode`, `scope`, `author`, `created`. The types in `crates/jouet-rules/src/decl.rs` are both the
  parser and the JSON schema (`jouet rule schema`, published as `docs/public/schema/rules.json`;
  regenerate it with `jouet rule schema >docs/public/schema/rules.json`, or a bats test fails).
  Matching is plain Rust (`matcher.rs`), so a rule without a script never starts a Luau VM; keep
  the matcher small, and give a rule with real logic a `script` instead (only `checks` has one).
  Scripts are written against the public `jouet` module (`crates/jouet-rules/jouet.d.luau`,
  printed by `jouet rule types`). User and repository rules may only add decisions; keep that
  floor (no replacing built-in rules, no locking out `jouet`) when changing the engine. A rule's
  `examples` are its tests: `cargo test` runs every built-in rule's, so add matching and near-miss
  commands with each change. A rule whose advice names a jouet command declares it in `needs`.
  Keep `jouet.d.luau` in step with `install_module`, `prelude.luau` and the probes, and check it
  with `luau-lsp analyze --definitions=jouet.d.luau rules/*.luau`. Whole-file `.luau` rules still
  load but are deprecated (`LUAU_RULES_REMOVED_IN`); a test fails once jouet reaches that version.
- **Probes:** probes only read, are cached per hook call, and run within the rule's remaining
  budget; the GitHub ones stay off in hooks unless `rules.network` is set.
- **Routines:** routines stay Luau programs, unlike rules. A routine reaches the world only through
  the primitives in `crates/jouet-routines/src/prims/`, each refused without its capability. A
  primitive that writes goes through `Ctx::write`, so it is previewed without `--yes`; never add
  one that runs a shell command or trusts a repository. Keep `routines.d.luau` in step with the
  primitives and the `joue` module. Every built-in routine carries examples with fixtures in
  `routines/fixtures/` (listed in `testing::FIXTURES`), which `cargo test` runs, and the `pr-next`
  routine must agree with `jouet_github::pr::next`, which `pr land` and the probes still use
  (`parity.rs`). `joue` is a usage view of `jouet run`; `src/bin/joue.rs` is the launcher that
  ships it as a file of its own.
- **Tests:** the CLI surface is tested by running the built binary (`tests/cli.rs`). GitHub
  responses are to be recorded as fixtures and replayed, never fetched live in tests.
- **End-to-end tests** run a command the way an agent does, from a shell: one bats file per
  command in `test/` (bats is a git submodule; `git submodule update --init --recursive`). Windows
  gets Pester tests in `e2e-win/` only for what can differ there (line endings, paths, cmd.exe,
  file locking) and for Windows regressions; Windows CI does not run the full suite.

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
  `130` interrupted. `1` and `4` are deliberately separate. `pr next` alone adds `10` (act) and
  `20` (waiting), outside the table on purpose.
- **No prompts and no TTY requirement.** Display style is chosen from an explicit flag,
  `JOUET_DISPLAY`, agent detection (`mise-agent-env`), then whether stdout is a TTY. The one
  prompt is `pr land`'s confirmation, shown only when `display::person_at_terminal()` (stdin and
  stderr are terminals, no agent detected); everyone else passes `--yes` or gets exit 2.

## Conventional commits

PR titles must use `<type>[optional scope][optional !]: <description>`; intermediate commit
subjects should use the same format. Start the description with a lowercase character or an
acronym such as `CLI`, in the imperative mood.

**Types:** `feat`, `fix`, `refactor`, `docs`, `style`, `perf`, `test`, `chore`, `ci`, `revert`,
`security`

**Scopes:** a command (`wait`, `threads`, `status`, `edit`, `rule`, `setup`, `commit`, `handoff`) or a
subsystem (`github`, `git`, `output`, `config`, `release`, `deps`).

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
- Published crates carry source and licence only. Each crate's `Cargo.toml` lists what it ships
  in `include`, so nothing else in a checkout is packaged; a new file that must ship goes there.

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
