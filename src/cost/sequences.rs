//! Opt-in, local observations of successive completed `jouet test` jobs.
//! These are associations, not evidence that either job warmed a cache.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_GAP_MS: u64 = 30 * 60 * 1000;
const MIN_SAMPLES: usize = 3;
const MAX_LOG_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Start {
        id: String,
        session: String,
        at: u64,
    },
    Finish {
        id: String,
        at: u64,
        ms: u64,
        command: Option<String>,
        ok: bool,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct Observation {
    pub before: String,
    pub after: String,
    pub median_ms: u64,
    pub samples: usize,
}

pub(crate) struct Ticket {
    path: PathBuf,
    id: String,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn path(cwd: &Path) -> Option<PathBuf> {
    let repo = jouet_output::paths::repo_key(cwd)?;
    Some(
        jouet_output::paths::state_dir()?
            .join("cost-sequences")
            .join(format!("{repo}.jsonl")),
    )
}

fn append(path: &Path, event: &Event) -> io::Result<()> {
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("missing parent"))?,
    )?;
    let mut opts = OpenOptions::new();
    opts.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    file.lock()?;
    if file.metadata()?.len() >= MAX_LOG_BYTES {
        return Err(io::Error::other(format!(
            "{} reached the 16 MiB limit; move or delete it to resume recording",
            path.display()
        )));
    }
    serde_json::to_writer(&mut file, event)?;
    file.write_all(b"\n")
}

/// Record a start before running the job. A missing finish remains a barrier.
pub(crate) fn begin(cwd: &Path, ordinal: usize) -> Option<Ticket> {
    let session = jouet_git::session::id_from_env()
        .or_else(|| std::env::var("CODEX_THREAD_ID").ok())
        .filter(|s| !s.trim().is_empty())?;
    let digest = Sha256::digest(session.as_bytes());
    let session: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let at = now_ms();
    if at == 0 {
        return None;
    }
    let id = format!("{session}-{}-{ordinal}-{at}", std::process::id());
    let path = path(cwd)?;
    if let Err(error) = append(
        &path,
        &Event::Start {
            id: id.clone(),
            session,
            at,
        },
    ) {
        eprintln!("jouet: could not record a cost sequence start: {error}");
        return None;
    }
    Some(Ticket { path, id })
}

impl Ticket {
    pub(crate) fn finish(self, command: Option<&str>, ms: u64, ok: bool) {
        let command = command.map(crate::redact::redact).filter(|s| plain(s));
        if let Err(error) = append(
            &self.path,
            &Event::Finish {
                id: self.id,
                at: now_ms(),
                ms,
                command,
                ok,
            },
        ) {
            eprintln!("jouet: could not record a cost sequence finish: {error}");
        }
    }
}

fn plain(command: &str) -> bool {
    !command.is_empty() && command.chars().count() <= 200 && !command.chars().any(char::is_control)
}

struct Run {
    start: u64,
    finish: Option<(u64, u64, Option<String>, bool)>,
}

/// Every pair is two adjacent recorded starts in one session, with both
/// finishes known and no overlapping recorded job. Other work is unknown.
fn summarize(events: Vec<Event>) -> io::Result<Vec<Observation>> {
    let mut starts = BTreeMap::<String, (String, Run)>::new();
    let mut finishes = BTreeMap::new();
    for event in events {
        match event {
            Event::Start { id, session, at } => {
                if starts
                    .insert(
                        id,
                        (
                            session,
                            Run {
                                start: at,
                                finish: None,
                            },
                        ),
                    )
                    .is_some()
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate sequence start",
                    ));
                }
            }
            Event::Finish {
                id,
                at,
                ms,
                command,
                ok,
            } => {
                if finishes.insert(id, (at, ms, command, ok)).is_some() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "duplicate sequence finish",
                    ));
                }
            }
        }
    }
    let mut sessions = BTreeMap::<String, Vec<Run>>::new();
    for (id, (session, mut run)) in starts {
        run.finish = finishes.remove(&id);
        sessions.entry(session).or_default().push(run);
    }
    if !finishes.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "sequence finish has no start",
        ));
    }
    let mut pairs = BTreeMap::<(String, String), Vec<u64>>::new();
    for runs in sessions.values_mut() {
        runs.sort_by_key(|r| r.start);
        let mut earlier_end = 0;
        for i in 1..runs.len() {
            let previous = &runs[i - 1];
            let current = &runs[i];
            if let (Some((prev_end, _, Some(before), true)), Some((end, ms, Some(after), true))) =
                (&previous.finish, &current.finish)
            {
                let next_start = runs.get(i + 1).map_or(u64::MAX, |r| r.start);
                if plain(before)
                    && plain(after)
                    && earlier_end < previous.start
                    && *prev_end >= previous.start
                    && *prev_end < current.start
                    && current.start.saturating_sub(*prev_end) <= MAX_GAP_MS
                    && *end >= current.start
                    && *end < next_start
                {
                    pairs
                        .entry((before.clone(), after.clone()))
                        .or_default()
                        .push(*ms);
                }
            }
            earlier_end = earlier_end.max(previous.finish.as_ref().map_or(u64::MAX, |f| f.0));
        }
    }
    Ok(pairs
        .into_iter()
        .filter_map(|((before, after), times)| {
            (times.len() >= MIN_SAMPLES).then(|| Observation {
                before,
                after,
                median_ms: super::median(&times).unwrap_or_default(),
                samples: times.len(),
            })
        })
        .collect())
}

