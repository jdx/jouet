//! `jouet.claims.held()`: the advisory claims `jouet claim` keeps for the
//! repository, and `jouet.git.changed()`: the files a commit would take.

use jouet_git::claim::{self, Store};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// Every live claim in the repository containing `dir`, as JSON.
pub(crate) fn list(state: &Path, dir: &Path) -> Result<Option<Value>, String> {
    let Some(store) = Store::open(state, dir) else {
        return Ok(None);
    };
    let claims = store.live(jouet_output::time::now());
    serde_json::to_value(claims)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// The claims in `claims` that cover any of `paths`, each with the `path`
/// it covers, relative to the repository's root. A path given is relative
/// to `dir` or absolute; a directory covers the claims inside it, as a
/// pathspec does. Without a work tree to place them in, as in a rule's
/// examples, paths are taken as relative to the repository's root.
pub(crate) fn held(claims: &Value, paths: &[String], dir: Option<&Path>) -> Value {
    let root = dir.and_then(jouet_output::paths::work_tree);
    let mut hits = Vec::new();
    for given in paths {
        let rel = match (&root, dir) {
            (Some(root), Some(dir)) => claim::relative(root, dir, given),
            _ => (!Path::new(given).is_absolute())
                .then(|| given.trim_start_matches("./").to_string()),
        };
        let Some(rel) = rel.filter(|r| !r.is_empty()) else {
            continue;
        };
        for c in claims.as_array().into_iter().flatten() {
            let Some(pattern) = c["pattern"].as_str() else {
                continue;
            };
            let inside = pattern.starts_with(&format!("{}/", rel.trim_end_matches('/')));
            if claim::covers(pattern, &rel) || inside {
                let mut hit = c.clone();
                hit["path"] = rel.as_str().into();
                hits.push(hit);
            }
        }
    }
    Value::Array(hits)
}

/// The files a commit would take, as absolute paths: what is staged
/// (`staged`), or every change to a tracked file (`tracked`, as `git commit
/// -a` takes).
pub(crate) fn changed(dir: Option<&Path>, scope: &str, timeout: Duration) -> Result<Value, String> {
    let args: &[&str] = match scope {
        "staged" => &["diff", "--cached", "--name-only", "-z", "--no-renames"],
        _ => &["diff", "HEAD", "--name-only", "-z", "--no-renames"],
    };
    let started = std::time::Instant::now();
    let root = super::git::git(dir, &["rev-parse", "--show-toplevel"], timeout)?;
    let left = timeout.saturating_sub(started.elapsed());
    let out = super::git::git(dir, args, left)?;
    let root = Path::new(root.trim());
    Ok(out
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(|p| Value::from(root.join(p).to_string_lossy().into_owned()))
        .collect())
}
