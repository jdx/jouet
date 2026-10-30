//! What a call was for.

use crate::event::{Call, Tool};
use crate::shape::programs;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

/// What a call was for. Requests that issued no call are [`Category::Reply`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Checking whether CI has finished: `gh pr checks`, `gh run view`,
    /// `statusCheckRollup`, and sleep loops around them.
    CiPolling,
    /// Checking on a running process: Codex's `wait` and `write_stdin`,
    /// Claude Code's `BashOutput`, and `sleep` before looking at a log or PID.
    ProcessPolling,
    /// Listing, replying to and resolving review threads.
    PrThreads,
    /// Reading where pull requests stand: `gh pr view`, `gh pr list`,
    /// `mergeable`, `reviewDecision`.
    PrStatus,
    /// Any other GitHub call.
    Github,
    /// Editing files: the harness's edit tools, `apply_patch`, `sed -i` and
    /// scripts that rewrite files.
    Edits,
    /// Builds, tests and linters.
    BuildTestLint,
    /// Reading, searching and listing files.
    ReadSearch,
    /// Local git.
    Git,
    Web,
    /// Starting, messaging or waiting for other agents.
    Agents,
    /// A request that called no tool: the model answering or thinking.
    Reply,
    Other,
}

impl Category {
    pub const ALL: [Self; 13] = [
        Self::CiPolling,
        Self::ProcessPolling,
        Self::PrThreads,
        Self::PrStatus,
        Self::Github,
        Self::Edits,
        Self::BuildTestLint,
        Self::ReadSearch,
        Self::Git,
        Self::Web,
        Self::Agents,
        Self::Reply,
        Self::Other,
    ];

    /// The `snake_case` name JSON uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::CiPolling => "ci_polling",
            Self::ProcessPolling => "process_polling",
            Self::PrThreads => "pr_threads",
            Self::PrStatus => "pr_status",
            Self::Github => "github",
            Self::Edits => "edits",
            Self::BuildTestLint => "build_test_lint",
            Self::ReadSearch => "read_search",
            Self::Git => "git",
            Self::Web => "web",
            Self::Agents => "agents",
            Self::Reply => "reply",
            Self::Other => "other",
        }
    }

    /// A short label for text output.
    pub fn label(self) -> &'static str {
        match self {
            Self::CiPolling => "CI polling",
            Self::ProcessPolling => "process polling",
            Self::PrThreads => "PR threads",
            Self::PrStatus => "PR status",
            Self::Github => "other GitHub",
            Self::Edits => "edits",
            Self::BuildTestLint => "build/test/lint",
            Self::ReadSearch => "reads and searches",
            Self::Git => "git",
            Self::Web => "web",
            Self::Agents => "agents",
            Self::Reply => "replies (no tool)",
            Self::Other => "other",
        }
    }

    pub(crate) fn index(self) -> usize {
        self as usize
    }
}

static CI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\bgh\s+(?:pr\s+checks|run\s+(?:view|watch|list)|workflow\s+view)\b|statusCheckRollup",
        r"|\bcheck-runs\b|\bcheck-suites\b|/actions/runs\b|\bjouet\s+wait\s+(?:ci|run)\b",
    ))
    .expect("the CI pattern compiles")
});
static THREADS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"reviewThreads|resolveReviewThread|addPullRequestReviewThreadReply|addPullRequestReviewComment",
        r"|pulls/(?:\d+|<n>)/comments|comments/(?:\d+|<n>)/replies|\bjouet\s+pr\s+threads\b",
    ))
    .expect("the threads pattern compiles")
});
static STATUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\bgh\s+(?:pr\s+(?:view|list|status)|search\s+prs)\b|\bmergeable|reviewDecision",
        r"|\bjouet\s+(?:pr\s+status|wait\s+pr)\b",
    ))
    .expect("the status pattern compiles")
});
static EDIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\bsed\s+(?:-[a-zA-Z]*\s+)*-[a-zA-Z]*i|\bperl\s+-[a-z]*p[a-z]*i|\bapply_patch\b|\bjouet\s+edit\b",
        r"|\.write_text\(|\.replace\(.*(?s:.*)\.write|writeFileSync|File\.write",
        r"|\b(?:cat|tee)\s+(?:-a\s+)?>?\s*[^\s|;&<>]+\s*<<|\bcat\s+<<-?\s*\S+\s*>",
    ))
    .expect("the edit pattern compiles")
});
static GITHUB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bgh\s|api\.github\.com").expect("the GitHub pattern compiles"));
static SLEEP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsleep\s+\d").expect("the sleep pattern compiles"));

