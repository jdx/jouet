//! What agents are told about jouet: `jouet agent-help`, the skill and the
//! session-start primer. All of it is generated from the CLI definition, so
//! it describes the installed binary and cannot go stale.

use crate::Jouet;
use jouet_output::exit::Exit;
use std::fmt::Write as _;
use usage_rs::spec::CommandMeta;

/// When an agent should reach for each command. Every visible command has a
/// line (a test checks), so a new command lands with one; `None` marks a
/// command that is for people setting jouet up, not for agents.
const WHEN: &[(&str, Option<&str>)] = &[
    (
        "search",
        Some(
            "searching code: like rg, but grouped by the function each match is in, cut to a budget with markers for what it left out, and collapsing matches this session already saw",
        ),
    ),
    (
        "read",
        Some(
            "reading a long file (400 lines or more): `jouet read FILE --outline`, then `--symbol NAME` or `--range A:B`, instead of reading it whole to find one thing. Read a shorter file whole, in one call; `--outline` prints a file of up to 100 lines whole anyway. For a repository new to you, `jouet read --map` prints its most important definitions to fit a token budget",
        ),
    ),
    (
        "docs",
        Some(
            "using a dependency's API: `jouet docs PKG` outlines it and `jouet docs PKG SYMBOL` prints one definition's signature, doc and source, at the version the project's lockfile names (Rust, TypeScript, Python, Go), instead of guessing from memory or reading the cargo registry, node_modules or site-packages by hand",
        ),
    ),
    (
        "deps",
        Some(
            "adding or upgrading a dependency: `jouet deps info cargo:NAME npm:NAME pypi:NAME go:MODULE` (or a bare NAME the project's lockfile places) says whether each exists (with close names when not), its latest version, licence, deprecation and OSV advisories, before you write it into a manifest from memory; `jouet deps audit` checks every locked version against OSV",
        ),
    ),
    (
        "db",
        Some(
            "looking at a SQLite or Postgres database: `jouet db schema [TABLE]` lists its tables with columns, keys and indexes, and `jouet db query SQL` runs one read-only query, with rows cut to --limit and a count of the rest, instead of `sqlite3` or `psql` printing whole tables. The database is --url, DATABASE_URL, or DATABASE_URL in the project's fnox config; a write is refused (exit 4) even with write rights, and a host other than this machine needs --remote",
        ),
    ),
    (
        "edit",
        Some(
            "changing files: exact replacements (`jouet edit replace <PATHS>... --old TEXT --new TEXT`, --old and --new repeatable, or a spec on stdin), whole definitions by name (`=== PATH#SYMBOL`), or every match of a code pattern (`--pattern --rewrite --lang`), that all land or none do, instead of python/sed rewrites or a string of separate edits",
        ),
    ),
    (
        "diag",
        Some(
            "after an edit, in a project with an hk config (hk.pkl): the errors and warnings in just the files you changed (`diag check --changed`), or only the ones your edit introduced (`diag delta`, same arguments), from the project's hk checks, instead of a full `mise run lint` or `cargo clippy | head`; `diag fix` formats them. Without hk.pkl it exits 4 and checks nothing",
        ),
    ),
    (
        "wait",
        Some(
            "waiting for CI, a PR or a workflow run, or for a file, log line, port, URL, process, command, `jouet job` or pitchfork daemon, instead of a sleep loop",
        ),
    ),
    (
        "http",
        Some(
            "calling an HTTP API or checking an endpoint: `jouet http GET URL --select '$.items[*].id'` prints the status and only those JSONPath values, `--expect 'jsonpath $.ok == true'` exits 1 when a value is wrong, and a long body is cut to --budget, instead of curl piped into jq or python. Put credentials in `${NAME}` (from the environment, or `fnox exec -- jouet http ...`), never inline",
        ),
    ),
    (
        "job",
        Some(
            "running a build, a test suite, a long script or a server for one check in the background: `jouet job start ID --port N -- CMD` returns once it is ready, `jouet wait job ID` blocks until it ends, instead of `&`, nohup, sleep-and-curl loops and pkill",
        ),
    ),
    (
        "ci",
        Some(
            "reproducing a GitHub Actions job on this machine before pushing, or one `wait ci` reported failing (`ci run --check NAME`, needs jactionlint), instead of pushing again to see; and before rerunning or \"fixing\" a CI failure: `ci explain [PR] --run ID` says whether each failed job is flaky, infrastructure or real, with the evidence, and `--rerun` reruns only a flake or an infrastructure failure, instead of `gh run rerun`",
        ),
    ),
    (
        "pr",
        Some(
            "checking many PRs at once (`pr status`), finding the one thing a PR needs next (`pr next`) and merging it once ready (`pr land`), or reading, replying to and resolving review threads and Entire findings (`pr threads`), instead of gh loops, raw GraphQL and `entire trail finding`",
        ),
    ),
    (
        "commit",
        Some(
            "committing: only the paths you name, with the configured trailers and hooks run, instead of `git add -A && git commit`",
        ),
    ),
    (
        "secrets",
        Some(
            "checking for API keys, tokens and private keys before they are committed or pushed: `jouet secrets scan` (files), `--staged` or `--diff REF` (the lines being added), with gitleaks' rules; `jouet commit` runs the same scan, and `--allow-secret RULE:PATH:LINE` lets a false positive through",
        ),
    ),
    (
        "review-package",
        Some(
            "handing a range of commits to a reviewer: one file with the commits, a diffstat and the full diff, refusing a range that would show the wrong diff",
        ),
    ),
    (
        "handoff",
        Some(
            "ending a session with work in progress: records the branch, uncommitted files, the PR, the last test runs, background processes and jobs for the next session",
        ),
    ),
    (
        "pickup",
        Some(
            "starting a session that continues earlier work: prints the last handoff and checks that it still holds; `jouet sessions` shows which other agent sessions are working where on this machine, and warns when two share a worktree",
        ),
    ),
    (
        "stack",
        Some(
            "working on a stack of PRs: `jouet stack status` shows each layer's PR and whether it really contains the layer below, by ancestry and by content (a restack that reverted a parent's change is flagged), and `jouet stack sync` merges each layer up to date from its local branch and verifies it, instead of trusting `gh stack view` or restacking with rebase, `checkout -B` or force pushes",
        ),
    ),
    (
        "test",
        Some(
            "running tests or a build: `jouet test --affected` runs only the tests your changes affect (vitest, jest, pytest, unittest, cargo by crate, and the code graph for Go, Ruby and Java) and lists why each was picked; `jouet test` runs the full suite, when a full run is needed (a mise, npm, just or make `test` task, cargo, go, vitest, jest, pytest or `python -m unittest`). `jouet test --cmd -- CMD` runs any other test, e2e or build command; add `--baseline` to compare a failing command with the merge base. Each prints one line when it passes, or each failure with its error within --budget bytes, and saves the whole output to a log it names: read that instead of piping the run through head, tail or grep, or running it again",
        ),
    ),
    (
        "rule",
        Some(
            "writing or checking a jouet rule, the TOML matchers behind jouet's hooks (`jouet rule schema`): write its `fires` and `quiet` examples first, and `jouet rule test` runs them before it is enabled",
        ),
    ),
    (
        "run",
        Some(
            "a job you would otherwise do step by step, written as a routine (code, where rules are TOML configuration): `joue land [PR]` (pr-next, wait for CI, then pr land), `joue sweep` (unresolved threads as a work list), `joue ship-check` (the tests, then the PR); `joue --list` lists them, `joue NAME --help` gives the args, and a routine that writes needs --yes (without it: a preview and exit 4)",
        ),
    ),
    (
        "wind",
        Some(
            "a winding note at session start names a drafted rule, a proposal or a stale memory: `jouet wind show ID` reads it; or a session is worth reviewing: `jouet wind review SESSION` prints a prompt and a digest for a subagent; `jouet unwind` proposes retiring a rule of yours that is noisy or keeps getting overridden",
        ),
    ),
    (
        "claim",
        Some(
            "working where other agent sessions on this machine also work: `jouet claim PATH... --note TEXT` tells them which files are yours, jouet's hooks warn when you edit or commit a path another session claims, and `jouet claim ls` lists who claims what",
        ),
    ),
    // Not in the overview: it is at its 40-line budget. `jouet agent-help
    // graph` describes it, and `jouet read` and `jouet edit` use it.
    ("graph", None),
    // `pickup`'s line names it.
    ("sessions", None),
    // `wind`'s line names it.
    ("unwind", None),
    // Named where it is needed: in a winding note at session start, which
    // says what to ask the user.
    ("task", None),
    ("share", None),
    ("agent-help", None),
    ("audit", None),
    ("skill", None),
    ("setup", None),
    // A person's decision, not an agent's: what a repository or a plugin
    // may run. The `trust` rule denies the first and `plugin add` to agents.
    ("trust", None),
    ("untrust", None),
    ("plugin", None),
    ("stats", None),
    // The session-start primer names the slow checks it lists.
    ("cost", None),
    ("completion", None),
];

