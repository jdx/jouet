//! Read-only discovery for an agent-driven scheduled review.

use jouet_audit::sessions::Session;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

const MAX_TRANSCRIPT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_INSPECTION_BYTES: u64 = 256 * 1024 * 1024;
const MAX_META_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Serialize)]
pub(crate) struct Candidate {
    pub session: String,
    pub project: PathBuf,
    pub transcript: PathBuf,
    pub modified_ms: i64,
    /// Identifies this version of an append-only transcript; a later append
    /// changes its length and makes it a different candidate.
    pub snapshot: String,
    #[serde(skip)]
    key: String,
    #[serde(skip)]
    bytes: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct Plan {
    pub candidates: Vec<Candidate>,
    pub eligible: usize,
    pub left_out: usize,
    pub idle_or_current: usize,
    pub unavailable: usize,
    pub reviewed: usize,
    pub claimed: usize,
    pub mixed_project: usize,
    pub inspection_limited: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Scheduled {
    #[serde(default)]
    receipts: BTreeMap<String, String>,
    #[serde(default)]
    claims: BTreeMap<String, Claim>,
}

impl Scheduled {
    pub(crate) fn load(paths: &super::Paths) -> std::io::Result<Self> {
        let path = paths.state.with_file_name("wind-scheduled.json");
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
}

/// Separate from wind.json so older jouet versions cannot erase receipts.
/// Refuse to overwrite corrupted state.
fn update<T>(
    paths: &super::Paths,
    change: impl FnOnce(&mut Scheduled) -> Result<T, String>,
) -> Result<T, String> {
    let dir = paths.state.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("wind-scheduled.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let mut state =
        Scheduled::load(paths).map_err(|e| format!("reading scheduled review state: {e}"))?;
    let out = change(&mut state)?;
    let mut file = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut file, &state).map_err(|e| e.to_string())?;
    file.persist(dir.join("wind-scheduled.json"))
        .map_err(|e| e.error.to_string())?;
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Claim {
    pub session: String,
    pub project: PathBuf,
    pub transcript: PathBuf,
    pub snapshot: String,
    pub lease: String,
    pub expires: u64,
}

/// Read an active claim without changing it. Finishing checks it again under
/// the lock, since a review may take long enough for the lease to expire.
pub(crate) fn active_claim(paths: &super::Paths, lease: &str) -> Result<Claim, String> {
    let state = Scheduled::load(paths).map_err(|e| e.to_string())?;
    let (key, claim) = state
        .claims
        .iter()
        .find(|(_, c)| c.lease == lease)
        .ok_or_else(|| "no scheduled review has that lease".to_string())?;
    if claim.expires <= super::now() || !claim_unchanged(key, claim) {
        return Err("the lease expired or the transcript changed; plan and claim again".into());
    }
    Ok(claim.clone())
}

/// Candidates from opted-in roots only. A project's oldest session is chosen
/// before its next one, so a busy project cannot consume the whole batch.
pub(crate) fn plan(
    mut sessions: Vec<Session>,
    roots: &[PathBuf],
    now_ms: i64,
    idle_ms: i64,
    selection: (usize, Option<&str>),
    current: Option<&str>,
    state: &Scheduled,
) -> Plan {
    let mut groups: BTreeMap<PathBuf, Vec<Candidate>> = BTreeMap::new();
    let mut idle_or_current = 0;
    let mut unavailable = 0;
    let mut reviewed = 0;
    let mut claimed = 0;
    let mut mixed_project = 0;
    let mut inspection_limited = 0;
    let mut inspected_bytes = 0_u64;
    sessions.sort_by(|a, b| {
        a.modified_ms
            .cmp(&b.modified_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    for session in sessions {
        let Ok((id, cwd)) = origin(&session.transcript) else {
            unavailable += 1;
            continue;
        };
        if id != session.id {
            unavailable += 1;
            continue;
        }
        let Some(project) = jouet_output::paths::work_tree(Path::new(&cwd)) else {
            if roots.iter().any(|root| Path::new(&cwd).starts_with(root)) {
                unavailable += 1;
            }
            continue;
        };
        if !roots.iter().any(|root| project.starts_with(root)) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&session.transcript) else {
            unavailable += 1;
            continue;
        };
        let Ok(modified) = metadata.modified().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map_err(std::io::Error::other)
        }) else {
            unavailable += 1;
            continue;
        };
        let modified_ms = i64::try_from(modified.as_millis()).unwrap_or(i64::MAX);
        if current == Some(session.id.as_str()) || now_ms.saturating_sub(modified_ms) < idle_ms {
            idle_or_current += 1;
            continue;
        }
        let identity = format!("{}\0{}\0{}", session.harness, session.id, project.display());
        let key = digest(&identity);
        let legacy_key = digest(&format!("{identity}\0{}", session.transcript.display()));
        let legacy_snapshot = snapshot(&legacy_key, metadata.len(), modified.as_nanos());
        let snapshot = snapshot(&key, metadata.len(), modified.as_nanos());
        // Inspect a claim's requested snapshot without spending its budget on
        // unrelated sessions that may have appeared since the plan.
        if selection.1.is_some_and(|wanted| wanted != snapshot) {
            continue;
        }
        // Older receipts included the transcript path in the key. Honour them
        // while that path remains, while new keys survive a plain-file move.
        if state.receipts.get(&key) == Some(&snapshot)
            || state.receipts.get(&legacy_key) == Some(&legacy_snapshot)
        {
            reviewed += 1;
            continue;
        }
        let now = u64::try_from(now_ms.max(0) / 1000).unwrap_or(0);
        if state.claims.values().any(|claim| {
            claim.session == session.id && claim.project == project && claim.expires > now
        }) {
            claimed += 1;
            continue;
        }
        groups.entry(project.clone()).or_default().push(Candidate {
            session: session.id,
            project,
            transcript: session.transcript,
            modified_ms,
            snapshot,
            key,
            bytes: metadata.len(),
        });
    }
    let mut queues: Vec<VecDeque<Candidate>> = groups
        .into_values()
        .map(|mut group| {
            group.sort_by(|a, b| {
                a.modified_ms
                    .cmp(&b.modified_ms)
                    .then_with(|| a.session.cmp(&b.session))
            });
            group.into()
        })
        .collect();
    if !queues.is_empty() {
        let day = u64::try_from(now_ms.max(0) / 86_400_000).unwrap_or(0);
        let start = usize::try_from(day % queues.len() as u64).unwrap_or(0);
        queues.rotate_left(start);
    }
    let mut candidates = Vec::new();
    let mut eligible = 0;
    loop {
        let mut added = false;
        for queue in &mut queues {
            if let Some(candidate) = queue.pop_front() {
                added = true;
                if candidate.bytes > MAX_TRANSCRIPT_BYTES || inspected_bytes >= MAX_INSPECTION_BYTES
                {
                    inspection_limited += 1;
                    continue;
                }
                let limit = MAX_TRANSCRIPT_BYTES.min(MAX_INSPECTION_BYTES - inspected_bytes);
                let (result, bytes) = one_project(&candidate.transcript, &candidate.project, limit);
                inspected_bytes = inspected_bytes.saturating_add(bytes);
                match result {
                    Ok(Some(true)) => {}
                    Ok(Some(false)) => {
                        mixed_project += 1;
                        continue;
                    }
                    Ok(None) => {
                        inspection_limited += 1;
                        continue;
                    }
                    Err(_) => {
                        unavailable += 1;
                        continue;
                    }
                }
                if !unchanged(&candidate) {
                    idle_or_current += 1;
                    continue;
                }
                eligible += 1;
                if candidates.len() < selection.0 {
                    candidates.push(candidate);
                }
            }
        }
        if !added {
            break;
        }
    }
    let left_out = eligible - candidates.len();
    Plan {
        candidates,
        eligible,
        left_out,
        idle_or_current,
        unavailable,
        reviewed,
        claimed,
        mixed_project,
        inspection_limited,
    }
}

/// Codex's initial project, which is also what `wind review` reads. Its
/// session listing uses the latest cwd instead, so it cannot identify this.
fn origin(path: &Path) -> std::io::Result<(String, String)> {
    let mut first = String::new();
    jouet_audit::open(path)?
        .take(MAX_META_BYTES)
        .read_line(&mut first)?;
    let value: serde_json::Value = serde_json::from_str(&first).map_err(std::io::Error::other)?;
    if value["type"] != "session_meta" {
        return Err(std::io::Error::other("missing session metadata"));
    }
    let payload = &value["payload"];
    let id = payload["id"]
        .as_str()
        .or_else(|| payload["session_id"].as_str())
        .ok_or_else(|| std::io::Error::other("missing session id"))?;
    let cwd = payload["cwd"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("missing session cwd"))?;
    Ok((id.to_owned(), cwd.to_owned()))
}

/// Reads at most one byte past the remaining decoded-byte budget, including
/// for compressed archives. An over-limit file is never treated as reviewed.
fn one_project(path: &Path, expected: &Path, limit: u64) -> (std::io::Result<Option<bool>>, u64) {
    let mut input = match jouet_audit::open(path) {
        Ok(input) => input.take(limit.saturating_add(1)),
        Err(e) => return (Err(e), 0),
    };
    let mut content = Vec::new();
    let read = input.read_to_end(&mut content);
    let bytes = content.len() as u64;
    if let Err(e) = read {
        return (Err(e), bytes);
    }
    if bytes > limit {
        return (Ok(None), bytes);
    }
    let result = (|| {
        let mut turn_cwd = None;
        let mut uncertain = false;
        for line in content.split(|&byte| byte == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_slice(line).map_err(std::io::Error::other)?;
            if value["type"] == "turn_context" {
                let cwd = value["payload"]["cwd"]
                    .as_str()
                    .ok_or_else(|| std::io::Error::other("turn context has no cwd"))?;
                if jouet_output::paths::work_tree(Path::new(cwd)).as_deref() != Some(expected) {
                    return Ok(Some(false));
                }
                turn_cwd = Some(PathBuf::from(cwd));
            }
            let payload = &value["payload"];
            if value["type"] != "response_item"
                || payload["type"] != "function_call"
                || !matches!(
                    payload["name"].as_str(),
                    Some("exec_command" | "functions.exec_command")
                )
            {
                continue;
            }
            let arguments = match &payload["arguments"] {
                serde_json::Value::String(text) => serde_json::from_str(text).ok(),
                serde_json::Value::Object(_) => Some(payload["arguments"].clone()),
                _ => None,
            };
            let Some(arguments) = arguments else {
                uncertain = true;
                continue;
            };
            if !arguments.is_object() {
                uncertain = true;
                continue;
            }
            let Some(workdir) = arguments.get("workdir") else {
                continue;
            };
            if workdir.is_null() {
                continue;
            }
            let Some(workdir) = workdir.as_str().filter(|path| !path.is_empty()) else {
                uncertain = true;
                continue;
            };
            let path = Path::new(workdir);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else if let Some(cwd) = &turn_cwd {
                cwd.join(path)
            } else {
                uncertain = true;
                continue;
            };
            if !path.is_dir() {
                uncertain = true;
                continue;
            }
            if let Some(project) = jouet_output::paths::work_tree(&path)
                && project != expected
            {
                return Ok(Some(false));
            }
        }
        Ok((!uncertain).then_some(true))
    })();
    (result, bytes)
}

fn snapshot(key: &str, len: u64, modified_ns: u128) -> String {
    digest(&format!("{key}\0{len}\0{modified_ns}"))
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Acquire one snapshot under the scheduled state lock. An expired
/// lease is retryable; finishing must still verify that the file is unchanged.
pub(crate) fn claim(
    paths: &super::Paths,
    candidate: &Candidate,
    now: u64,
) -> Result<Claim, String> {
    update(paths, |scheduled| {
        if scheduled.receipts.get(&candidate.key) == Some(&candidate.snapshot) {
            return Err("this snapshot is already reviewed".into());
        }
        if scheduled.claims.values().any(|claim| {
            claim.session == candidate.session
                && claim.project == candidate.project
                && claim.expires > now
        }) {
            return Err("this session is already claimed".into());
        }
        if !unchanged(candidate) {
            return Err("the transcript changed; plan it again".into());
        }
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let lease: String = Sha256::digest(
            format!("{}\0{}\0{nonce}", candidate.snapshot, std::process::id()).as_bytes(),
        )
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
        let claim = Claim {
            session: candidate.session.clone(),
            project: candidate.project.clone(),
            transcript: candidate.transcript.clone(),
            snapshot: candidate.snapshot.clone(),
            lease,
            expires: now.saturating_add(60 * 60),
        };
        scheduled.claims.retain(|_, old| old.expires > now);
        scheduled
            .claims
            .insert(candidate.key.clone(), claim.clone());
        Ok(claim)
    })
}

fn claim_unchanged(key: &str, claim: &Claim) -> bool {
    let Ok(metadata) = std::fs::metadata(&claim.transcript) else {
        return false;
    };
    let Ok(modified) = metadata.modified().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)
    }) else {
        return false;
    };
    snapshot(key, metadata.len(), modified.as_nanos()) == claim.snapshot
        && jouet_output::paths::work_tree(&claim.project).as_deref()
            == Some(claim.project.as_path())
}

/// Record a claimed review and mark the snapshot only after every proposal is
/// handled. Duplicate proposals count as handled; refused or deferred ones do not.
pub(crate) fn finish(
    paths: &super::Paths,
    lease: &str,
    reply: super::review::Reply,
    targets: &[String],
    project: Option<&str>,
) -> Result<(super::review::Recorded, bool), String> {
    if reply.proposals.is_empty() {
        return Err("a scheduled review needs proposals or an explicit `nothing`".into());
    }
    let (key, claim) = update(paths, |state| {
        let Some((key, claim)) = state
            .claims
            .iter()
            .find(|(_, c)| c.lease == lease)
            .map(|(k, c)| (k.clone(), c.clone()))
        else {
            return Err("no scheduled review has that lease".to_string());
        };
        if claim.expires <= super::now() {
            return Err("the lease expired; plan and claim again".into());
        }
        if reply.session != claim.session {
            return Err("the reply's session does not match the lease".into());
        }
        let Some(repo) = jouet_output::paths::repo_key(&claim.project) else {
            return Err("the claimed project is unavailable".into());
        };
        if project.is_some_and(|p| p != repo) {
            return Err("--project does not match the lease".into());
        }
        if !claim_unchanged(&key, &claim) {
            return Err(
                "the transcript or project changed; release the lease and plan again".into(),
            );
        }
        Ok((key, claim))
    })?;
    let repo = jouet_output::paths::repo_key(&claim.project)
        .ok_or_else(|| "the claimed project is unavailable".to_string())?;
    // These stores are separate. If interrupted between them, a retry sees
    // duplicate proposals and can still record the receipt without loss.
    let out = super::review::record(paths, reply, super::now(), targets, Some(&repo))
        .map_err(|e| format!("recording the review: {e}; the snapshot is not marked complete"))?;
    let handled = out.skipped.iter().all(|s| s.reason == "already proposed");
    if handled {
        update(paths, |state| {
            let Some(current) = state.claims.get(&key) else {
                return Err("the lease is no longer active".to_string());
            };
            if current.lease != lease
                || current.expires <= super::now()
                || !claim_unchanged(&key, &claim)
            {
                return Err(
                    "the lease or transcript changed; the review is not marked complete".into(),
                );
            }
            if !state.receipts.contains_key(&key) && state.receipts.len() >= 100_000 {
                return Err(
                    "scheduled review receipt storage is full; the review is not marked complete"
                        .into(),
                );
            }
            state.receipts.insert(key.clone(), claim.snapshot);
            state.claims.remove(&key);
            Ok(())
        })?;
    }
    Ok((out, handled))
}

pub(crate) fn release(paths: &super::Paths, lease: &str) -> Result<bool, String> {
    update(paths, |state| {
        let key = state
            .claims
            .iter()
            .find_map(|(key, claim)| (claim.lease == lease).then(|| key.clone()));
        Ok(key.is_some_and(|key| state.claims.remove(&key).is_some()))
    })
}

fn unchanged(candidate: &Candidate) -> bool {
    let Ok(metadata) = std::fs::metadata(&candidate.transcript) else {
        return false;
    };
    let Ok(modified) = metadata.modified().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_err(std::io::Error::other)
    }) else {
        return false;
    };
    snapshot(&candidate.key, metadata.len(), modified.as_nanos()) == candidate.snapshot
        && jouet_output::paths::work_tree(&candidate.project).as_deref()
            == Some(candidate.project.as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_session(path: &Path, project: &Path, id: &str) {
        let meta = serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":project}});
        std::fs::write(path, format!("{meta}\n")).unwrap();
    }

    #[test]
    fn compressed_archives_are_inspected_and_count_decoded_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let transcript = root.join("rollout-test.jsonl.zst");
        let meta =
            serde_json::json!({"type":"session_meta","payload":{"id":"archived","cwd":project}});
        let turn = serde_json::json!({"type":"turn_context","payload":{"cwd":project}});
        let contents = format!("{meta}\n{turn}\n");
        std::fs::write(
            &transcript,
            zstd::stream::encode_all(contents.as_bytes(), 0).unwrap(),
        )
        .unwrap();
        assert_eq!(origin(&transcript).unwrap().0, "archived");
        let (result, bytes) = one_project(&transcript, &project, 10);
        assert_eq!(result.unwrap(), None);
        assert_eq!(bytes, 11);
        let (result, bytes) = one_project(&transcript, &project, 4096);
        assert_eq!(result.unwrap(), Some(true));
        assert_eq!(bytes, contents.len() as u64);
        let session = Session {
            harness: "codex",
            id: "archived".into(),
            cwd: None,
            branch: None,
            modified_ms: 0,
            transcript,
        };
        let now = i64::try_from(super::super::now() + 1).unwrap() * 1000;
        let found = plan(
            vec![session],
            &[root],
            now,
            0,
            (1, None),
            None,
            &Scheduled::default(),
        );
        assert_eq!(found.eligible, 1);
        assert_eq!(found.candidates[0].session, "archived");
    }

