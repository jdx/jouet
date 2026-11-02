//! Repository-specific options for partial test runs, gated by repository trust.

use super::Error;
use crate::sources::{self, Kind};
use jouet_output::exit::Exit;
use serde::Deserialize;
use std::path::Path;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    pytest: Pytest,
    baseline: Baseline,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Pytest {
    affected_args: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Baseline {
    setup: Vec<String>,
}

fn config(root: &Path, baseline_only: bool) -> Result<Config, Error> {
    let path = root.join(".jouet/test.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(e) => {
            return Err(Error::new(
                Exit::Precondition,
                format!("reading {}: {e}", path.display()),
            ));
        }
    };
    let config: Config = toml::from_str(&text).map_err(|e| {
        Error::new(
            Exit::Precondition,
            format!("reading {}: {e}", path.display()),
        )
    })?;
    // A repository with no setup command needs no trust to compare a full
    // suite; affected pytest arguments continue to require trust as before.
    if baseline_only && config.baseline.setup.is_empty() {
        return Ok(config);
    }
    let mut errors = Vec::new();
    let sets = sources::sets(root, None, &mut sources::digests(), &mut errors);
    let trusted = sets
        .iter()
        .any(|set| set.kind == Kind::Repo && set.status.runs());
    if !trusted {
        return Err(Error::new(
            Exit::Precondition,
            format!(
                "{} is not trusted or changed since it was trusted",
                path.display()
            ),
        )
        .hint("Review the repository's jouet files, then a person can run `jouet trust`."));
    }
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        return Err(Error::new(
            Exit::Precondition,
            format!("{} changed while its trust was checked", path.display()),
        ));
    }
    Ok(config)
}

pub(super) fn pytest_args(root: &Path) -> Result<Vec<String>, Error> {
    Ok(config(root, false)?.pytest.affected_args)
}

pub(crate) fn baseline_setup(root: &Path) -> Result<Vec<String>, Error> {
    let setup = config(root, true)?.baseline.setup;
    if setup.first().is_some_and(|word| word.is_empty()) {
        return Err(Error::new(
            Exit::Precondition,
            "baseline setup needs a nonempty program",
        ));
    }
    Ok(setup)
}
