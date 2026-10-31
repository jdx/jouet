//! What a run's primitives reach: the work tree, GitHub, jouet's own
//! commands, the ledger and mise, or for `joue test` the recorded answers
//! that stand in for them.

use serde_json::Value;
use std::path::PathBuf;

/// What runs jouet's own commands for a routine: the binary, or a test's
/// recorded answers.
pub trait Host {
    /// Runs `jouet --json ARGV` and returns how it exited and the JSON
    /// document it printed. Its progress goes to stderr.
    fn jouet(&mut self, argv: &[String]) -> Result<StepOutput, String>;
}

/// What a jouet command printed and how it exited.
#[derive(Debug, Clone)]
pub struct StepOutput {
    pub exit_code: i32,
    /// Its JSON document, when it printed one.
    pub json: Option<Value>,
}

/// Where GitHub is.
#[derive(Debug, Clone)]
pub enum Github {
    /// The API in `JOUET_GITHUB_API_URL` (github.com by default), with the
    /// usual token sources.
    Env,
    /// A test's fake server, with a token of its own.
    Fake { url: String, token: String },
}

/// Everything a run reaches outside the VM.
pub struct World {
    /// The work tree's directory: git and mise run here.
    pub root: PathBuf,
    pub host: Box<dyn Host>,
    pub github: Github,
    /// The ledger's directory, and the key it names this work tree by.
    pub ledger: Option<PathBuf>,
    pub repo_key: Option<String>,
    /// Where the output of mise tasks is saved.
    pub log_dir: PathBuf,
    /// A test's answers for git, mise and the ledger. None in a real run.
    pub recorded: Option<Recorded>,
}

/// The parts of a fixture that are not GitHub: git's, mise's and the
/// ledger's answers, by primitive.
#[derive(Debug, Clone, Default)]
pub struct Recorded {
    /// `state`, `repo`, `diff`, `log`, `refs` and `rev`.
    pub git: serde_json::Map<String, Value>,
    /// `tasks`, `config`, `tools`, `env`, `test`, and `run` by task name.
    pub mise: serde_json::Map<String, Value>,
    pub ledger: Vec<Value>,
}

impl Recorded {
    pub(crate) fn from_fixture(fixture: &Value) -> Self {
        let map = |key: &str| fixture[key].as_object().cloned().unwrap_or_default();
        Self {
            git: map("git"),
            mise: map("mise"),
            ledger: fixture["ledger"].as_array().cloned().unwrap_or_default(),
        }
    }
}
