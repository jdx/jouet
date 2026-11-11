//! jouet's settings, from `~/.config/jouet/config.toml` and `JOUET_*`
//! environment variables, resolved by usage-config.
//!
//! ```toml
//! [rules]
//! disable = ["git-stash", "no-verify"]
//! network = true
//!
//! [hooks]
//! verify = false
//! gate = "block"
//!
//! [edit]
//! fix_after = true
//!
//! [stats]
//! summary = "plain"
//!
//! [test]
//! baseline = "off"
//!
//! [wind]
//! enabled = false
//!
//! [entire]
//! timeout_ms = 3000
//! ```

use usage_rs::config::{EnvLayer, FileLayer, FileScope, Layers, resolve};

#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
pub(crate) struct Settings {
    #[usage(flatten)]
    pub rules: RuleSettings,
    #[usage(flatten)]
    pub hooks: HookSettings,
    #[usage(flatten)]
    pub edit: EditSettings,
    #[usage(flatten)]
    pub stats: StatsSettings,
    #[usage(flatten)]
    pub wind: WindSettings,
    #[usage(flatten)]
    pub share: ShareSettings,
    #[usage(flatten)]
    pub entire: EntireSettings,
    #[usage(flatten)]
    pub test: TestSettings,
    #[usage(flatten)]
    pub cost: CostSettings,
    #[usage(flatten)]
    pub experiment: ExperimentSettings,
}

/// The experiment arms (see [`crate::experiment`]).
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "experiment")]
pub(crate) struct ExperimentSettings {
    /// Percent of sessions, 0 to 100, that keep the old behaviour of each toggle under test: the stop gate blocking, the python-edit rule denying, and the piped-test-output rule warning about and then denying a cut test run rather than running it under `jouet test --cmd`; 0 turns the arms off
    #[usage(env = "JOUET_EXPERIMENT_CONTROL_SHARE", default = 50)]
    pub control_share: u64,
    /// Percent of Claude Code sessions, 0 to 100, whose primer has the agent run `jouet wait pr --until needs-agent` in the background after each push, alongside the desktop app's Auto-fix; 0 turns this arm off
    #[usage(env = "JOUET_EXPERIMENT_WATCHER_SHARE", default = 25)]
    pub watcher_share: u64,
}

impl Settings {
    /// The arm `session` is in, by `[experiment] control_share` and
    /// `watcher_share`.
    pub(crate) fn arm(&self, session: Option<&str>) -> crate::experiment::Arm {
        session.map_or_else(Default::default, |s| {
            crate::experiment::Arm::of(
                s,
                self.experiment.control_share,
                self.experiment.watcher_share,
            )
        })
    }
}

/// What builds, tests and lints cost here, kept as git notes (see
/// [`crate::cost`]).
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "cost")]
pub(crate) struct CostSettings {
    /// Keep how long each build, test and lint run took as a git note on HEAD, under refs/notes/jouet-cost
    #[usage(env = "JOUET_COST_RECORD", default = true)]
    pub record: bool,
    /// Record local, session-linked timings for successive jouet test checks
    #[usage(env = "JOUET_COST_SEQUENCES", default = false)]
    pub sequences: bool,
}

/// What `jouet test` does about a failing run.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "test")]
pub(crate) struct TestSettings {
    /// When a failing run is compared with the merge base: auto runs a failing full suite, or a failing --cmd run shorter than baseline_cmd_under_ms, there once per merge base, and compares any other failing run with what is kept; off only with --baseline
    #[usage(env = "JOUET_TEST_BASELINE", default = "auto", choices("auto", "off"))]
    pub baseline: String,
    /// Milliseconds under which a failing --cmd run is run again at the merge base on its own; a slower one names the --baseline command instead
    #[usage(env = "JOUET_TEST_BASELINE_CMD_UNDER_MS", default = 60000)]
    pub baseline_cmd_under_ms: u64,
    /// Seconds for which a run is answered from an earlier run of the same command on the same files (tracked and untracked, less ignored ones), HEAD and environment; 0 always runs
    #[usage(env = "JOUET_TEST_REUSE_S", default = 0)]
    pub reuse_s: u64,
}

/// Reading Entire's sessions, checkpoints and line origins, where a
/// repository has Entire enabled.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "entire")]
pub(crate) struct EntireSettings {
    /// Draw on Entire (sessions, checkpoints, line origins, review findings and the repositories it hosts) where `entire` is installed and its login works; off never runs it
    #[usage(env = "JOUET_ENTIRE", default = true)]
    pub enabled: bool,
    /// Milliseconds each `entire` call for status, sessions, checkpoints, trails, findings and discussions may take before jouet stops waiting
    #[usage(env = "JOUET_ENTIRE_TIMEOUT_MS", default = 5000)]
    pub timeout_ms: u64,
    /// Milliseconds each `entire why` for --why may take; a repository with a long history can take a minute
    #[usage(env = "JOUET_ENTIRE_WHY_TIMEOUT_MS", default = 15000)]
    pub why_timeout_ms: u64,
}