/// What `jouet agent-help COMMAND` prints by default: the invocations agents
/// use, each with what it does, and what `--full` adds. Reading the whole
/// guide before every first use cost agents up to 7 kB a command. Every
/// command with a `WHEN` line has one (a test checks), and every flag a line
/// names must exist on the command it names.
struct Usage {
    command: &'static str,
    lines: &'static [(&'static str, &'static str)],
    /// What `--full` adds, so the agent knows when it needs it.
    more: &'static str,
}

const USAGE: &[Usage] = &[
    Usage {
        command: "search",
        lines: &[
            (
                "jouet search <PATTERN> [<PATHS>...] [--glob GLOB]",
                "regex search, matches grouped by the function they are in",
            ),
            (
                "jouet search <PATTERN> --fixed-strings --ignore-case",
                "literal text, any case",
            ),
            (
                "jouet search <PATTERN> --files-with-matches",
                "only the files, with their match counts",
            ),
            (
                "jouet search --structural --lang LANG '<PAT>' [<PATHS>...]",
                "a code pattern (ast-grep), such as 'foo($$$ARGS)'",
            ),
            (
                "jouet search <PATTERN> --again",
                "also the matches this session was already shown",
            ),
        ],
        more: "the byte budgets, --callers-of and --impact from the code graph, hidden files",
    },
    Usage {
        command: "read",
        lines: &[
            (
                "jouet read <PATH> --outline",
                "a long file's definitions with their line ranges",
            ),
            (
                "jouet read <PATH> --symbol NAME",
                "one definition: NAME, KIND NAME or Scope::NAME",
            ),
            ("jouet read <PATH> --range A:B", "lines A to B"),
            (
                "jouet read --map [--budget TOKENS]",
                "a new repository's most important definitions",
            ),
        ],
        more: "--max-lines, --again, --focus and how --map ranks definitions",
    },
    Usage {
        command: "docs",
        lines: &[
            (
                "jouet docs <PACKAGE>",
                "outline a dependency's API at the version the lockfile names",
            ),
            (
                "jouet docs <PACKAGE> <SYMBOL>",
                "one definition's signature, doc and source",
            ),
        ],
        more: "--lang, --private, --budget and how the version is found",
    },
    Usage {
        command: "deps",
        lines: &[
            (
                "jouet deps info <PACKAGES>...",
                "cargo:NAME, npm:NAME, pypi:NAME, go:MODULE or a bare NAME: whether it exists, its latest version, licence and advisories",
            ),
            ("jouet deps audit", "every locked version against OSV"),
        ],
        more: "--ecosystem, --refresh and what each check reads",
    },
    Usage {
        command: "db",
        lines: &[
            (
                "jouet db schema [TABLE]",
                "tables with columns, keys and indexes; one table with its row count",
            ),
            (
                "jouet db query <SQL> --limit 20",
                "one read-only statement, rows cut to the limit with a count of the rest",
            ),
            (
                "jouet db query <SQL> --url app.db",
                "a SQLite file or a Postgres URL, else DATABASE_URL or fnox's",
            ),
        ],
        more: "--width, --budget, --timeout, --remote and how reads are enforced",
    },
    Usage {
        command: "edit",
        lines: &[
            (
                "jouet edit replace <PATHS>... --old TEXT --new TEXT",
                "exactly one match in each file; repeat --old/--new pairs, applied in order, all or nothing",
            ),
            (
                "jouet edit replace <PATHS>... --old TEXT --new TEXT --all",
                "every match, at least one in each file (--count N for an exact count)",
            ),
            (
                "jouet edit [--spec FILE]",
                "search/replace blocks, a patch or JSON from stdin; `=== PATH#SYMBOL` replaces a whole definition",
            ),
            (
                "jouet edit --pattern '<PAT>' --rewrite '<REPL>' --lang LANG <PATHS>... --dry-run",
                "every match of a code pattern (ast-grep), previewed",
            ),
            (
                "jouet edit ... --check CMD --revert-on-fail",
                "run CMD after writing, and restore every file if it fails",
            ),
        ],
        more: "spec formats, symbol edits, pattern syntax, --callers-of, --impact, --diag, --expect-hash",
    },
    Usage {
        command: "diag",
        lines: &[
            (
                "jouet diag check --changed",
                "hk's errors and warnings in just the files you changed",
            ),
            (
                "jouet diag delta --changed",
                "only the ones your edit introduced",
            ),
            ("jouet diag fix --changed", "run hk's fixes on them"),
        ],
        more: "--max, --step, --dry-run; without hk.pkl it exits 4",
    },
    Usage {
        command: "wait",
        lines: &[
            (
                "jouet wait ci [<PR>] [--all]",
                "a PR's (or the branch's) checks; failing logs are shown; --all waits for every check after a failure",
            ),
            (
                "jouet wait pr [<PR>] --until CONDITION",
                "mergeable, merged, approved, ready, ...",
            ),
            (
                "jouet wait http <URL> | port <ADDR> | file <PATH> | log <PATH> --match REGEX",
                "a server, a file or a log line",
            ),
            (
                "jouet wait job <ID>",
                "a `jouet job` to end (--until ready for ready)",
            ),
            (
                "jouet wait --timeout DUR ...",
                "give up after DUR (default 30m): exit 124, run it again to keep waiting",
            ),
        ],
        more: "every condition and flag: run, pid, cmd, daemon, --required, --check, --log-lines",
    },
    Usage {
        command: "http",
        lines: &[
            (
                "jouet http GET <URL> --select '$.items[*].id'",
                "the status and only the values at those JSONPaths (repeatable)",
            ),
            (
                "jouet http GET <URL> --expect status=200 --expect 'jsonpath $.ok == true'",
                "hurl-style checks; exit 1 names the one that failed",
            ),
            (
                "jouet http POST <URL> --json-body @FILE",
                "send JSON (or --data TEXT|@FILE|@-); --header NAME:VALUE",
            ),
            (
                "jouet http GET <URL> --header 'Authorization: Bearer ${TOKEN}'",
                "credentials by name from the environment (or `fnox exec -- jouet http ...`), redacted from output",
            ),
        ],
        more: "every predicate, --budget, --headers, --no-follow, --timeout and what is redacted",
    },
    Usage {
        command: "job",
        lines: &[
            (
                "jouet job start <ID> --port N -- CMD...",
                "run CMD in the background; returns once it is ready (also --http URL, --output REGEX)",
            ),
            ("jouet wait job <ID>", "block until it ends"),
            ("jouet job logs <ID>", "its log"),
            (
                "jouet job ls / jouet job stop <IDS>...",
                "list or stop jobs",
            ),
        ],
        more: "--timeout, --replace, --lines, --grace and how readiness is judged",
    },
    Usage {
        command: "ci",
        lines: &[
            (
                "jouet ci explain [<TARGET>] [--run ID]",
                "whether each failed job is flaky, infrastructure or real, with the evidence",
            ),
            (
                "jouet ci explain [<TARGET>] --rerun",
                "rerun only when every failure is a flake or infrastructure",
            ),
            (
                "jouet ci run [<WORKFLOW>] --check NAME",
                "run the job behind a check here, with jactionlint",
            ),
        ],
        more: "--job, --event, --secrets-from, --log-lines and how a failure is classified",
    },
    Usage {
        command: "pr",
        lines: &[
            (
                "jouet pr threads [<PR>] [--context N] [--full]",
                "the unresolved review threads and open Entire findings, with N lines of code around each thread",
            ),
            (
                "jouet pr threads reply <ID> [<PR>] --body TEXT --resolve",
                "reply to a thread and resolve it",
            ),
            (
                "jouet pr status [<PRS>...] [--mine]",
                "where PRs stand and why any need attention",
            ),
            ("jouet pr next [<PR>]", "the one thing a PR needs next"),
            (
                "jouet pr land [<PR>] --yes",
                "merge it once ready (--dry-run first)",
            ),
        ],
        more: "thread filters, resolve, apply, --method, --settle",
    },
    Usage {
        command: "commit",
        lines: &[
            (
                "jouet commit <PATHS>... --message MSG",
                "commit exactly these paths, with the configured trailers; hooks run",
            ),
            (
                "jouet commit <PATHS>... --dry-run",
                "show what would be committed",
            ),
        ],
        more: "--file, --trailer, --no-trailers, --allow-secret and what it refuses",
    },
    Usage {
        command: "secrets",
        lines: &[
            (
                "jouet secrets scan [<PATHS>...]",
                "API keys, tokens and private keys in files, with gitleaks' rules",
            ),
            (
                "jouet secrets scan --staged",
                "only the lines the index adds: what `git commit` would record",
            ),
            (
                "jouet secrets scan --diff REF",
                "the lines each commit in REF..HEAD adds",
            ),
        ],
        more: "false positives (gitleaks:allow, .gitleaksignore, .gitleaks.toml) and the --json fingerprints",
    },
    Usage {
        command: "review-package",
        lines: &[(
            "jouet review-package <BASE> [<HEAD>] [--output FILE]",
            "the commits, a diffstat and the full diff in one file",
        )],
        more: "--context and the ranges it refuses",
    },
    Usage {
        command: "handoff",
        lines: &[(
            "jouet handoff [--output FILE]",
            "record the branch, uncommitted files, PR, test runs and jobs for the next session",
        )],
        more: "--no-pr, --session and what it records",
    },
    Usage {
        command: "pickup",
        lines: &[(
            "jouet pickup [<FILE>]",
            "print the last handoff and check it still holds",
        )],
        more: "what it checks",
    },
    Usage {
        command: "stack",
        lines: &[
            (
                "jouet stack status",
                "each layer's PR and whether it really contains the layer below",
            ),
            (
                "jouet stack sync --dry-run",
                "what merging each layer up to date would do",
            ),
            (
                "jouet stack sync [--push]",
                "merge each layer up to date and verify it",
            ),
        ],
        more: "--no-fetch and how a layer is checked",
    },
    Usage {
        command: "test",
        lines: &[
            (
                "jouet test",
                "the full suite (the project's test task, or its runner)",
            ),
            (
                "jouet test --affected",
                "only the tests your changes affect, with why each was picked",
            ),
            (
                "jouet test --cmd -- CMD...",
                "any other test, e2e or build command",
            ),
            (
                "jouet test ... --budget BYTES",
                "failure text to show (default 4000); the full log's path is printed",
            ),
        ],
        more: "--since, --adapter, --dry-run, the runners it finds and how failures are cut",
    },
    Usage {
        command: "rule",
        lines: &[
            ("jouet rule list", "every rule and whether it runs"),
            (
                "jouet rule test [<RULE>] [--command CMD]",
                "run a rule's examples, or a rule against a command",
            ),
            (
                "jouet rule new <NAME> [--from-note ID]",
                "write a rule of yours, in warn mode",
            ),
            ("jouet rule schema", "the rule file's JSON schema"),
        ],
        more: "promote, log, gap, types and the payload and replay options",
    },
    Usage {
        command: "run",
        lines: &[
            ("joue --list", "every routine and what it reaches"),
            ("joue NAME --help", "a routine's arguments"),
            (
                "joue land [PR] / joue sweep / joue ship-check",
                "the built-in routines; one that writes needs --yes (without: a preview and exit 4)",
            ),
        ],
        more: "run test and run types",
    },
    Usage {
        command: "wind",
        lines: &[
            ("jouet wind show <ID>", "read a note or proposal"),
            ("jouet wind dismiss <ID>", "drop it"),
            (
                "jouet wind review [<SESSION>] [--last]",
                "a prompt and digest for a subagent to review a session",
            ),
        ],
        more: "adopt, digest, propose, memories and their filters",
    },
    Usage {
        command: "claim",
        lines: &[
            (
                "jouet claim <PATHS>... --note TEXT",
                "tell other sessions these files are yours (for 2h; --for DUR)",
            ),
            ("jouet claim ls", "who claims what"),
            ("jouet claim release [<PATHS>...]", "give them back"),
        ],
        more: "how the hooks use claims",
    },
    Usage {
        command: "graph",
        lines: &[
            (
                "jouet graph index",
                "build the graph, or bring it up to date",
            ),
            (
                "jouet graph callers <SYMBOL> | callees <SYMBOL> | impact <SYMBOL>",
                "what calls it, what it calls, what a change could break",
            ),
            (
                "jouet graph explore <QUERY>...",
                "the code that answers a question",
            ),
            (
                "jouet graph affected [<FILES>...]",
                "the test files that depend on changed files",
            ),
        ],
        more: "symbols, status, prune, --depth, budgets and pages",
    },
];