/// Programs whose run is a build, a test or a linter.
const BUILD: &[&str] = &[
    "cargo",
    "mbx",
    "hk",
    "make",
    "just",
    "bats",
    "pytest",
    "tox",
    "tsc",
    "eslint",
    "prettier",
    "biome",
    "oxlint",
    "jest",
    "vitest",
    "shellcheck",
    "shfmt",
    "rustfmt",
    "clippy-driver",
    "jactionlint",
    "actionlint",
    "golangci-lint",
    "ruff",
    "mypy",
    "gradle",
    "mvn",
];
/// Programs that read, search or list files.
const READ: &[&str] = &[
    "cat",
    "head",
    "tail",
    "sed",
    "nl",
    "rg",
    "grep",
    "ls",
    "find",
    "fd",
    "wc",
    "tree",
    "bat",
    "less",
    "stat",
    "file",
    "du",
    "awk",
    "cut",
    "sort",
    "uniq",
    "diff",
    "realpath",
    "readlink",
    "pwd",
    "jq",
    "yq",
    "xxd",
    "od",
    "sha256sum",
    "md5sum",
];

/// Programs that set up for the command after them: `cd x && sed -n ...`
/// is a read.
const SETUP: &[&str] = &[
    "cd", "pushd", "popd", "export", "set", "unset", "source", ".", "true", ":", "echo", "printf",
    "shopt", "trap", "ulimit", "local",
];

/// The category of a call. Shell commands are categorized by what they run,
/// most specific first: a command that polls CI and also reads a file is CI
/// polling.
pub fn categorize(call: &Call) -> Category {
    match call.tool {
        Tool::Shell => call
            .command
            .as_deref()
            .map_or(Category::Other, categorize_command),
        Tool::Read | Tool::Search => Category::ReadSearch,
        Tool::Edit => Category::Edits,
        Tool::Poll => Category::ProcessPolling,
        Tool::Github => Category::Github,
        Tool::Web => Category::Web,
        Tool::Agent => Category::Agents,
        Tool::Other => Category::Other,
    }
}

/// The category of a shell command, or of its shape.
pub(crate) fn categorize_command(command: &str) -> Category {
    if CI.is_match(command) {
        return Category::CiPolling;
    }
    // A sleep before a GitHub query is waiting on CI or a PR, whatever it asks.
    let sleeps = SLEEP.is_match(command);
    if sleeps && GITHUB.is_match(command) {
        return Category::CiPolling;
    }
    if THREADS.is_match(command) {
        return Category::PrThreads;
    }
    if STATUS.is_match(command) {
        return Category::PrStatus;
    }
    if EDIT.is_match(command) {
        return Category::Edits;
    }
    let programs = programs(command);
    let runs = |p: &str| programs.iter().any(|q| q == p);
    if programs.iter().any(|p| is_build(p, command)) {
        return Category::BuildTestLint;
    }
    if sleeps || runs("pgrep") || runs("wait") {
        return Category::ProcessPolling;
    }
    if GITHUB.is_match(command) {
        return Category::Github;
    }
    if runs("git") {
        return Category::Git;
    }
    let first = programs.iter().find(|p| !SETUP.contains(&p.as_str()));
    if first.is_some_and(|p| READ.contains(&p.as_str())) {
        return Category::ReadSearch;
    }
    if runs("curl") || runs("wget") {
        return Category::Web;
    }
    Category::Other
}

