# Entire-hosted repository fixtures

What `entire` 0.11.4 printed with `--json` for et/jdx/jouet-entire-forge-test,
a private repository on Entire made for these tests, on 2026-10-31. The unit
tests in `entire_forge.rs` read them, and `test/entire-forge.bats`'s fake
`entire` replays them. Nothing in them is a credential; the logins and IDs are
the test repository's own.

- `show.json`: `entire trail show 1 --json --repo et/jdx/jouet-entire-forge-test`,
  the trail of branch `feat/subtract`. The repository has no CI and no gate
  configured, so its gates are all `skipped`.
- `show-failing.json`: **edited** from `show.json`, since no gate could be made
  to fail on the repository: the approvals, findings and up-to-date gates fail,
  with the rationales Entire gives on other trails, `mergeable` is false, and
  two check runs are added, one failed and one in progress, in the shape
  `trail show` gives runs on repositories with Buildkite.
- `show-2.json`: `entire trail show 2 --json`, an open trail with no
  discussions.
- `comment-list.json`, `comment-list-2.json`: `entire trail comment list
  --trail N --json` for trails 1 and 2. Trail 1 has an open discussion
  ("Tests") and a resolved one ("Naming").
- `comment-ID.json`: `entire trail comment show ID --trail 1 --json` for each
  of those two discussions. "Naming" holds the reply jouet posted with `jouet
  pr threads reply --resolve`.
- `approvals.json`: `entire trail approvals 1 --json`. The repository's
  approvals gate is disabled, so no decision could be recorded and the list is
  empty; the shape of a decision comes from other trails (`author`, `event`,
  `commitSha`, `createdAt`).
- `list-mine.json`: `entire trail list --author jdx --status open,draft --json
  -n 100`.
- `finding-open.json`, `finding-any.json`: `entire trail finding --trail 1
  --json -n 100`, and the same with `--status any --freshness any
  --include-dismissed`: one open finding, and one resolved and stale.