fn usage_of(name: &str) -> Option<&'static Usage> {
    USAGE.iter().find(|u| u.command == name)
}

/// Subcommands an agent is told about where it needs them, in a line jouet
/// prints (a proposal's note, the review prompt), so the overview leaves
/// them out. `jouet agent-help COMMAND` still lists them.
const PROMPTED: &[(&str, &str)] = &[
    ("wind", "adopt"),
    ("wind", "propose"),
    ("wind", "dismiss"),
    // `diag`'s when line names them, and they take check's arguments.
    ("diag", "delta"),
    ("diag", "fix"),
];

/// Tools jouet works alongside rather than reimplementing, mentioned when
/// they are installed.
const COMPANIONS: &[(&str, &str)] = &[
    (
        "mbx",
        "mbx: Rust builds. Run cargo as usual (or prefix it with `mbx`); compiled output is shared across checkouts and builds are scheduled machine-wide. `mbx analyze` explains a slow rebuild",
    ),
    (
        "jactionlint",
        "jactionlint: lints GitHub workflows; `jouet ci run` runs their jobs here through it",
    ),
    (
        "rtk",
        "rtk: shrinks the output of common commands; see `rtk --help`",
    ),
    (
        "bd",
        "bd (beads): issue tracking for agents; `bd ready` lists unblocked work",
    ),
    (
        "fnox",
        "fnox: a project's secrets, encrypted or in a vault. `fnox exec -- CMD` runs a command with them in its environment and `fnox list` shows their names; never print a value (`fnox get`, `env`) into the transcript",
    ),
    (
        "pitchfork",
        "pitchfork: runs a project's long-running services, the daemons in pitchfork.toml. `pitchfork start ID` starts one and returns once it is ready, `pitchfork logs ID -n 50` reads its log, `pitchfork restart ID` and `pitchfork stop ID` replace killing it, and `jouet wait daemon ID` waits for it; one-off background work is `jouet job`'s",
    ),
];

