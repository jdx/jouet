//! `joue`: runs jouet's routines. A launcher for the `jouet` beside it, so
//! that `cargo install`, the release archives and every package manager
//! install `joue` as a real file, with no symlink to create.
//!
//! On Unix it becomes `jouet` with argv[0] `joue`, which jouet reads as its
//! `joue` view of `jouet run`. Windows has no exec and cannot set argv[0],
//! so there it runs `jouet run ARGS` and exits as it did.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

/// The `jouet` next to this file, else the one on PATH.
fn jouet() -> PathBuf {
    let name = format!("jouet{}", std::env::consts::EXE_SUFFIX);
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join(&name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(unix)]
fn main() -> ExitCode {
    use std::os::unix::process::CommandExt;
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let jouet = jouet();
    let error = std::process::Command::new(&jouet)
        .arg0("joue")
        .args(args)
        .exec();
    eprintln!("joue: running {}: {error}", jouet.display());
    ExitCode::from(127)
}

#[cfg(not(unix))]
fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let jouet = jouet();
    match std::process::Command::new(&jouet)
        .arg("run")
        .args(args)
        .status()
    {
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(1),
        ),
        Err(error) => {
            eprintln!("joue: running {}: {error}", jouet.display());
            ExitCode::from(127)
        }
    }
}
