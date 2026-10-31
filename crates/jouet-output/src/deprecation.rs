//! Deprecations, the way mise handles them: [`deprecated_at!`] warns about
//! something on its way out and fails a debug build once jouet reaches the
//! version it was to be removed in; [`remove_by!`] fails the build itself,
//! for temporary code that has no path to warn from.
//!
//! A warning is one line on stderr, at most once per id in a process. The
//! binary sets a [`Recorder`] that keeps each one in the hook ledger, so an
//! agent session is told once, not on every command, and `jouet stats` and
//! `jouet wind` can find what still uses an old spelling.
//!
//! [`deprecated_at!`]: crate::deprecated_at
//! [`remove_by!`]: crate::remove_by

use crate::display::Mode;
use std::collections::BTreeSet;
use std::sync::{Mutex, OnceLock};

/// Records a deprecation that fired, by its id, and returns whether the
/// agent session running this command was told about it already, in which
/// case no note is printed.
pub type Recorder = fn(id: &str) -> bool;

static RECORDER: OnceLock<Recorder> = OnceLock::new();
static MODE: OnceLock<Mode> = OnceLock::new();
static WARNED: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

/// Sets the [`Recorder`]. Only the first call counts.
pub fn set_recorder(recorder: Recorder) {
    let _ = RECORDER.set(recorder);
}

/// The display a note is written for, once [`crate::Output`] has chosen it.
pub(crate) fn set_mode(mode: Mode) {
    let _ = MODE.set(mode);
}

/// Warns about the deprecation `id`, unless this process or this agent
/// session already did. Called by [`deprecated_at!`](crate::deprecated_at).
#[doc(hidden)]
pub fn warn(id: &'static str, remove_at: &'static str, message: &str) {
    let mode = *MODE.get_or_init(|| Mode::detect(None).unwrap_or(Mode::Plain));
    if let Some(note) = take(id, remove_at, message, mode, RECORDER.get().copied()) {
        eprintln!("{note}");
    }
}

/// The note to print for `id`, or None when it was printed before.
fn take(
    id: &'static str,
    remove_at: &str,
    message: &str,
    mode: Mode,
    recorder: Option<Recorder>,
) -> Option<String> {
    if !WARNED.lock().unwrap_or_else(|e| e.into_inner()).insert(id) {
        return None;
    }
    if recorder.is_some_and(|record| record(id)) {
        return None;
    }
    Some(note(id, remove_at, message, mode))
}

/// One line in every display: a warning for a person, a short note for an
/// agent.
fn note(id: &str, remove_at: &str, message: &str, mode: Mode) -> String {
    let message = message.trim().trim_end_matches('.');
    match mode {
        Mode::Human => format!(
            "jouet: warning: deprecated [{id}]: {message}. It will be removed in jouet {remove_at}."
        ),
        Mode::Agent | Mode::Plain => {
            format!("jouet: deprecated [{id}]: {message} (removed in jouet {remove_at})")
        }
    }
}

/// Whether jouet version `a` precedes `b`, comparing their dot-separated
/// numbers in order; a missing number counts as 0. A pre-release or build
/// suffix (`-rc.1`, `+abc`) is ignored, so `1.0.0-rc.1` counts as `1.0.0`
/// and a removal is due on the release candidate. Runs at compile time,
/// where a version that is not numbers and dots fails the build.
pub const fn version_lt(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (a_end, b_end) = (release_end(a), release_end(b));
    let (mut i, mut j) = (0, 0);
    while i < a_end || j < b_end {
        let (mut x, mut y) = (0u64, 0u64);
        while i < a_end && a[i] != b'.' {
            x = x * 10 + digit(a[i]);
            i += 1;
        }
        while j < b_end && b[j] != b'.' {
            y = y * 10 + digit(b[j]);
            j += 1;
        }
        if x != y {
            return x < y;
        }
        i += 1;
        j += 1;
    }
    false
}

/// Where the `MAJOR.MINOR.PATCH` part of a version ends.
const fn release_end(v: &[u8]) -> usize {
    let mut end = 0;
    while end < v.len() && v[end] != b'-' && v[end] != b'+' {
        end += 1;
    }
    end
}

const fn digit(c: u8) -> u64 {
    assert!(
        c.is_ascii_digit(),
        "a jouet version is numbers separated by dots"
    );
    (c - b'0') as u64
}