fn root() -> &'static CommandMeta<'static> {
    Jouet::spec().root
}

fn find<'a>(meta: &'a CommandMeta<'a>, name: &str) -> Option<&'a CommandMeta<'a>> {
    meta.subcommands
        .iter()
        .copied()
        .find(|c| c.cmd.name == name || c.cmd.aliases.contains(&name))
}

fn lookup<S: AsRef<str>>(path: &[S]) -> Option<&'static CommandMeta<'static>> {
    let mut meta = root();
    for name in path {
        meta = find(meta, name.as_ref())?;
    }
    Some(meta)
}

/// The commands an agent should know about, with when to use each.
pub(crate) fn agent_commands() -> Vec<(&'static CommandMeta<'static>, &'static str)> {
    root()
        .subcommands
        .iter()
        .filter(|c| !c.hide)
        .filter_map(|c| when(c.cmd.name).map(|w| (*c, w)))
        .collect()
}

fn when(name: &str) -> Option<&'static str> {
    WHEN.iter().find(|(n, _)| *n == name).and_then(|(_, w)| *w)
}

/// Commands that work together, which the overview and the skill describe
/// in one entry, led by the first, to stay within their budgets.
pub(crate) const GROUPED: &[&[&str]] = &[
    &["commit", "secrets"],
    &["handoff", "pickup"],
    &["read", "docs", "deps", "db"],
    &["wait", "http"],
];

