//! Answers kept under `<cache dir>/deps`, each in its own file with the
//! time it was fetched. A cache that cannot be read or written is only
//! slower.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::time::Duration;

/// How long a registry's answer is kept: a new release shows within this.
pub(crate) const REGISTRY_TTL: Duration = Duration::from_secs(10 * 60);
/// How long OSV's answers are kept.
pub(crate) const OSV_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Serialize, serde::Deserialize)]
struct Entry<T> {
    /// Unix seconds.
    at: i64,
    value: T,
}

/// Bumped when what is cached changes shape or meaning, so older answers
/// are fetched again.
const FORMAT: &str = "v1";

/// `<cache dir>/deps/v1/<parts>.json`, each part made safe for a file name:
/// `@scope/name` and `github.com/a/b` are one file each.
fn path(parts: &[&str]) -> Option<PathBuf> {
    let mut path = jouet_output::paths::cache_dir()?.join("deps").join(FORMAT);
    for (i, part) in parts.iter().enumerate() {
        let mut safe: String = part
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-_.@".contains(c) {
                    c
                } else {
                    '+'
                }
            })
            .collect();
        if safe.starts_with('.') {
            safe.insert(0, '+');
        }
        if i + 1 == parts.len() {
            safe.push_str(".json");
        }
        path.push(safe);
    }
    Some(path)
}

pub(crate) fn read<T: DeserializeOwned>(parts: &[&str], ttl: Duration) -> Option<T> {
    let bytes = std::fs::read(path(parts)?).ok()?;
    let entry: Entry<T> = serde_json::from_slice(&bytes).ok()?;
    let age = jouet_output::time::now() - entry.at;
    (0..i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX))
        .contains(&age)
        .then_some(entry.value)
}

/// Writes through a temporary file, so a reader never sees half of one.
pub(crate) fn write<T: Serialize>(parts: &[&str], value: &T) {
    let Some(path) = path(parts) else { return };
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let entry = Entry {
        at: jouet_output::time::now(),
        value,
    };
    let Ok(bytes) = serde_json::to_vec(&entry) else {
        return;
    };
    if let Ok(mut tmp) = tempfile::NamedTempFile::new_in(dir) {
        use std::io::Write as _;
        if tmp.write_all(&bytes).is_ok() {
            let _ = tmp.persist(&path);
        }
    }
}
