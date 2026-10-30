//! jouet's settings, from `~/.config/jouet/config.toml` and `JOUET_*`
//! environment variables, resolved by usage-config.
//!
//! ```toml
//! [rules]
//! disable = ["git-stash", "no-verify"]
//! network = true
//! ```

use usage_rs::config::{EnvLayer, FileLayer, FileScope, Layers, resolve};

#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
pub(crate) struct Settings {
    #[usage(flatten)]
    pub rules: RuleSettings,
}

/// Which rules run.
#[derive(usage_rs::Config, Debug, Clone, PartialEq)]
#[usage(prefix = "rules")]
pub(crate) struct RuleSettings {
    /// Rules to turn off, by name; "all" turns every rule off
    #[usage(env = "JOUET_RULES_DISABLE", merge = "union", parse = "list_by_comma")]
    pub disable: Option<Vec<String>>,
    /// Let rules in hooks use the GitHub probes (`jouet.pr.*`); off so a
    /// hook never waits on the network. Recipes always have them
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
    fn a_broken_file_falls_back_to_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[rules\n").unwrap();
        let s = Settings::load_from(&env(&[("JOUET_RULES_DISABLE", "force-push")]), Some(path));
        assert_eq!(s.rules.disable.unwrap(), ["force-push"]);
    }
}