/// The group `name` belongs to, if any.
pub(crate) fn group(name: &str) -> Option<&'static [&'static str]> {
    GROUPED.iter().copied().find(|g| g.contains(&name))
}

/// The guide for the whole tool: one entry per command family, with its
/// subcommands by name and when to reach for it. Flags and synopses live in
/// `jouet agent-help COMMAND`, so a new subcommand costs the overview a word,
/// not a line.
pub(crate) fn overview() -> String {
    let mut out = format!(
        "jouet {}: a toolkit CLI for coding agents. Reach for it before hand-rolling a script.\n\n",
        env!("CARGO_PKG_VERSION")
    );
    let commands = agent_commands();
    for (meta, when) in &commands {
        let name = meta.cmd.name;
        let members = group(name).unwrap_or(&[]);
        if members.first().is_some_and(|first| *first != name) {
            continue;
        }
        let others: Vec<_> = members
            .iter()
            .skip(1)
            .filter_map(|other| commands.iter().find(|(m, _)| m.cmd.name == *other))
            .collect();
        if !others.is_empty() {
            let mut names = head(name, meta);
            let mut abouts = meta.about.unwrap_or("").to_string();
            let mut whens = (*when).to_string();
            for (other, other_when) in others {
                let _ = write!(names, " / jouet {}", other.cmd.name);
                let _ = write!(abouts, " / {}", lowercase_first(other.about.unwrap_or("")));
                let _ = write!(whens, "; {other_when}");
            }
            let _ = writeln!(out, "{names}: {abouts}");
            let _ = writeln!(out, "  when: {whens}");
            continue;
        }
        let _ = writeln!(out, "{}: {}", head(name, meta), meta.about.unwrap_or(""));
        let _ = writeln!(out, "  when: {when}");
    }
    out.push('\n');
    out.push_str(&conventions());
    for line in companions() {
        let _ = writeln!(out, "Also installed: {line}.");
    }
    out.push_str(
        "Details: `jouet agent-help COMMAND` for its subcommands and flags, `jouet COMMAND --help`.\n",
    );
    out
}

/// `jouet NAME` and its subcommands by name, as the overview lists it.
fn head(name: &str, meta: &CommandMeta<'_>) -> String {
    let subs: Vec<&str> = meta
        .subcommands
        .iter()
        .filter(|s| {
            let at = (name, s.cmd.name);
            !s.hide && !PROMPTED.contains(&at)
        })
        .map(|s| s.cmd.name)
        .collect();
    if subs.is_empty() {
        format!("jouet {name}")
    } else if meta.subcommand_required() {
        format!("jouet {name} <{}>", subs.join("|"))
    } else {
        // The family also runs on its own.
        format!("jouet {name} [{}]", subs.join("|"))
    }
}

/// The guide for one command: what `jouet agent-help COMMAND` prints. A
/// command family with a usage block gets just that, ending with what
/// `--full` adds; anything else, a subcommand such as `pr threads` included,
/// gets the whole guide.
pub(crate) fn command<S: AsRef<str>>(path: &[S]) -> Option<String> {
    let usage = match path {
        [name] => usage_of(name.as_ref()),
        _ => None,
    };
    let Some(usage) = usage else {
        return full(path);
    };
    let meta = lookup(path)?;
    let mut out = heading(usage.command, meta);
    usage_block(&mut out, usage);
    let _ = writeln!(
        out,
        "more: `jouet agent-help {} --full` ({})",
        usage.command, usage.more
    );
    let codes: Vec<String> = Exit::ALL
        .iter()
        .map(|e| format!("{} {}", e.code(), e.name()))
        .collect();
    let _ = writeln!(out, "Exit codes: {}.", codes.join(", "));
    Some(out)
}