/// Whether a program run is a build, a test or a linter. Task runners count
/// when the task is a build, test, check or lint task.
fn is_build(program: &str, command: &str) -> bool {
    if BUILD.contains(&program) {
        return true;
    }
    static TASK: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(concat!(
            r"\b(?:mise|npm|pnpm|yarn|bun|aube)\s+(?:run\s+|r\s+)?(?:test|build|check|lint|ci|e2e|b|t)\b",
            r"|\bgo\s+(?:test|build|vet)\b|\bnpm\s+test\b|\bmise\s+(?:run|r)\s+\S+",
        ))
        .expect("the task pattern compiles")
    });
    matches!(
        program,
        "mise" | "npm" | "pnpm" | "yarn" | "bun" | "aube" | "go"
    ) && TASK.is_match(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat(command: &str) -> Category {
        categorize_command(command)
    }

    #[test]
    fn categorizes_github_by_what_it_asks() {
        assert_eq!(cat("gh pr checks 12 --watch"), Category::CiPolling);
        assert_eq!(cat("gh run view 123 --log-failed"), Category::CiPolling);
        assert_eq!(
            cat("gh pr view 1 --json statusCheckRollup"),
            Category::CiPolling
        );
        assert_eq!(
            cat("until gh pr view 1 --json state; do sleep 30; done"),
            Category::CiPolling
        );
        assert_eq!(
            cat("sleep 60 && gh api repos/x/y/pulls/1"),
            Category::CiPolling
        );
        assert_eq!(
            cat(
                "gh api graphql -f query='{ repository { pullRequest { reviewThreads { nodes { id } } } } }'"
            ),
            Category::PrThreads
        );
        assert_eq!(
            cat("gh api repos/o/r/pulls/12/comments/34/replies"),
            Category::PrThreads
        );
        assert_eq!(
            cat("gh pr view 12 --json mergeable,reviewDecision"),
            Category::PrStatus
        );
        assert_eq!(cat("gh pr list --author @me"), Category::PrStatus);
        assert_eq!(cat("gh pr create --fill"), Category::Github);
        assert_eq!(cat("jouet wait ci"), Category::CiPolling);
        assert_eq!(cat("jouet pr threads 12"), Category::PrThreads);
    }

    #[test]
    fn categorizes_local_work() {
        assert_eq!(
            cat("cargo test -p x 2>&1 | tail -20"),
            Category::BuildTestLint
        );
        assert_eq!(cat("mise run lint-fix"), Category::BuildTestLint);
        assert_eq!(cat("mise x hk -- hk check --all"), Category::BuildTestLint);
        assert_eq!(cat("sed -i 's/a/b/' f.rs"), Category::Edits);
        assert_eq!(
            cat(
                "python3 - <<'EOF'\np=open('f').read()\np=p.replace('a','b')\nopen('f','w').write(p)\nEOF"
            ),
            Category::Edits
        );
        assert_eq!(cat("cat > notes.md <<'EOF'\nhi\nEOF"), Category::Edits);
        assert_eq!(cat("sed -n '1,20p' src/main.rs"), Category::ReadSearch);
        assert_eq!(
            cat("cd ~/src/x && sed -n '1,20p' a.rs; echo ---"),
            Category::ReadSearch
        );
        assert_eq!(cat("rg -n foo src | head"), Category::ReadSearch);
        assert_eq!(
            cat("git status --short && git log --oneline -3"),
            Category::Git
        );
        assert_eq!(cat("sleep 20; tail -5 build.log"), Category::ProcessPolling);
        assert_eq!(cat("pgrep -f server"), Category::ProcessPolling);
        assert_eq!(cat("curl -s https://example.com"), Category::Web);
        assert_eq!(cat("echo hi"), Category::Other);
    }

    #[test]
    fn categorizes_shapes_as_well_as_commands() {
        assert_eq!(cat("gh pr checks <n> --watch"), Category::CiPolling);
        assert_eq!(cat("cargo test 2>&1 | head -<n>"), Category::BuildTestLint);
        assert_eq!(
            cat("GH_TOKEN= gh pr view <n> --json <v>"),
            Category::PrStatus
        );
    }

    #[test]
    fn tools_other_than_the_shell_have_fixed_categories() {
        assert_eq!(categorize(&Call::new(Tool::Poll)), Category::ProcessPolling);
        assert_eq!(categorize(&Call::new(Tool::Read)), Category::ReadSearch);
        assert_eq!(categorize(&Call::new(Tool::Edit)), Category::Edits);
        assert_eq!(categorize(&Call::shell("git diff")), Category::Git);
    }
}