/// Contributing back to jouet, through `jouet share`.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "share")]
pub(crate) struct ShareSettings {
    /// Suggest, at session start, sharing what sessions show about jouet itself (crashes, confusing flags, abandoned output) with its maintainers; every post still needs the user's yes
    #[usage(env = "JOUET_SHARE_CONTRIBUTE", default = false)]
    pub contribute: bool,
}

/// Self-winding: the ledger the hooks keep, and the rules drafted from it.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "wind")]
pub(crate) struct WindSettings {
    /// Draft rules from costly patterns in the hook ledger, and show them at session start
    #[usage(env = "JOUET_WIND", default = true)]
    pub enabled: bool,
    /// Promote a user rule from warn to deny once it has fired often with no overrides
    #[usage(env = "JOUET_WIND_AUTO_PROMOTE", default = false)]
    pub auto_promote: bool,
    /// Firings, with no overrides, before a rule is promoted automatically
    #[usage(env = "JOUET_WIND_PROMOTE_AFTER", default = 20)]
    pub promote_after: u64,
    /// Digest each session at its stop hook, and suggest reviewing one with user corrections or a lot of waste at the next session start
    #[usage(env = "JOUET_WIND_REVIEW", default = false)]
    pub review: bool,
    /// Sessions, at least, between two suggested reviews
    #[usage(env = "JOUET_WIND_REVIEW_EVERY", default = 5)]
    pub review_every: u64,
    /// User corrections in a session that make it worth a review
    #[usage(env = "JOUET_WIND_REVIEW_CORRECTIONS", default = 2)]
    pub review_corrections: u64,
    /// Check your agents' memories once a day for workarounds whose bug looks fixed, and note them at session start
    #[usage(env = "JOUET_WIND_MEMORIES", default = true)]
    pub memories: bool,
    /// A gitleaks config whose rules also redact digests; ~/.config/jouet/gitleaks.toml when it exists
    #[usage(env = "JOUET_WIND_GITLEAKS")]
    pub gitleaks: Option<String>,
}

/// What the hooks `jouet setup` installs do.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "hooks")]
pub(crate) struct HookSettings {
    /// Run jouet's hooks at all
    #[usage(env = "JOUET_HOOKS", default = true)]
    pub enabled: bool,
    /// Run the pre-tool-use rules, which deny or warn about commands
    #[usage(env = "JOUET_HOOKS_STEER", default = true)]
    pub steer: bool,
    /// Print a short jouet guide when a session starts
    #[usage(env = "JOUET_HOOKS_PRIME", default = true)]
    pub prime: bool,
    /// Record test, lint and build runs, and judge each stop by them: whether
    /// a run failed, or files changed with no test run passing since
    #[usage(env = "JOUET_HOOKS_VERIFY", default = true)]
    pub verify: bool,
    /// What the stop gate does where it would block: observe records a would-block in the ledger and lets the agent finish; block stops the agent once with the reason
    #[usage(
        env = "JOUET_HOOKS_GATE",
        default = "observe",
        choices("observe", "block")
    )]
    pub gate: String,
    /// Keep a ledger of command shapes and rule decisions
    #[usage(env = "JOUET_HOOKS_LEDGER", default = true)]
    pub ledger: bool,
    /// Re-index the files an agent edits in the code graph, when the
    /// repository has one
    #[usage(env = "JOUET_HOOKS_GRAPH", default = true)]
    pub graph: bool,
    /// Milliseconds a tool hook may take before the ledger records it as slow; session start and stop get ten times this
    #[usage(env = "JOUET_HOOKS_BUDGET_MS", default = 500)]
    pub budget_ms: u64,
}

/// What happens after a file is edited.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "edit")]
pub(crate) struct EditSettings {
    /// Format the files `jouet edit` writes, and those the harness's Edit and Write tools change, with `hk run fix --safe` when the repository has an hk config
    #[usage(env = "JOUET_EDIT_FIX_AFTER", default = false)]
    pub fix_after: bool,
    /// Milliseconds the fix after an edit may take before it is stopped
    #[usage(env = "JOUET_EDIT_FIX_TIMEOUT_MS", default = 8000)]
    pub fix_timeout_ms: u64,
    /// Only these hk steps format after an edit, such as cargo-fmt; every fix step when unset
    #[usage(env = "JOUET_EDIT_FIX_STEPS", merge = "union", parse = "list_by_comma")]
    pub fix_steps: Option<Vec<String>>,
}