/// The whole guide for a command: its usage block, if it has one, then every
/// subcommand, nested ones included, with all their flags and details.
pub(crate) fn full<S: AsRef<str>>(path: &[S]) -> Option<String> {
    let meta = lookup(path)?;
    let words: Vec<&str> = path.iter().map(AsRef::as_ref).collect();
    let name = format!("jouet {}", words.join(" "));
    let mut out = heading(&words.join(" "), meta);
    if let [only] = words.as_slice()
        && let Some(usage) = usage_of(only)
    {
        usage_block(&mut out, usage);
        out.push_str("\n--- details ---\n");
    }
    describe(&mut out, &name, meta);
    subcommands(&mut out, &name, meta);
    out.push('\n');
    out.push_str(&conventions());
    Some(out)
}

fn heading(words: &str, meta: &CommandMeta<'_>) -> String {
    let mut out = format!("jouet {words}: {}\n", meta.about.unwrap_or(""));
    if let Some(when) = words.split(' ').next().and_then(when) {
        let _ = writeln!(out, "when: {when}");
    }
    out
}

fn usage_block(out: &mut String, usage: &Usage) {
    out.push_str("usage:\n");
    for (line, what) in usage.lines {
        let _ = writeln!(out, "  {line}  # {what}");
    }
}

fn subcommands(out: &mut String, name: &str, meta: &CommandMeta<'_>) {
    for sub in meta.subcommands.iter().filter(|s| !s.hide) {
        let name = format!("{name} {}", sub.cmd.name);
        out.push('\n');
        let _ = writeln!(out, "{name}: {}", sub.about.unwrap_or(""));
        describe(out, &name, sub);
        subcommands(out, &name, sub);
    }
}

fn describe(out: &mut String, name: &str, meta: &CommandMeta<'_>) {
    if let Some(long) = meta.long_about {
        for line in long.lines().skip_while(|l| !l.is_empty()).skip(1) {
            let _ = writeln!(out, "  {line}");
        }
    }
    let _ = writeln!(out, "  {}", synopsis(name, meta));
    for flag in visible_flags(meta) {
        let _ = writeln!(out, "    {}  {}", flag_usage(flag), flag.help.unwrap_or(""));
    }
}

/// The commands the primer gives a usage line for, with the flags it shows:
/// the ones agents reach for in almost every task, so the first use costs
/// no `--help`.
const PRIMED: &[(&[&str], &[&str])] = &[
    (&["commit"], &["--message"]),
    (&["edit", "replace"], &["--old", "--new"]),
    (&["pr", "threads", "reply"], &["--body", "--resolve"]),
];

/// The short primer printed when a session starts: the command families by
/// name, usage for the few used in almost every task, and the deny rules
/// with what each suggests, so the agent neither looks those up nor pays a
/// denied call to learn them. When to use each command and its flags are
/// one `jouet agent-help COMMAND` away; listing them here as well made every
/// session pay for them.
/// `affected` is whether `jouet test --affected` can select tests where the
/// session starts: without it, the primer leaves the flag out rather than
/// send the agent to a command that exits 4. `wait` is how the harness waits
/// for a long command in one call (see [`wait_hint`]).
pub(crate) fn summary(
    rules: &[(&str, Option<&str>)],
    verify: bool,
    affected: bool,
    wait: Option<&str>,
) -> String {
    let commands = agent_commands();
    let present = |name: &str| commands.iter().any(|(m, _)| m.cmd.name == name);
    // A group is one entry, as in the overview: `read/docs/deps/db`.
    let names: Vec<String> = commands
        .iter()
        .map(|(m, _)| m.cmd.name)
        .filter(|name| group(name).is_none_or(|g| g[0] == *name))
        .map(|name| match group(name) {
            Some(members) => members
                .iter()
                .filter(|m| present(m))
                .copied()
                .collect::<Vec<_>>()
                .join("/"),
            None => name.to_string(),
        })
        .collect();
    let mut out = format!(
        "jouet is installed, a CLI toolkit for coding agents: {}.\n",
        names.join(", ")
    );
    let usage: Vec<String> = PRIMED
        .iter()
        .filter_map(|(path, flags)| short_synopsis(path, flags))
        .map(|s| format!("`{s}`"))
        .collect();
    // Codex read `agent-help test` and `agent-help commit` before nearly
    // every first use, so these say they need no lookup.
    let tests: Vec<&str> = usage_of("test")
        .map(|u| u.lines.iter().take(3).map(|(line, _)| *line).collect())
        .unwrap_or_default();
    let wait = wait.map_or(String::new(), |w| format!("{w} "));
    if let ([commit, edit, reply], [_, selected, cmd]) = (usage.as_slice(), tests.as_slice()) {
        let test = if affected {
            format!(
                "Test with `{selected}` (what your changes affect) or `{cmd}` (any other test or build)."
            )
        } else {
            format!("Test with `{cmd}` (the tests or build for what you changed).")
        };
        let _ = writeln!(
            out,
            "{test} {wait}Commit only the paths you name with {commit}, in the same call as their test run (`jouet test ... && jouet commit ...`). Edit with your own edit tool or {edit}, which takes several files and repeated --old/--new pairs at once; never a Python script. List a PR's open review threads with `jouet pr threads [PR]`; reply to and resolve one with {reply}. These need no lookup; for other commands, `jouet agent-help COMMAND` gives their usage."
        );
    }
    if !rules.is_empty() {
        // Rules that suggest the same command share it.
        let mut named: Vec<String> = Vec::new();
        for (i, (name, suggest)) in rules.iter().enumerate() {
            let last = rules.get(i + 1).is_none_or(|(_, next)| next != suggest);
            named.push(match suggest {
                Some(s) if last => format!("{name} (use `{s}`)"),
                _ => (*name).to_string(),
            });
        }
        let _ = writeln!(
            out,
            "jouet's hooks deny these and say what to do instead: {}.",
            named.join(", ")
        );
    }
    if verify {
        out.push_str(
            "They also stop you finishing while a test, lint or build run failed, or when no test passed since your last change. For an unrelated failure, compare its command with `jouet test --baseline --cmd -- CMD`.\n",
        );
    }
    out
}

