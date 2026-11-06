//! The scripts in the nearest Node package manifest.

use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

/// `None` means that no manifest was found. An unreadable, oversized or
/// malformed manifest is not evidence that a script is absent.
pub(super) fn scripts(dir: &Path) -> Result<Option<Value>, String> {
    let mut path = None;
    for candidate in dir.ancestors().map(|d| d.join("package.json")) {
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {
                path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("reading {}: {error}", candidate.display())),
        }
    }
    let Some(path) = path else { return Ok(None) };
    let metadata =
        std::fs::metadata(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let file =
        std::fs::File::open(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(super::FILE_MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    if bytes.len() as u64 > super::FILE_MAX {
        return Err(format!("{} is over 4 MiB", path.display()));
    }
    let manifest: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let object = manifest
        .as_object()
        .ok_or_else(|| format!("{} is not an object", path.display()))?;
    match object.get("scripts") {
        None => Ok(Some(json!([]))),
        Some(Value::Object(scripts)) => Ok(Some(json!(scripts.keys().collect::<Vec<_>>()))),
        Some(_) => Err(format!("{} scripts is not an object", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_nearest_manifest_and_does_not_assume_malformed_data_is_empty() {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"vitest":"echo hi"}}"#,
        )
        .unwrap();
        assert_eq!(scripts(&child).unwrap(), Some(json!(["vitest"])));
        std::fs::write(child.join("package.json"), "{}").unwrap();
        assert_eq!(scripts(&child).unwrap(), Some(json!([])));
        std::fs::write(child.join("package.json"), r#"{"scripts":null}"#).unwrap();
        assert!(scripts(&child).is_err());
        std::fs::write(child.join("package.json"), "not json").unwrap();
        assert!(scripts(&child).is_err());
    }
}