/// What `jouet stats` prints.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "stats")]
pub(crate) struct StatsSettings {
    /// How `jouet stats --summary` tells its line: quips, plain, or off for no line
    #[usage(
        env = "JOUET_STATS_SUMMARY",
        default = "quips",
        choices("quips", "plain", "off")
    )]
    pub summary: String,
}

/// Which rules run.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "rules")]
pub(crate) struct RuleSettings {
    /// Rules to turn off, by name; "all" turns every rule off
    #[usage(env = "JOUET_RULES_DISABLE", merge = "union", parse = "list_by_comma")]
    pub disable: Option<Vec<String>>,
    /// Built-in rules that are off by default to turn on, by name, such as python-edit
    #[usage(env = "JOUET_RULES_ENABLE", merge = "union", parse = "list_by_comma")]
    pub enable: Option<Vec<String>>,
    /// Let rules in hooks use the GitHub probes (`jouet.pr.*`); off so a
    /// hook never waits on the network
    #[usage(env = "JOUET_RULES_NETWORK", default = false)]
    pub network: bool,
}

impl RuleSettings {
    /// The rules to load turned off: those `disable` names, and the
    /// built-in ones off by default ([`crate::rules::OFF_BY_DEFAULT`]) that
    /// `enable` does not name. A session in python-edit's control arm
    /// ([`crate::experiment`]) has it on.
    pub(crate) fn disabled(&self, arm: crate::experiment::Arm) -> Vec<String> {
        let mut enabled = self.enable.clone().unwrap_or_default();
        if arm.python_edit {
            enabled.push(crate::experiment::PYTHON_EDIT.to_string());
        }
        let mut disabled = self.disable.clone().unwrap_or_default();
        disabled.extend(
            crate::rules::OFF_BY_DEFAULT
                .iter()
                .filter(|name| !enabled.iter().any(|e| e == *name))
                .map(|name| (*name).to_string()),
        );
        disabled
    }
}

impl HookSettings {
    /// Whether the stop gate blocks where it finds a reason (`gate =
    /// "block"`), rather than only recording it (`observe`).
    pub(crate) fn gate_blocks(&self) -> bool {
        self.gate == "block"
    }
}

impl Settings {
    /// Reads the settings. Never fails: a broken config file or variable
    /// falls back to the defaults for what it broke, because a hook must not
    /// stop working over a typo.
    pub(crate) fn load() -> Self {
        let file = jouet_output::paths::config_dir().map(|d| d.join("config.toml"));
        Self::load_from(&EnvLayer::from_process(), file.filter(|p| p.is_file()))
    }

    fn load_from(env: &EnvLayer, file: Option<std::path::PathBuf>) -> Self {
        let file = file.map(|path| FileLayer::at(path, FileScope::Global));
        let mut layers = Layers::new().then(env);
        if let Some(file) = &file {
            layers = layers.then(file);
        }
        let resolved = resolve(Self::SETTINGS_REGISTRY, layers)
            .or_else(|_| resolve(Self::SETTINGS_REGISTRY, Layers::new().then(env)))
            .or_else(|_| resolve(Self::SETTINGS_REGISTRY, Layers::new()))
            .expect("resolving only defaults cannot fail");
        Self::read_lossy(&resolved)
            .0
            .expect("every setting has a declared default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> EnvLayer {
        EnvLayer::new(vars.iter().map(|(k, v)| (k.to_string(), v.to_string())))
    }

    #[test]
    fn nothing_is_disabled_by_default() {
        let s = Settings::load_from(&env(&[]), None);
        assert_eq!(s.rules.disable, None);
        assert!(!s.rules.network);
    }

    #[test]
    fn the_network_probes_are_turned_on_by_the_file_or_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[rules]\nnetwork = true\n").unwrap();
        assert!(Settings::load_from(&env(&[]), Some(path)).rules.network);
        let on = Settings::load_from(&env(&[("JOUET_RULES_NETWORK", "true")]), None);
        assert!(on.rules.network);
    }