    #[test]
    fn legacy_path_keyed_receipts_and_claims_remain_effective() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let transcript = root.join("session.jsonl");
        write_session(&transcript, &project, "legacy");
        let session = Session {
            harness: "codex",
            id: "legacy".into(),
            cwd: None,
            branch: None,
            modified_ms: 0,
            transcript: transcript.clone(),
        };
        let now = super::super::now();
        let identity = format!(
            "codex\0legacy\0{}\0{}",
            project.display(),
            transcript.display()
        );
        let key = digest(&identity);
        let metadata = std::fs::metadata(&transcript).unwrap();
        let modified = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let old_snapshot = snapshot(&key, metadata.len(), modified.as_nanos());
        let mut state = Scheduled::default();
        state.receipts.insert(key.clone(), old_snapshot.clone());
        let make_plan = |state: &Scheduled| {
            plan(
                vec![session.clone()],
                std::slice::from_ref(&root),
                i64::try_from(now + 60).unwrap() * 1000,
                0,
                (10, None),
                None,
                state,
            )
        };
        assert_eq!(make_plan(&state).reviewed, 1);
        state.receipts.clear();
        state.claims.insert(
            key,
            Claim {
                session: "legacy".into(),
                project,
                transcript,
                snapshot: old_snapshot,
                lease: "old-lease".into(),
                expires: now + 3600,
            },
        );
        assert_eq!(make_plan(&state).claimed, 1);
    }

    #[test]
    fn plan_interleaves_allowed_projects_and_changes_snapshot_after_append() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project_a = root.join("a");
        let project_b = root.join("b");
        for project in [&project_a, &project_b] {
            std::fs::create_dir_all(project.join(".git")).unwrap();
        }
        let session = |project: &Path, id: &str| {
            let transcript = root.join(format!("{id}.jsonl"));
            write_session(&transcript, project, id);
            Session {
                harness: "codex",
                id: id.into(),
                cwd: Some(project.display().to_string()),
                branch: None,
                modified_ms: 0,
                transcript,
            }
        };
        let sessions = vec![
            session(&project_a, "a1"),
            session(&project_a, "a2"),
            session(&project_b, "b1"),
        ];
        let now = jouet_output::time::now()
            .saturating_add(3600)
            .saturating_mul(1000);
        let first = plan(
            sessions.clone(),
            std::slice::from_ref(&root),
            now,
            900_000,
            (2, None),
            None,
            &Scheduled::default(),
        );
        assert_eq!(first.eligible, 3);
        assert_eq!(first.left_out, 1);
        assert_ne!(first.candidates[0].project, first.candidates[1].project);
        let wanted = &first.candidates[1].snapshot;
        let target = plan(
            sessions.clone(),
            std::slice::from_ref(&root),
            now,
            900_000,
            (1, Some(wanted)),
            None,
            &Scheduled::default(),
        );
        assert_eq!(target.eligible, 1);
        assert_eq!(&target.candidates[0].snapshot, wanted);
        let snapshot = first.candidates[0].snapshot.clone();
        let transcript = &first.candidates[0].transcript;
        use std::io::Write as _;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(transcript)
                .unwrap(),
            "{{\"type\":\"event_msg\",\"payload\":{{}}}}"
        )
        .unwrap();
        let next = plan(
            sessions,
            std::slice::from_ref(&root),
            now,
            900_000,
            (3, None),
            None,
            &Scheduled::default(),
        );
        let changed = next
            .candidates
            .iter()
            .find(|c| &c.transcript == transcript)
            .unwrap();
        assert_ne!(snapshot, changed.snapshot);
    }

    #[test]
    fn project_inspection_ignores_blank_lines_but_refuses_malformed_records() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(&transcript, "\n  \n{\"type\":\"event_msg\"}\n").unwrap();
        assert_eq!(
            one_project(&transcript, dir.path(), 1024).0.unwrap(),
            Some(true)
        );
        std::fs::write(&transcript, "{\"type\":\"turn_context\"\n").unwrap();
        assert!(one_project(&transcript, dir.path(), 1024).0.is_err());
        std::fs::write(&transcript, "{\"type\":\"turn_context\",\"payload\":{}}\n").unwrap();
        assert!(one_project(&transcript, dir.path(), 1024).0.is_err());
    }

    #[test]
    fn project_inspection_checks_structured_command_workdirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let a = root.join("a");
        let b = root.join("b");
        for project in [&a, &b] {
            std::fs::create_dir_all(project.join(".git")).unwrap();
        }
        let transcript = root.join("session.jsonl");
        let turn = serde_json::json!({"type":"turn_context", "payload":{"cwd":a}});
        let command = |workdir: &str| {
            serde_json::json!({"type":"response_item", "payload":{
                "type":"function_call", "name":"exec_command",
                "arguments":serde_json::json!({"cmd":"pwd", "workdir":workdir}).to_string()
            }})
        };
        std::fs::write(&transcript, format!("{turn}\n{}\n", command("."))).unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), Some(true));
        std::fs::write(&transcript, format!("{turn}\n{}\n", command("../b"))).unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), Some(false));
        let object_command = serde_json::json!({"type":"response_item", "payload":{
            "type":"function_call", "name":"exec_command",
            "arguments":{"cmd":"pwd", "workdir":b}
        }});
        std::fs::write(&transcript, format!("{turn}\n{object_command}\n")).unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), Some(false));
        std::fs::write(&transcript, format!("{turn}\n{}\n", command("../removed"))).unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), None);
        std::fs::write(
            &transcript,
            format!("{turn}\n{}\n", command(root.to_str().unwrap())),
        )
        .unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), Some(true));
        let malformed = serde_json::json!({"type":"response_item", "payload":{
            "type":"function_call", "name":"exec_command", "arguments":"not json"
        }});
        std::fs::write(&transcript, format!("{turn}\n{malformed}\n")).unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), None);
        std::fs::write(&transcript, format!("{}\n", command("."))).unwrap();
        assert_eq!(one_project(&transcript, &a, 8192).0.unwrap(), None);
    }

    #[test]
    fn plan_excludes_other_roots_current_sessions_and_recent_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let allowed = root.join("allowed");
        let other = root.join("other");
        for project in [&allowed, &other] {
            std::fs::create_dir_all(project.join(".git")).unwrap();
        }
        let session = |project: &Path, id: &str| {
            let transcript = root.join(format!("{id}.jsonl"));
            write_session(&transcript, project, id);
            Session {
                harness: "codex",
                id: id.into(),
                cwd: Some(project.display().to_string()),
                branch: None,
                modified_ms: 0,
                transcript,
            }
        };
        let sessions = vec![
            session(&allowed, "current"),
            session(&allowed, "recent"),
            session(&other, "other"),
        ];
        let now = jouet_output::time::now()
            .saturating_add(1)
            .saturating_mul(1000);
        let result = plan(
            sessions,
            std::slice::from_ref(&allowed),
            now,
            900_000,
            (10, None),
            Some("current"),
            &Scheduled::default(),
        );
        assert_eq!(result.eligible, 0);
        assert_eq!(result.idle_or_current, 2);
        assert!(result.candidates.is_empty());
    }

    #[test]
    fn plan_uses_the_original_project_and_rejects_a_session_spanning_projects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let a = root.join("a");
        let b = root.join("b");
        for project in [&a, &b] {
            std::fs::create_dir_all(project.join(".git")).unwrap();
        }
        let transcript = root.join("session.jsonl");
        write_session(&transcript, &a, "s1");
        let session = Session {
            harness: "codex",
            id: "s1".into(),
            cwd: Some(b.display().to_string()),
            branch: None,
            modified_ms: 0,
            transcript: transcript.clone(),
        };
        let now = jouet_output::time::now()
            .saturating_add(3600)
            .saturating_mul(1000);
        let first = plan(
            vec![session.clone()],
            std::slice::from_ref(&root),
            now,
            0,
            (10, None),
            None,
            &Scheduled::default(),
        );
        assert_eq!(first.candidates[0].project, a);
        use std::io::Write as _;
        let turn = serde_json::json!({"type":"turn_context","payload":{"cwd":b}});
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&transcript)
                .unwrap(),
            "{turn}"
        )
        .unwrap();
        let next = plan(
            vec![session],
            std::slice::from_ref(&root),
            now,
            0,
            (10, None),
            None,
            &Scheduled::default(),
        );
        assert!(next.candidates.is_empty());
        assert_eq!(next.mixed_project, 1);
    }

    #[test]
    fn claims_are_exclusive_and_only_unchanged_completed_snapshots_are_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let transcript = root.join("session.jsonl");
        write_session(&transcript, &project, "session-one");
        let session = Session {
            harness: "codex",
            id: "session-one".into(),
            cwd: Some(project.display().to_string()),
            branch: None,
            modified_ms: 0,
            transcript: transcript.clone(),
        };
        let paths = super::super::Paths {
            ledger: root.join("ledger"),
            state: root.join("wind.json"),
            rules: root.join("rules"),
            log: root.join("rules.log"),
        };
        let now = super::super::now();
        let make_plan = || {
            plan(
                vec![session.clone()],
                std::slice::from_ref(&root),
                i64::try_from(now + 60).unwrap() * 1000,
                0,
                (10, None),
                None,
                &Scheduled::load(&paths).unwrap(),
            )
        };
        let first = make_plan();
        let candidate = &first.candidates[0];
        let first_claim = claim(&paths, candidate, now).unwrap();
        assert!(claim(&paths, candidate, now).is_err());
        assert_eq!(make_plan().claimed, 1);
        let moved = root.join("archived-active.jsonl");
        std::fs::rename(&transcript, &moved).unwrap();
        let mut archived = session.clone();
        archived.transcript = moved.clone();
        let after_move = plan(
            vec![archived],
            std::slice::from_ref(&root),
            i64::try_from(now + 60).unwrap() * 1000,
            0,
            (10, None),
            None,
            &Scheduled::load(&paths).unwrap(),
        );
        assert_eq!(after_move.claimed, 1);
        std::fs::rename(&moved, &transcript).unwrap();
        let reply = |id: &str| {
            serde_json::from_value::<super::super::review::Reply>(
                serde_json::json!({"session": id, "proposals": [{"kind":"nothing"}]}),
            )
            .unwrap()
        };
        assert!(finish(&paths, &first_claim.lease, reply("wrong"), &[], None).is_err());
        use std::io::Write as _;
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&transcript)
                .unwrap(),
            "{{\"type\":\"event_msg\",\"payload\":{{}}}}"
        )
        .unwrap();
        assert!(finish(&paths, &first_claim.lease, reply("session-one"), &[], None).is_err());
        assert!(release(&paths, &first_claim.lease).unwrap());
        let second = make_plan();
        assert_eq!(second.candidates.len(), 1);
        let second_claim = claim(&paths, &second.candidates[0], now).unwrap();
        assert!(
            finish(&paths, &second_claim.lease, reply("session-one"), &[], None)
                .unwrap()
                .1
        );
        assert_eq!(make_plan().reviewed, 1);
        let moved = root.join("archived.jsonl");
        std::fs::rename(&transcript, &moved).unwrap();
        let mut archived = session.clone();
        archived.transcript = moved.clone();
        let after_move = plan(
            vec![archived],
            std::slice::from_ref(&root),
            i64::try_from(now + 60).unwrap() * 1000,
            0,
            (10, None),
            None,
            &Scheduled::load(&paths).unwrap(),
        );
        assert_eq!(after_move.reviewed, 1);
        std::fs::rename(&moved, &transcript).unwrap();
        writeln!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(&transcript)
                .unwrap(),
            "{{\"type\":\"event_msg\",\"payload\":{{}}}}"
        )
        .unwrap();
        assert_eq!(make_plan().eligible, 1);
    }

    #[test]
    fn a_refused_proposal_does_not_mark_the_snapshot_reviewed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let transcript = root.join("session.jsonl");
        write_session(&transcript, &project, "session-one");
        let session = Session {
            harness: "codex",
            id: "session-one".into(),
            cwd: Some(project.display().to_string()),
            branch: None,
            modified_ms: 0,
            transcript,
        };
        let paths = super::super::Paths {
            ledger: root.join("ledger"),
            state: root.join("wind.json"),
            rules: root.join("rules"),
            log: root.join("rules.log"),
        };
        let now = super::super::now();
        let make_plan = || {
            plan(
                vec![session.clone()],
                std::slice::from_ref(&root),
                i64::try_from(now + 60).unwrap() * 1000,
                0,
                (10, None),
                None,
                &Scheduled::load(&paths).unwrap(),
            )
        };
        let pending = make_plan();
        let lease = claim(&paths, &pending.candidates[0], now).unwrap().lease;
        let invalid = serde_json::from_value(serde_json::json!({"session":"session-one","proposals":[{"kind":"project_change","title":"Fix CI","why":"x","draft":"x"}]})).unwrap();
        let (out, complete) = finish(&paths, &lease, invalid, &[], None).unwrap();
        assert!(!complete);
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(make_plan().claimed, 1);
        let corrected = serde_json::from_value(serde_json::json!({"session":"session-one","proposals":[{"kind":"project_change","title":"Fix CI","why":"x","evidence":["L4"],"draft":"x"}]})).unwrap();
        assert!(finish(&paths, &lease, corrected, &[], None).unwrap().1);
        assert_eq!(make_plan().reviewed, 1);
    }

    #[test]
    fn corrupt_scheduled_state_is_reported_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let paths = super::super::Paths {
            ledger: root.join("ledger"),
            state: root.join("wind.json"),
            rules: root.join("rules"),
            log: root.join("rules.log"),
        };
        let scheduled = root.join("wind-scheduled.json");
        std::fs::write(&scheduled, b"not json").unwrap();
        assert!(Scheduled::load(&paths).is_err());
        assert!(release(&paths, "unknown").is_err());
        assert_eq!(std::fs::read(scheduled).unwrap(), b"not json");
    }
}
