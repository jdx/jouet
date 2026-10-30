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
//!
//! [stats]
//! summary = "plain"
//!
//! [wind]
//! enabled = false
//! ```

use usage_rs::config::{EnvLayer, FileLayer, FileScope, Layers, resolve};

#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
pub(crate) struct Settings {
    #[usage(flatten)]
    pub rules: RuleSettings,
    #[usage(flatten)]
    pub hooks: HookSettings,
    #[usage(flatten)]
    pub stats: StatsSettings,
    #[usage(flatten)]
    pub wind: WindSettings,
    #[usage(flatten)]
    pub share: ShareSettings,
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
    /// Block finishing while a test, lint or build run failed, or after only
    /// part of the test suite ran on changed code
    #[usage(env = "JOUET_HOOKS_VERIFY", default = true)]
    pub verify: bool,
    /// Keep a ledger of command shapes and rule decisions
    #[usage(env = "JOUET_HOOKS_LEDGER", default = true)]
    pub ledger: bool,
    /// Milliseconds a tool hook may take before the ledger records it as slow; session start and stop get ten times this
    #[usage(env = "JOUET_HOOKS_BUDGET_MS", default = 500)]
    pub budget_ms: u64,
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
    /// Let rules in hooks use the GitHub probes (`jouet.pr.*`); off so a
    /// hook never waits on the network
    #[usage(env = "JOUET_RULES_NETWORK", default = false)]
    pub network: bool,
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