/// A command's positional arguments and the given flags, or `None` when it
/// lacks one of them.
pub(crate) fn short_synopsis(path: &[&str], flags: &[&str]) -> Option<String> {
    let meta = lookup(path)?;
    let mut parts = vec![format!("jouet {}", path.join(" "))];
    for arg in meta.args {
        let value = arg.value_names.first().copied().unwrap_or(arg.arg.name);
        parts.push(if arg.arg.var {
            format!("<{value}>...")
        } else if arg.arg.required {
            format!("<{value}>")
        } else {
            format!("[{value}]")
        });
    }
    for long in flags {
        let flag =
            visible_flags(meta).find(|f| f.flag.longs.iter().any(|l| format!("--{l}") == *long))?;
        parts.push(flag_usage(flag));
    }
    Some(parts.join(" "))
}

fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn conventions() -> String {
    let codes: Vec<String> = Exit::ALL
        .iter()
        .map(|e| format!("{} {} ({})", e.code(), e.name(), e.meaning()))
        .collect();
    format!(
        "Output: the result on stdout, diagnostics on stderr. --json prints one JSON document, errors included. Lists are never cut short without saying so.\nExit codes: {}.\n",
        codes.join(", ")
    )
}

fn companions() -> Vec<&'static str> {
    COMPANIONS
        .iter()
        .filter(|(bin, _)| on_path(bin))
        .map(|(_, line)| *line)
        .collect()
}

pub(crate) fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let names: &[String] = &if cfg!(windows) {
        vec![format!("{bin}.exe"), format!("{bin}.cmd")]
    } else {
        vec![bin.to_string()]
    };
    std::env::split_paths(&path).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
}

/// The root's own flags, which every command accepts and the guide states
/// once, in the conventions.
fn is_root_flag(flag: &usage_rs::spec::FlagMeta<'_>) -> bool {
    root().flags.iter().any(|r| r.flag.longs == flag.flag.longs)
}

fn visible_flags<'a>(
    meta: &'a CommandMeta<'a>,
) -> impl Iterator<Item = &'a usage_rs::spec::FlagMeta<'a>> {
    meta.flags
        .iter()
        .filter(|f| !f.builtin && !f.hide && !is_root_flag(f))
}

fn flag_usage(flag: &usage_rs::spec::FlagMeta<'_>) -> String {
    let name = flag.flag.longs.first().map_or_else(
        || {
            flag.flag
                .shorts
                .first()
                .map(|s| format!("-{}", *s as char))
                .unwrap_or_default()
        },
        |l| format!("--{l}"),
    );
    match flag.value_name {
        Some(value) if flag.flag.takes_value => format!("{name} {value}"),
        _ => name,
    }
}

