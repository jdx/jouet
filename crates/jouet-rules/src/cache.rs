//! Compiled rules, cached on disk by a hash of their source.
//!
//! Every hook is a new process, so without a cache each one compiles every
//! rule again. Luau does not verify bytecode, so the cache must live where
//! only the user can write (jouet uses `$XDG_CACHE_HOME`), never in a
//! repository.

use sha2::{Digest, Sha256};
use std::path::Path;

/// The rule's bytecode: from the cache when it holds this exact source,
/// otherwise compiled and, when there is a cache, stored there.
pub fn compile(code: &str, cache: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut hasher = Sha256::new();
    hasher.update(env!("CARGO_PKG_VERSION"));
    hasher.update([0]);
    hasher.update(code);
    let key: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let path = cache.map(|dir| dir.join(format!("{key}.luauc")));
    if let Some(bytes) = path.as_ref().and_then(|p| std::fs::read(p).ok())
        && !bytes.is_empty()
    {
        return Ok(bytes);
    }
    let bytes = mlua::chunk::Compiler::new()
        .compile(code)
        .map_err(|e| e.to_string().lines().next().unwrap_or("").to_string())?;
    if let Some(path) = path {
        store(&path, &bytes);
    }
    Ok(bytes)
}

/// Writes the file beside its final name and renames it into place, so a
/// concurrent reader never sees half of it. Best effort.
fn store(path: &Path, bytes: &[u8]) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}