/// Warns that something is deprecated, from jouet `warn_at` on, and fails
/// a debug build's run of this path once jouet reaches `remove_at`, so the
/// release that gets there removes it. The note names `remove_at`.
///
/// `id` names the deprecation in the ledger and in the note: kebab-case,
/// stable, and with no values in it. The message is a format string, and
/// says what to use instead.
///
/// ```ignore
/// fn read_old_spelling() {
///     jouet_output::deprecated_at!(
///         "0.0.1",
///         "0.2.0",
///         "wait-ci-interval",
///         "`--poll` is now `--interval`"
///     );
/// }
/// ```
#[macro_export]
macro_rules! deprecated_at {
    ($warn_at:literal, $remove_at:literal, $id:literal, $($arg:tt)+) => {
        $crate::deprecated_at!(@at env!("CARGO_PKG_VERSION"); $warn_at, $remove_at, $id, $($arg)+)
    };
    (@at $version:expr; $warn_at:literal, $remove_at:literal, $id:literal, $($arg:tt)+) => {{
        const _: () = assert!(
            $crate::deprecation::version_lt($warn_at, $remove_at),
            concat!("deprecated_at! [", $id, "]: warn_at must come before remove_at")
        );
        debug_assert!(
            $crate::deprecation::version_lt($version, $remove_at),
            concat!(
                "[", $id, "] should have been removed in jouet ", $remove_at,
                ". Remove it, and every other deprecated_at! with this id."
            )
        );
        if !$crate::deprecation::version_lt($version, $warn_at) {
            $crate::deprecation::warn($id, $remove_at, &format!($($arg)+));
        }
    }};
}

/// Fails the build once jouet's own version reaches `remove_at`, for code
/// that only has to live for a while, such as a fallback that reads what an
/// older jouet wrote. Unlike [`deprecated_at!`], it needs no code path to
/// run: the release that bumps the version cannot build until the code
/// carrying it is removed.
///
/// ```ignore
/// fn load() {
///     // A state file an older jouet wrote has no `failures`.
///     jouet_output::remove_by!("0.2.0", "state-without-failures");
/// }
/// ```
#[macro_export]
macro_rules! remove_by {
    ($remove_at:literal, $id:literal) => {
        const _: () = assert!(
            $crate::deprecation::version_lt(env!("CARGO_PKG_VERSION"), $remove_at),
            concat!(
                "[",
                $id,
                "] should have been removed by jouet ",
                $remove_at,
                ". Remove it, and every other remove_by! with this id."
            )
        );
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_lt_compares_each_number() {
        assert!(version_lt("0.0.1", "0.0.2"));
        assert!(version_lt("0.9.9", "0.10.0"));
        assert!(version_lt("1.2", "1.2.1"));
        assert!(version_lt("0.9.0", "1.0.0-rc.1"));
        assert!(!version_lt("1.0.0", "1.0.0"));
        assert!(!version_lt("1.0", "1.0.0"));
        assert!(!version_lt("1.0.0-rc.1", "1.0.0"));
        assert!(!version_lt("2.0.0", "1.9.9"));
        assert!(!version_lt("1.0.0+build.5", "1.0.0"));
    }

    #[test]
    #[should_panic(expected = "numbers separated by dots")]
    fn a_version_must_be_numbers() {
        version_lt("v1.0", "2.0");
    }

    #[test]
    fn the_note_is_one_line_for_each_reader() {
        let human = note(
            "old-flag",
            "0.2.0",
            "`--poll` is now `--interval`.",
            Mode::Human,
        );
        assert_eq!(
            human,
            "jouet: warning: deprecated [old-flag]: `--poll` is now `--interval`. \
             It will be removed in jouet 0.2.0."
        );
        let agent = note(
            "old-flag",
            "0.2.0",
            "`--poll` is now `--interval`",
            Mode::Agent,
        );
        assert_eq!(
            agent,
            "jouet: deprecated [old-flag]: `--poll` is now `--interval` (removed in jouet 0.2.0)"
        );
        assert!(!human.contains('\n') && !agent.contains('\n'));
    }

    #[test]
    fn warns_once_per_id() {
        assert!(take("test-once", "0.2.0", "m", Mode::Agent, None).is_some());
        assert!(take("test-once", "0.2.0", "m", Mode::Agent, None).is_none());
        assert!(take("test-once-other", "0.2.0", "m", Mode::Agent, None).is_some());
    }

    #[test]
    fn a_session_told_already_is_not_told_again() {
        fn told(_: &str) -> bool {
            true
        }
        fn new(_: &str) -> bool {
            false
        }
        assert!(take("test-told", "0.2.0", "m", Mode::Agent, Some(told)).is_none());
        assert!(take("test-new", "0.2.0", "m", Mode::Agent, Some(new)).is_some());
    }

    fn warned(id: &str) -> bool {
        WARNED.lock().unwrap().contains(id)
    }

    #[test]
    fn deprecated_at_warns_from_warn_at() {
        crate::deprecated_at!(@at "0.1.9"; "0.2.0", "0.3.0", "test-early", "not yet");
        assert!(!warned("test-early"));
        crate::deprecated_at!(@at "0.2.0"; "0.2.0", "0.3.0", "test-due", "use {} instead", "x");
        assert!(warned("test-due"));
    }

    #[test]
    #[should_panic(expected = "[test-overdue] should have been removed in jouet 0.3.0")]
    fn deprecated_at_fails_a_debug_build_at_remove_at() {
        crate::deprecated_at!(@at "0.3.0"; "0.2.0", "0.3.0", "test-overdue", "gone");
    }

    #[test]
    fn remove_by_passes_before_its_version() {
        crate::remove_by!("999.0.0", "test-remove-by");
    }
}