pub(crate) fn load(cwd: &Path) -> io::Result<Vec<Observation>> {
    let Some(path) = path(cwd) else {
        return Ok(Vec::new());
    };
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    file.lock_shared()?;
    let size = file.metadata()?.len();
    if size > MAX_LOG_BYTES + 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "local cost sequence file exceeds its size limit",
        ));
    }
    if size >= MAX_LOG_BYTES {
        eprintln!(
            "jouet: local cost sequence recording stopped at 16 MiB; move or delete {} to resume",
            path.display()
        );
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let events = text
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Event>, _>>()?;
    summarize(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        events: &mut Vec<Event>,
        session: &str,
        id: &str,
        start: u64,
        end: u64,
        command: &str,
        ok: bool,
    ) {
        events.push(Event::Start {
            id: id.into(),
            session: session.into(),
            at: start,
        });
        events.push(Event::Finish {
            id: id.into(),
            at: end,
            ms: end - start,
            command: Some(command.into()),
            ok,
        });
    }

    #[test]
    fn reports_only_sufficient_adjacent_successful_samples_in_one_session() {
        let mut events = Vec::new();
        for n in 0..3 {
            let start = 100 + n * 100;
            run(
                &mut events,
                "a",
                &format!("a{n}"),
                start,
                start + 10,
                "cargo clippy",
                true,
            );
            run(
                &mut events,
                "a",
                &format!("b{n}"),
                start + 20,
                start + 25 + n,
                "cargo test",
                true,
            );
        }
        run(&mut events, "other", "other", 110, 120, "cargo test", true);
        let result = summarize(events).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(
            (
                &*result[0].before,
                &*result[0].after,
                result[0].median_ms,
                result[0].samples
            ),
            ("cargo clippy", "cargo test", 6, 3)
        );
    }

    #[test]
    fn failures_missing_finishes_and_overlaps_never_form_a_pair() {
        let mut events = Vec::new();
        for n in 0..3 {
            let failed = format!("failed{n}");
            run(
                &mut events,
                &failed,
                &format!("a{n}"),
                10,
                20,
                "clippy",
                true,
            );
            run(
                &mut events,
                &failed,
                &format!("b{n}"),
                30,
                40,
                "test",
                false,
            );
            run(&mut events, &failed, &format!("c{n}"), 50, 60, "test", true);
            let overlap = format!("overlap{n}");
            run(
                &mut events,
                &overlap,
                &format!("d{n}"),
                10,
                100,
                "clippy",
                true,
            );
            run(
                &mut events,
                &overlap,
                &format!("e{n}"),
                30,
                40,
                "test",
                true,
            );
            let incomplete = format!("incomplete{n}");
            events.push(Event::Start {
                id: format!("unfinished{n}"),
                session: incomplete.clone(),
                at: 10,
            });
            run(
                &mut events,
                &incomplete,
                &format!("f{n}"),
                30,
                40,
                "clippy",
                true,
            );
            run(
                &mut events,
                &incomplete,
                &format!("g{n}"),
                50,
                60,
                "test",
                true,
            );
            let unknown = format!("unknown{n}");
            run(
                &mut events,
                &unknown,
                &format!("h{n}"),
                10,
                20,
                "clippy",
                true,
            );
            events.push(Event::Start {
                id: format!("barrier{n}"),
                session: unknown.clone(),
                at: 30,
            });
            events.push(Event::Finish {
                id: format!("barrier{n}"),
                at: 40,
                ms: 10,
                command: None,
                ok: true,
            });
            run(
                &mut events,
                &unknown,
                &format!("i{n}"),
                50,
                60,
                "test",
                true,
            );
            let distant = format!("distant{n}");
            run(
                &mut events,
                &distant,
                &format!("j{n}"),
                10,
                20,
                "clippy",
                true,
            );
            run(
                &mut events,
                &distant,
                &format!("k{n}"),
                MAX_GAP_MS + 30,
                MAX_GAP_MS + 40,
                "test",
                true,
            );
        }
        assert!(summarize(events).unwrap().is_empty());
    }

    #[test]
    fn corrupt_or_incomplete_event_pairs_do_not_create_observations() {
        let finish = Event::Finish {
            id: "missing".into(),
            at: 20,
            ms: 10,
            command: Some("test".into()),
            ok: true,
        };
        assert_eq!(
            summarize(vec![finish]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let start = Event::Start {
            id: "duplicate".into(),
            session: "s".into(),
            at: 10,
        };
        assert_eq!(
            summarize(vec![start.clone(), start]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn recording_stops_without_truncating_a_full_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("full.jsonl");
        File::create(&path).unwrap().set_len(MAX_LOG_BYTES).unwrap();
        let event = Event::Start {
            id: "id".into(),
            session: "session".into(),
            at: 10,
        };
        assert!(append(&path, &event).is_err());
        assert_eq!(std::fs::metadata(path).unwrap().len(), MAX_LOG_BYTES);
    }
}