fn synopsis(name: &str, meta: &CommandMeta<'_>) -> String {
    let mut parts = vec![name.to_string()];
    for arg in meta.args {
        let value = arg.value_names.first().copied().unwrap_or(arg.arg.name);
        let mut text = format!("<{value}>");
        if arg.arg.var {
            text.push_str("...");
        }
        if !arg.arg.required {
            text = format!("[{text}]");
        }
        parts.push(text);
    }
    for flag in visible_flags(meta) {
        let usage = flag_usage(flag);
        parts.push(if flag.required {
            usage
        } else {
            format!("[{usage}]")
        });
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_visible_command_says_when_to_use_it() {
        for meta in root().subcommands.iter().filter(|c| !c.hide) {
            assert!(
                WHEN.iter().any(|(n, _)| *n == meta.cmd.name),
                "add `{}` to WHEN in src/guide.rs",
                meta.cmd.name
            );
        }
        for (name, _) in WHEN {
            assert!(
                crate::spec::has_command(&[name]),
                "WHEN names `{name}`, which jouet does not have"
            );
        }
    }

    #[test]
    fn subcommands_left_out_of_the_overview_exist() {
        for (name, sub) in PROMPTED {
            assert!(crate::spec::has_command(&[name, sub]), "{name} {sub}");
            let guide = full(&[name]).unwrap();
            assert!(guide.contains(&format!("jouet {name} {sub}")), "{guide}");
            assert!(!overview().contains(&format!("jouet {name} {sub}")));
        }
    }

    #[test]
    fn the_overview_stays_short() {
        let text = overview();
        // The cap is a budget to defend, not a ceiling to fill. It counts
        // jouet's own lines: the "Also installed" lines depend on which
        // companion tools are on this machine's PATH, so counting them made
        // the test pass or fail by machine.
        let own = text
            .lines()
            .filter(|line| !line.starts_with("Also installed:"))
            .count();
        assert!(own <= 40, "{own} lines:\n{text}");
        assert!(
            text.contains("jouet edit replace <PATHS>... --old TEXT --new TEXT"),
            "{text}"
        );
        // Families list their subcommands by name, so a new one costs a word.
        assert!(text.contains("jouet pr <"), "{text}");
        assert!(text.contains("|land"), "{text}");
        assert!(text.contains("jouet edit [replace|hash]"), "{text}");
        assert!(text.contains("jouet wait <ci|"), "{text}");
        assert!(text.contains("4 precondition"), "{text}");
        assert!(!text.contains("jouet setup"), "{text}");
    }

    #[test]
    fn the_primer_leaves_out_affected_where_it_cannot_select() {
        // In a repository with no runner --affected knows, it exits 4.
        let text = summary(&[], true, false, None);
        assert!(!text.contains("--affected"), "{text}");
        assert!(
            text.contains(
                "Test with `jouet test --cmd -- CMD...` (the tests or build for what you changed)."
            ),
            "{text}"
        );
    }

    #[test]
    fn the_primer_is_short_and_needs_no_lookup_for_the_common_commands() {
        for (path, flags) in PRIMED {
            assert!(
                short_synopsis(path, flags).is_some(),
                "{path:?} lost one of {flags:?}: update PRIMED"
            );
        }
        let text = summary(
            &[
                ("python-edit", Some("jouet edit")),
                ("git-stash", None),
                ("print-secrets", Some("fnox exec")),
                ("secret-files", Some("fnox exec")),
            ],
            true,
            true,
            None,
        );
        assert!(
            text.contains("`jouet commit <PATHS>... --message MSG`"),
            "{text}"
        );
        assert!(
            text.contains("`jouet edit replace <PATHS>... --old TEXT --new TEXT`"),
            "{text}"
        );
        assert!(
            text.contains("`jouet pr threads reply <ID> [PR] --body TEXT --resolve`"),
            "{text}"
        );
        assert!(text.contains("`jouet test --cmd -- CMD...`"), "{text}");
        assert!(
            text.contains("compare its command with `jouet test --baseline --cmd -- CMD`"),
            "{text}"
        );
        assert!(text.contains("These need no lookup"), "{text}");
        // CI's coverage depends on the repository; the primer cannot assert it.
        assert!(!text.contains("CI runs the full suite"), "{text}");
        // Agents committed in a call of their own after the test run, and
        // guessed `pr threads list` before `pr threads`.
        assert!(
            text.contains("in the same call as their test run"),
            "{text}"
        );
        assert!(text.contains("`jouet pr threads [PR]`"), "{text}");
        // With the full suite first in this list, Codex ran it unasked on a
        // repository whose suite could not pass in its sandbox, then changed
        // the environment, outside its checkout, to make it pass.
        assert!(text.contains("Test with `jouet test --affected`"), "{text}");
        assert!(
            text.contains(
                "python-edit (use `jouet edit`), git-stash, print-secrets, secret-files (use `fnox exec`)."
            ),
            "{text}"
        );
        // The family names, not each one's description: that is agent-help's.
        assert!(text.contains("commit/secrets, "), "{text}");
        assert!(
            !text.contains("Commit only the named paths, with trailers"),
            "{text}"
        );
        assert!(text.lines().count() <= 4, "{text}");
        assert!(text.len() <= 1300, "{} bytes:\n{text}", text.len());
    }

    #[test]
    fn a_full_command_guide_lists_its_flags() {
        let text = full(&["edit"]).unwrap();
        assert!(text.contains("--expect-hash PATH=SHA256"), "{text}");
        assert!(text.contains("jouet edit hash"), "{text}");
        assert!(!text.contains("--json  "), "{text}");
        // The usage block leads, then the details after a break.
        let usage = text.find("usage:\n").unwrap();
        let details = text.find("--- details ---").unwrap();
        assert!(usage < details, "{text}");
        // Nested subcommands are in the whole guide too.
        let pr = full(&["pr"]).unwrap();
        assert!(pr.contains("jouet pr threads reply <ID>"), "{pr}");
        assert_eq!(command(&["nope"]), None);
        assert_eq!(full(&["nope"]), None);
    }

    #[test]
    fn every_agent_command_has_a_short_usage_block() {
        for (meta, _) in agent_commands() {
            let name = meta.cmd.name;
            assert!(
                usage_of(name).is_some(),
                "add `{name}` to USAGE in src/guide.rs"
            );
            let text = command(&[name]).unwrap();
            assert!(
                text.contains(&format!("more: `jouet agent-help {name} --full` (")),
                "{text}"
            );
            // The point of the short page: an agent reads it before a first
            // use, so it stays well under the whole guide.
            assert!(text.len() <= 1600, "{name}: {} bytes:\n{text}", text.len());
            assert!(text.len() < full(&[name]).unwrap().len(), "{name}");
        }
        for usage in USAGE {
            assert!(
                crate::spec::has_command(&[usage.command]),
                "USAGE names `{}`, which jouet does not have",
                usage.command
            );
        }
        // A subcommand gets its whole guide.
        assert_eq!(command(&["pr", "threads"]), full(&["pr", "threads"]));
    }

    /// Every flag a usage line names exists on the command it names, so a
    /// renamed flag fails here instead of misleading an agent.
    #[test]
    fn usage_lines_name_real_commands_and_flags() {
        for usage in USAGE {
            for (line, _) in usage.lines {
                let mut parent: Vec<&str> = Vec::new();
                for segment in line.split(" | ").flat_map(|s| s.split(" / ")) {
                    let words: Vec<&str> = segment.split_whitespace().collect();
                    let words = match words.first() {
                        // Routines' own arguments are theirs, not jouet's.
                        Some(&"joue") => continue,
                        Some(&"jouet") => words[1..].to_vec(),
                        _ => parent.iter().copied().chain(words).collect(),
                    };
                    let mut path: Vec<&str> = Vec::new();
                    for word in &words {
                        let next: Vec<&str> = path.iter().copied().chain([*word]).collect();
                        if lookup(&next).is_none() {
                            break;
                        }
                        path = next;
                    }
                    assert!(!path.is_empty(), "`{line}` names no jouet command");
                    parent = path[..path.len() - 1].to_vec();
                    for word in &words {
                        let flag = word.trim_matches(|c| "[]'\"".contains(c));
                        let Some(long) = flag.strip_prefix("--").filter(|l| !l.is_empty()) else {
                            continue;
                        };
                        let known = (0..=path.len()).any(|n| {
                            lookup(&path[..n]).is_some_and(|m| {
                                m.flags.iter().any(|f| f.flag.longs.contains(&long))
                            })
                        });
                        assert!(
                            known,
                            "`{line}`: `jouet {}` has no --{long}",
                            path.join(" ")
                        );
                    }
                }
            }
        }
    }
}
