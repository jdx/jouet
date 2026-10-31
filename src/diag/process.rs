//! Running a checker with input on stdin and a deadline.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(crate) struct Ran {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// Runs `argv` in `dir` with `input` on stdin. Past `timeout` it is asked to
/// stop (SIGTERM on Unix, so hk can stop its own steps), then killed.
pub(crate) fn run(
    argv: &[String],
    dir: &Path,
    input: &[u8],
    timeout: Duration,
) -> std::io::Result<Ran> {
    let (program, args) = argv.split_first().expect("a program to run");
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = out.read_to_end(&mut bytes);
        bytes
    });
    let err_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = err.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            terminate(&mut child);
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let _ = writer.join();
    Ok(Ran {
        code: status.code(),
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
        timed_out,
    })
}

/// SIGTERM, then SIGKILL two seconds later if it has not exited.
#[cfg(unix)]
fn terminate(child: &mut std::process::Child) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: kill(2) with a child's pid and a constant signal has no
        // memory effects.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let grace = Instant::now() + Duration::from_secs(2);
        while Instant::now() < grace {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn passes_stdin_and_stops_at_the_deadline() {
        let dir = std::env::temp_dir();
        let ran = run(&["cat".into()], &dir, b"a\0b\0", Duration::from_secs(5)).unwrap();
        assert_eq!(ran.stdout, b"a\0b\0");
        assert_eq!(ran.code, Some(0));
        let slow = run(
            &["sh".into(), "-c".into(), "sleep 5".into()],
            &dir,
            b"",
            Duration::from_millis(100),
        )
        .unwrap();
        assert!(slow.timed_out);
    }
}
