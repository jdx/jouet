//! Compiled rules, cached on disk by a hash of their source.
//!
//! Every hook is a new process, so without a cache each one compiles every
//! rule again. Luau does not verify bytecode, so the cache must live where
//! only the user can write (jouet uses `$XDG_CACHE_HOME`), never in a
//! repository.

use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// The rule's bytecode, and whether it came from the cache: from the cache
/// when it holds this exact source, otherwise compiled and, when there is a
/// cache, stored there. The key does not name the Luau version, so a caller
/// whose cached chunk fails to load calls [`recompile`].
pub(crate) fn compile(code: &str, cache: Option<&Path>) -> Result<(Vec<u8>, bool), String> {
    let key = key(code);
    let path = cache.map(|dir| dir.join(format!("{key}.luauc")));
    if let Some(bytes) = path.as_ref().and_then(|p| std::fs::read(p).ok())
        && !bytes.is_empty()
    {
        return Ok((bytes, true));
    }
    Ok((fresh(code, path.as_deref())?, false))
}

/// Compiles the rule again, replacing whatever the cache held for it.
pub(crate) fn recompile(code: &str, cache: Option<&Path>) -> Result<Vec<u8>, String> {
    let path = cache.map(|dir| dir.join(format!("{}.luauc", key(code))));
    fresh(code, path.as_deref())
}

fn fresh(code: &str, path: Option<&Path>) -> Result<Vec<u8>, String> {
    let bytes = mlua::chunk::Compiler::new()
        .compile(code)
        .map_err(|e| e.to_string().lines().next().unwrap_or("").to_string())?;
    if let Some(path) = path {
        store(path, &bytes);
    }
    Ok(bytes)
}

fn key(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(env!("CARGO_PKG_VERSION"));
    hasher.update([0]);
    hasher.update(code);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Writes the file beside its final name and renames it into place, so a
/// concurrent reader never sees half of it. Best effort.
fn store(path: &Path, bytes: &[u8]) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Threads compiling the same rule in one process must never share a
    // temporary file: one could rename it while another is still writing.
    for _ in 0..16 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp{}-{sequence}", std::process::id()));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return,
        };
        let written = file.write_all(bytes).is_ok();
        drop(file);
        if !written || std::fs::rename(&tmp, path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_stores_do_not_expose_partial_bytecode_or_clobber_a_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rule.luauc");
        let stale = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&stale, b"another writer").unwrap();
        let bytes = Arc::new(vec![42; 128 * 1024]);
        store(&path, &bytes);
        assert_eq!(std::fs::read(&stale).unwrap(), b"another writer");

        let barrier = Arc::new(Barrier::new(5));
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let barrier = barrier.clone();
                let bytes = bytes.clone();
                let path = &path;
                scope.spawn(move || {
                    barrier.wait();
                    for _ in 0..16 {
                        store(path, &bytes);
                    }
                });
            }
            barrier.wait();
            for _ in 0..128 {
                assert_eq!(std::fs::read(&path).unwrap(), *bytes);
            }
        });
        assert_eq!(std::fs::read(&path).unwrap(), *bytes);
    }
}
