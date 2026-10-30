//! `jouet.cargo.workspace()`: the Cargo workspace a directory is in, read
//! from its `Cargo.toml` files, so a rule can tell whether a cargo command
//! covered every package.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The workspace around `dir`: its root, the package whose manifest is
/// nearest `dir` (none at a virtual manifest), its members' package names
/// (none when a `members` pattern is more than a `*` in its last part), and
/// whether it sets `default-members`. `Ok(None)` outside any Cargo project.
pub(crate) fn workspace(dir: &Path) -> Result<Option<Value>, String> {
    let Some(nearest) = dir.ancestors().find(|d| d.join("Cargo.toml").is_file()) else {
        return Ok(None);
    };
    let manifest = read(nearest)?;
    let package = package_name(&manifest);
    let root = if manifest.contains_key("workspace") {
        Some((nearest.to_path_buf(), manifest.clone()))
    } else if let Some(path) = manifest
        .get("package")
        .and_then(|p| p.get("workspace"))
        .and_then(|w| w.as_str())
    {
        let root = nearest.join(path);
        Some((root.clone(), read(&root)?))
    } else {
        nearest
            .ancestors()
            .skip(1)
            .filter(|d| d.join("Cargo.toml").is_file())
            .find_map(|d| {
                let m = read(d).ok()?;
                m.contains_key("workspace").then(|| (d.to_path_buf(), m))
            })
    };
    let Some((root, root_manifest)) = root else {
        // A package of its own.
        return Ok(Some(json!({
            "root": nearest.to_string_lossy(),
            "package": package,
            "members": package.iter().collect::<Vec<_>>(),
            "default_members": false,
        })));
    };
    let ws = &root_manifest["workspace"];
    let mut result = json!({
        "root": root.to_string_lossy(),
        "default_members": ws.get("default-members").is_some(),
    });
    if let Some(package) = package {
        result["package"] = package.into();
    }
    if let Some(members) = members(&root, &root_manifest) {
        result["members"] = members.into();
    }
    Ok(Some(result))
}

fn read(dir: &Path) -> Result<toml::Table, String> {
    let path = dir.join("Cargo.toml");
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    text.parse()
        .map_err(|e| format!("parsing {}: {e}", path.display()))
}

fn package_name(manifest: &toml::Table) -> Option<String> {
    Some(manifest.get("package")?.get("name")?.as_str()?.to_string())
}

/// The package names of the workspace's members, the root package among
/// them; None when one cannot be told.
fn members(root: &Path, manifest: &toml::Table) -> Option<Vec<String>> {
    let ws = manifest.get("workspace")?;
    let strings = |key: &str| -> Vec<&str> {
        ws.get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default()
    };
    let exclude: Vec<PathBuf> = strings("exclude").iter().map(|e| root.join(e)).collect();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for pattern in strings("members") {
        let pattern = pattern.trim_end_matches('/');
        let (parent, last) = pattern.rsplit_once('/').unwrap_or(("", pattern));
        if parent.contains(['*', '?', '[']) || last.contains('[') {
            return None;
        }
        if !last.contains(['*', '?']) {
            dirs.push(root.join(pattern));
            continue;
        }
        let mut found: Vec<PathBuf> = std::fs::read_dir(root.join(parent))
            .ok()?
            .flatten()
            .filter(|e| glob(last, &e.file_name().to_string_lossy()))
            .map(|e| e.path())
            .filter(|p| p.join("Cargo.toml").is_file())
            .collect();
        found.sort();
        dirs.extend(found);
    }
    let mut names: Vec<String> = package_name(manifest).into_iter().collect();
    for dir in dirs.iter().filter(|d| !exclude.contains(d)) {
        names.push(package_name(&read(dir).ok()?)?);
    }
    names.sort();
    names.dedup();
    Some(names)
}

/// A `*` and `?` glob over one path part.
fn glob(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    go(&p, &n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn reads_a_workspace_from_any_of_its_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write(
            dir,
            "Cargo.toml",
            "[package]\nname = \"app\"\n[workspace]\nmembers = [\"crates/*\", \"tools/gen\"]\nexclude = [\"crates/old\"]\n",
        );
        write(dir, "crates/one/Cargo.toml", "[package]\nname = \"one\"\n");
        write(dir, "crates/two/Cargo.toml", "[package]\nname = \"two\"\n");
        write(dir, "crates/old/Cargo.toml", "[package]\nname = \"old\"\n");
        write(dir, "crates/notes/README", "");
        write(dir, "tools/gen/Cargo.toml", "[package]\nname = \"gen\"\n");
        let root = workspace(dir).unwrap().unwrap();
        assert_eq!(root["package"], "app");
        assert_eq!(root["members"], json!(["app", "gen", "one", "two"]));
        assert_eq!(root["default_members"], false);
        let member = workspace(&dir.join("crates/one/src")).unwrap().unwrap();
        assert_eq!(member["package"], "one");
        assert_eq!(member["root"], root["root"]);
        assert_eq!(member["members"], root["members"]);
    }

    #[test]
    fn a_lone_package_and_a_virtual_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        write(
            tmp.path(),
            "solo/Cargo.toml",
            "[package]\nname = \"solo\"\n",
        );
        let solo = workspace(&tmp.path().join("solo")).unwrap().unwrap();
        assert_eq!(solo["members"], json!(["solo"]));
        write(
            tmp.path(),
            "virt/Cargo.toml",
            "[workspace]\nmembers = [\"a\", \"b/**\"]\ndefault-members = [\"a\"]\n",
        );
        let virt = workspace(&tmp.path().join("virt")).unwrap().unwrap();
        assert_eq!(virt.get("package"), None);
        assert_eq!(virt.get("members"), None);
        assert_eq!(virt["default_members"], true);
        assert_eq!(workspace(tmp.path()).unwrap(), None);
    }

    #[test]
    fn globs_match_one_part() {
        assert!(glob("*", "x"));
        assert!(glob("jouet-*", "jouet-edit"));
        assert!(!glob("jouet-*", "mise"));
        assert!(glob("a?c", "abc"));
    }
}
