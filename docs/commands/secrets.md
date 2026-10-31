---
description: Find API keys, tokens and private keys in files, in what is staged, or in commits about to be pushed, with gitleaks' rules, before they are committed or pushed.
---

# jouet secrets

An agent that pastes a token into a config file or a test commits it with everything else, and once the commit is pushed the token is in the history of every copy. Taking it out later does not take it back; only rotating it does. `jouet secrets scan` looks for secrets with [gitleaks](https://github.com/gitleaks/gitleaks)' detection rules, built in. `jouet commit` runs the same scan over what it is about to commit, and a hook rule runs it before `git commit` and `git push`.

```text
jouet secrets scan [PATHS]... [--staged | --diff REF]
```

Every flag is listed in the [CLI reference](/cli/secrets/scan).

## What it scans

| Invocation | What is read |
|---|---|
| `jouet secrets scan [PATHS]` | The files and directories given (the current directory by default), as they are on disk. Directories are walked as git sees them, so ignored files are left out unless named. |
| `jouet secrets scan --staged [PATHS]` | Only the lines the index adds to HEAD: what `git commit` would record. |
| `jouet secrets scan --diff REF [PATHS]` | The lines each commit in `REF..HEAD` adds, commit by commit, so a secret added in one commit and removed in the next is still found. `REF` may be a range, `A..B`. |

With `--staged` or `--diff`, the paths limit the diff. Only added lines are read, so a secret already in HEAD is not reported again by every later commit.

```output
$ jouet secrets scan
found 2 secrets in 2 files (gitleaks v8.30.1, 222 rules)
  config/app.toml:2  slack-bot-token   xoxb****
  src/aws.rs:1       aws-access-token  AKIA****
  a false positive: add `gitleaks:allow` to its line, or its fingerprint (PATH:RULE:LINE, --json lists them) to .gitleaksignore
(exit 1)

$ jouet secrets scan --staged
found 1 secret in 1 staged file (gitleaks v8.30.1, 222 rules)
  config/app.toml:2  slack-bot-token  xoxb****
  a false positive: add `gitleaks:allow` to its line, or its fingerprint (PATH:RULE:LINE, --json lists them) to .gitleaksignore
(exit 1)
```

Each finding names its rule, its file and line, and the match, redacted. The redacted match keeps up to 40 characters of what comes before the secret on its line (`api_key = "`), and at most the secret's first four characters, only when the secret is 16 characters or longer: a token's prefix, such as `ghp_`. The rest of the secret is never printed, in any output.

A scan of commits also names each finding's commit. Binary files are checked by name only (a `.p12` or `.pfx` file is a finding of its own), files over 10 MiB are listed as not scanned, and lockfiles, images, fonts and vendored code are left out by gitleaks' own allowlist.

## The rules

The rules are gitleaks' `config/gitleaks.toml`, vendored unchanged into jouet (gitleaks is MIT; see `NOTICE`), and applied the way gitleaks applies them:

- A rule runs only on text holding one of its keywords, ignoring case. Its regex is compiled the first time one appears, so a scan pays only for the rules its text could match.
- The secret is the rule's `secretGroup`, else the first group that matched something, else the whole match.
- A rule with an `entropy` needs a secret above it (Shannon entropy, in bits per character), and gitleaks' generic rules also want a digit in it.
- A rule's `path` limits it to matching files; a rule with only a `path` matches the file's name.
- Allowlists, the rule's own and the config's, pass a finding over when their `paths`, `regexes` (against the secret, or with `regexTarget` the match or the line), `stopwords` or `commits` match, or all of them with `condition = "AND"`.
- A generic rule's finding is dropped when a more specific rule found the same secret.

gitleaks' regexes are Go's RE2 syntax, which jouet reads with Rust's `regex` crate in ASCII mode, as Go does, and translates where Go takes a character literally that `regex` would not (a `{` that starts no repetition, a `[` inside a class). A test checks that every default rule compiles. gitleaks' decoding of base64 and hex before matching is not done, so an encoded secret is missed.

`mise run secrets:update` vendors the latest gitleaks release's rules and licence, and records its version in `crates/jouet-secrets/gitleaks/VERSION`.

## False positives

gitleaks' rules match on shape, and the generic ones guess. A finding that is not a secret, such as test data, can be passed over four ways:

- **`gitleaks:allow`** anywhere on its line, as in gitleaks.
- **`.gitleaksignore`** at the repository's root, one fingerprint per line (`#` starts a comment). A fingerprint is `PATH:RULE:LINE`, after `COMMIT:` for a finding in a commit; `--json` gives each finding's. A line-based fingerprint stops matching when the line moves.
- **`.gitleaks.toml`** at the repository's root. With `[extend] useDefault = true`, its rules and allowlists are added to the defaults and `disabledRules` turns rules off; a file with only allowlists is added to the defaults too. A file with rules and no `[extend]` replaces the defaults, as in gitleaks. `[extend] path` and `url` are not read.
- **`jouet commit --allow-secret RULE:PATH:LINE`**, for one commit: see [below](#before-a-commit).

## Before a commit

[`jouet commit`](/commands/commit#secret-scanning) builds a temporary index from HEAD plus the named paths, exactly what it will commit, and scans the lines it adds, together with fnox's check for the project's secret values in a fnox project. A finding refuses the commit, with nothing committed and nothing left staged:

```output
$ jouet commit config/app.toml src/aws.rs -m "feat: add the slack bot"
commit refused: gitleaks' rules found a secret in what would be committed; nothing was committed and nothing was left staged
  slack-bot-token   config/app.toml:2  xoxb****
  aws-access-token  src/aws.rs:1       AKIA****
  remove the secret and read it from the environment instead; for a false positive, commit again with --allow-secret RULE:PATH:LINE for each, or add `gitleaks:allow` to its line
(exit 1)
```

`--allow-secret` takes a finding as the refusal lists it, and is repeatable. It applies to gitleaks' findings only: a fnox finding is the actual value of one of the project's secrets.

## Before `git commit` and `git push`

The `committed-secrets` [hook rule](/setup#steering-rules) runs the same scan within the hooks' probe budget:

- **`git commit`**: the staged lines, every tracked change for `git commit -a`, and the paths a `git add` earlier in the same command names (`git add a.rs && git commit -m x`), as they are in the work tree.
- **`git push`**: the lines each commit not on any remote adds.
- **Deny or warn.** It denies, naming each finding; when only gitleaks' generic rule matched, it warns instead, since that rule is the one most often wrong.
- **Large diffs.** A diff over 1 MiB is not scanned, and the rule stays quiet.

`jouet commit` is not checked by the rule, since it scans for itself.

## Speed

Measured on this repository with a release build (`date +%s%N` around each run, the median of three on a Linux workstation, the process start and git included):

| Scan | Size | Time |
|---|---|---|
| `--diff HEAD~1` | 8 files, 132 added lines | 10 ms |
| `--diff HEAD~5` | 134 files, 12,043 added lines | 33 ms |
| the whole work tree | 801 files | 100 ms |
| `--diff` of the whole history | 72 commits, 836 files | 197 ms |

In the hook, a whole `jouet hook claude pre-tool-use` call for `git commit` with a staged finding took 13 ms (median of five), against 9 ms for `git status`, which the rule does not scan. An ordinary commit is well inside the hooks' 200 ms probe budget, and the 1 MiB cap keeps a large one from running past it.

## Routines

`jouet.secrets.scan{paths, staged, diff}` runs it from a [routine](/routines), with the `git.read` capability.

## JSON

```json
{
  "source": "staged",
  "findings": [
    {
      "rule": "slack-bot-token",
      "description": "Identified a Slack Bot token, which may compromise bot integrations and communication channel security.",
      "path": "config/app.toml",
      "line": 2,
      "redacted": "xoxb****",
      "fingerprint": "config/app.toml:slack-bot-token:2"
    }
  ],
  "files_scanned": 1,
  "rules": 222,
  "config": "gitleaks v8.30.1"
}
```

`source` is `files`, `staged` or `commits`. A commit's finding has its `commit`; a rule that matches a file's name has no `line`. A scan of commits has `commits_scanned`. `binary_files` counts the files checked by name only, `ignored` the findings `.gitleaksignore` or `--allow-secret` passed over, and `skipped` lists the files not read, each with its `path` and `reason`. `config` says where the rules came from, `gitleaks v8.30.1 with .gitleaks.toml` when the repository has one.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Nothing found |
| 1 | A secret was found |
| 2 | Usage: `--staged` and `--diff` together |
| 3 | A path or `REF` does not exist |
| 4 | `--staged` or `--diff` outside a git repository, or a `.gitleaks.toml` that does not parse |