    #[test]
    fn the_file_and_the_environment_both_disable_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[rules]\ndisable = [\"git-stash\"]\n").unwrap();
        let s = Settings::load_from(&env(&[("JOUET_RULES_DISABLE", "no-verify")]), Some(path));
        let mut disabled = s.rules.disable.unwrap();
        disabled.sort();
        assert_eq!(disabled, ["git-stash", "no-verify"]);
    }

    #[test]
    fn every_hook_is_on_until_turned_off() {
        let s = Settings::load_from(&env(&[]), None);
        assert!(s.hooks.enabled && s.hooks.steer && s.hooks.prime && s.hooks.verify);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[hooks]\nverify = false\n").unwrap();
        let s = Settings::load_from(&env(&[("JOUET_HOOKS_PRIME", "false")]), Some(path));
        assert!(!s.hooks.verify && !s.hooks.prime && s.hooks.steer);
    }

    #[test]
    fn python_edit_is_off_until_enabled() {
        let s = Settings::load_from(&env(&[]), None);
        let new = crate::experiment::Arm::default();
        assert_eq!(s.rules.disabled(new), ["python-edit"]);
        let control = crate::experiment::Arm {
            python_edit: true,
            ..new
        };
        assert!(s.rules.disabled(control).is_empty());
        let s = Settings::load_from(&env(&[("JOUET_RULES_ENABLE", "python-edit")]), None);
        assert!(s.rules.disabled(new).is_empty());
        let s = Settings::load_from(&env(&[("JOUET_RULES_DISABLE", "git-stash")]), None);
        assert_eq!(s.rules.disabled(new), ["git-stash", "python-edit"]);
    }

    #[test]
    fn the_experiment_arms_split_sessions_evenly_by_default() {
        let s = Settings::load_from(&env(&[]), None);
        assert_eq!(s.experiment.control_share, 50);
        assert_eq!(s.experiment.watcher_share, 25);
        let control = (0..2_000)
            .filter(|n| s.arm(Some(&format!("session-{n}"))).gate_blocks)
            .count();
        assert!((900..1_100).contains(&control), "{control}");
    }

    #[test]
    fn the_stop_gate_observes_until_set_to_block() {
        let s = Settings::load_from(&env(&[]), None);
        assert_eq!(s.hooks.gate, "observe");
        assert!(!s.hooks.gate_blocks());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[hooks]\ngate = \"block\"\n").unwrap();
        assert!(
            Settings::load_from(&env(&[]), Some(path.clone()))
                .hooks
                .gate_blocks()
        );
        // The environment wins over the file, as for every setting.
        let s = Settings::load_from(&env(&[("JOUET_HOOKS_GATE", "observe")]), Some(path));
        assert!(!s.hooks.gate_blocks());
        // A value jouet cannot read counts as the default.
        let s = Settings::load_from(&env(&[("JOUET_HOOKS_GATE", "loud")]), None);
        assert!(!s.hooks.gate_blocks());
    }

    #[test]
    fn the_stats_summary_is_quips_until_set() {
        assert_eq!(Settings::load_from(&env(&[]), None).stats.summary, "quips");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[stats]\nsummary = \"plain\"\n").unwrap();
        let s = Settings::load_from(&env(&[]), Some(path.clone()));
        assert_eq!(s.stats.summary, "plain");
        let s = Settings::load_from(&env(&[("JOUET_STATS_SUMMARY", "off")]), Some(path));
        assert_eq!(s.stats.summary, "off");
    }

    #[test]
    fn winding_is_on_and_promotion_off_until_changed() {
        let s = Settings::load_from(&env(&[]), None);
        assert!(s.wind.enabled && !s.wind.auto_promote);
        assert_eq!(s.wind.promote_after, 20);
        let s = Settings::load_from(&env(&[("JOUET_WIND", "0")]), None);
        assert!(!s.wind.enabled);
    }

    #[test]
    fn contributing_is_off_and_hooks_have_a_budget() {
        let s = Settings::load_from(&env(&[]), None);
        assert!(!s.share.contribute);
        assert_eq!(s.hooks.budget_ms, 500);
        let s = Settings::load_from(&env(&[("JOUET_SHARE_CONTRIBUTE", "1")]), None);
        assert!(s.share.contribute);
    }

    #[test]
    fn fixing_after_edits_is_off_until_turned_on() {
        let s = Settings::load_from(&env(&[]), None);
        assert!(!s.edit.fix_after);
        assert_eq!((s.edit.fix_timeout_ms, s.edit.fix_steps), (8000, None));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[edit]\nfix_after = true\nfix_steps = [\"cargo-fmt\"]\n",
        )
        .unwrap();
        let s = Settings::load_from(&env(&[]), Some(path));
        assert!(s.edit.fix_after);
        assert_eq!(
            s.edit.fix_steps.as_deref(),
            Some(&["cargo-fmt".to_string()][..])
        );
    }

    #[test]
    fn session_review_is_off_until_turned_on() {
        let s = Settings::load_from(&env(&[]), None);
        assert!(!s.wind.review);
        assert_eq!((s.wind.review_every, s.wind.review_corrections), (5, 2));
        assert_eq!(s.wind.gitleaks, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[wind]\nreview = true\nreview_every = 3\n").unwrap();
        let s = Settings::load_from(&env(&[("JOUET_WIND_GITLEAKS", "/x.toml")]), Some(path));
        assert!(s.wind.review);
        assert_eq!(s.wind.review_every, 3);
        assert_eq!(s.wind.gitleaks.as_deref(), Some("/x.toml"));
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[rules\n").unwrap();
        let s = Settings::load_from(&env(&[("JOUET_RULES_DISABLE", "force-push")]), Some(path));
        assert_eq!(s.rules.disable.unwrap(), ["force-push"]);
    }
}
