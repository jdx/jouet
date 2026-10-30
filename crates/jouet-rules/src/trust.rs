//! Which repositories' rules may run.
//!
//! A repository's `.jouet/rules/` can deny commands and put text in front of
//! the agent, so it runs only once the user has trusted it. Trust is recorded
//! against a hash of every rule file in the directory: adding or changing a
//! rule needs trusting again, like a reviewed lockfile.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    /// Trusted once, but the rules have changed since.
    Changed,
    Untrusted,
}

pub struct TrustStore {
    path: PathBuf,
    /// Canonical directory → hash of its rules when it was trusted.
    dirs: BTreeMap<String, String>,
}

impl TrustStore {
    /// Reads the store; a missing or unreadable file trusts nothing.
    pub fn load(path: PathBuf) -> Self {
        let dirs = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, dirs }
    }

    pub fn check(&self, dir: &Path) -> Trust {
        let Some(trusted) = self.dirs.get(&key(dir)) else {
            return Trust::Untrusted;
        };
        match digest(dir) {
            Ok(current) if &current == trusted => Trust::Trusted,
            _ => Trust::Changed,
        }
    }

    /// Trusts the directory's rules as they are now.
    pub fn trust(&mut self, dir: &Path) -> std::io::Result<()> {
        let digest = digest(dir)?;
        self.dirs.insert(key(dir), digest);
        self.save()
    }

    /// Returns whether the directory was trusted.
    pub fn untrust(&mut self, dir: &Path) -> std::io::Result<bool> {
        let removed = self.dirs.remove(&key(dir)).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self
            .path
            .with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.dirs)?)?;
        std::fs::rename(&tmp, &self.path)
    }
}

fn key(dir: &Path) -> String {
    dir.canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .display()
        .to_string()
}

/// A hash over the names and contents of the directory's rule files.
pub fn digest(dir: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    for path in crate::rule_files(dir)? {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let code = std::fs::read(&path)?;
        hasher.update(name.len().to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update(code.len().to_le_bytes());
        hasher.update(&code);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_follows_the_rules_contents() {
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("rules");
        std::fs::create_dir(&rules).unwrap();
        std::fs::write(rules.join("a.luau"), "return {}").unwrap();
        let mut store = TrustStore::load(dir.path().join("trust.json"));
        assert_eq!(store.check(&rules), Trust::Untrusted);

        store.trust(&rules).unwrap();
        assert_eq!(store.check(&rules), Trust::Trusted);
        let reloaded = TrustStore::load(dir.path().join("trust.json"));
        assert_eq!(reloaded.check(&rules), Trust::Trusted);

        std::fs::write(rules.join("b.luau"), "return {}").unwrap();
        assert_eq!(store.check(&rules), Trust::Changed);

        assert!(store.untrust(&rules).unwrap());
        assert_eq!(store.check(&rules), Trust::Untrusted);
    }
}
