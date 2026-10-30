//! Digests the fixture sessions in `tests/fixtures/digest`: a Claude Code
//! session with an interruption, a correction, a refused force-push, a
//! repeated instruction holding a token, failing retries, long calls,
//! re-reads and a sleep loop, and a Codex rollout with an aborted turn and
//! a command that never worked.
//!
//! The rendered digests are compared with `tests/snapshots/`; run with
//! `UPDATE_SNAPSHOTS=1` to rewrite them after a deliberate change.

use jouet_audit::digest::{self, Digest, Kind, Options};
use jouet_audit::{ClaudeCode, Codex, Filter, Reader, Source};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/digest")
}

fn read(reader: &dyn Reader) -> Digest {
    let filter = Filter {
        detail: true,
        ..Filter::default()
    };
    let sources = reader.discover(&filter).unwrap();
    assert_eq!(sources.len(), 1, "one fixture per harness");
    let source: &Source = &sources[0];
    let mut input = jouet_audit::open(&source.path).unwrap();
    let transcript = reader.read(source, &mut input, &filter).unwrap().unwrap();
    let root = fixtures().to_string_lossy().into_owned();
    let path = source.path.to_string_lossy().replace(&root, "<fixtures>");
    digest::digest(
        reader.harness(),
        &transcript,
        &path,
        &Options {
            home: None,
            redact: &jouet_audit::redact,
        },
    )
}

fn claude() -> Digest {
    read(&ClaudeCode {
        root: fixtures().join("claude"),
    })
}

fn codex() -> Digest {
    read(&Codex {
        root: fixtures().join("codex"),
    })
}

fn snapshot(name: &str, text: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_SNAPSHOTS=1", path.display()));
    assert_eq!(
        text,
        expected,
        "{} is stale; run with UPDATE_SNAPSHOTS=1 if the change is deliberate",
        path.display()
    );
}

#[test]
fn a_claude_session_digests_to_its_snapshot() {
    snapshot("claude.txt", &digest::render(&claude(), 4000));
}

#[test]
fn a_codex_session_digests_to_its_snapshot() {
    snapshot("codex.txt", &digest::render(&codex(), 4000));
}

#[test]
fn corrections_carry_their_kind_span_and_context() {
    let d = claude();
    let kinds: Vec<Kind> = d.corrections.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [Kind::Interrupt, Kind::Negation, Kind::Refusal, Kind::Repeat]
    );
    let after_interrupt = &d.corrections[1];
    assert!(after_interrupt.after_interrupt);
    assert_eq!(
        after_interrupt.after.as_deref(),
        Some("cargo test 2>&1 | tail -<n>")
    );
    assert!(after_interrupt.span.from < after_interrupt.span.to);
    let refusal = &d.corrections[2];
    assert_eq!(refusal.text, "never force-push; open a PR instead");
    // Harness text written in the user's place is no prompt.
    assert_eq!(d.totals.prompts, 4);
    assert!(d.phases.iter().all(|p| {
        !p.prompt
            .as_deref()
            .is_some_and(|t| t.contains("task-notification"))
    }));
}

#[test]
fn secrets_never_reach_the_digest() {
    let text = digest::render(&claude(), 4000);
    assert!(!text.contains("ghp_"), "{text}");
    assert!(text.contains("GH_TOKEN=<redacted>"), "{text}");
    let json = serde_json::to_string(&claude()).unwrap();
    assert!(!json.contains("ghp_"));
}

#[test]
fn a_custom_redactor_runs_on_everything_shown() {
    let filter = Filter {
        detail: true,
        ..Filter::default()
    };
    let reader = ClaudeCode {
        root: fixtures().join("claude"),
    };
    let source = &reader.discover(&filter).unwrap()[0];
    let mut input = jouet_audit::open(&source.path).unwrap();
    let t = reader.read(source, &mut input, &filter).unwrap().unwrap();
    let hide = |s: &str| s.replace("acme", "<org>").replace("lib.rs", "<file>");
    let d = digest::digest(
        "claude",
        &t,
        "x.jsonl",
        &Options {
            home: None,
            redact: &hide,
        },
    );
    let text = digest::render(&d, 4000);
    assert!(!text.contains("acme") && !text.contains("lib.rs"), "{text}");
}

#[test]
fn failures_retries_waits_and_rereads_are_found() {
    let d = codex();
    let never = d
        .failures
        .iter()
        .find(|f| f.what.starts_with("cargo xtask"))
        .expect("the failing command");
    assert_eq!(
        (never.attempts, never.failed, never.recovered),
        (2, 2, false)
    );
    assert_eq!(
        never.error.as_deref(),
        Some("error: no such command: `xtask`")
    );
    assert_eq!(d.waits.len(), 3);
    assert_eq!(d.rereads[0].file, "docs/release.md");
    assert_eq!(d.rereads[0].reads, 3);
    assert_eq!(d.corrections[0].kind, Kind::Interrupt);
    assert_eq!(d.corrections[1].kind, Kind::Stop);

    let c = claude();
    assert_eq!(c.retyped[0].runs, 3);
    let lib = c.rereads.iter().find(|r| r.file == "src/lib.rs").unwrap();
    // One read before the edit, one right after it, then two unchanged.
    assert_eq!((lib.reads, lib.unchanged), (4, 2));
}

#[test]
fn a_tight_budget_cuts_with_markers_and_keeps_the_corrections() {
    let d = claude();
    let full = digest::render(&d, 100_000);
    let tight = digest::render(&d, 300);
    assert!(tight.len() < full.len());
    assert!(tight.contains("## User corrections (4)"));
    assert!(
        tight.contains("more kinds of call")
            || tight.contains("phases left out")
            || !tight.contains("   `")
    );
}

#[test]
fn what_sessions_share_is_listed_across_them() {
    let shared = digest::across(&[claude(), claude()]);
    assert!(
        shared
            .files
            .iter()
            .any(|f| f.what == "src/lib.rs" && f.sessions == 2),
        "{shared:?}"
    );
    let text = digest::render_all(
        &[claude(), codex()],
        &digest::across(&[claude(), codex()]),
        8000,
    );
    assert!(text.contains("# Session d1d1d1d1"));
    assert!(text.contains("(codex)"));
}
