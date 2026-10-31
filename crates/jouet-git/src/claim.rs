//! Advisory claims between agent sessions on one machine, for `jouet
//! claim`: a session says it is working on some paths, and the others are
//! told before they edit or commit them.
//!
//! Claims are files under `<state dir>/claims/<repository key>/`, one per
//! session and pattern. The key hashes the repository's git common
//! directory, so every worktree of a repository shares one set of claims.
//! A pattern is relative to the repository's root, with `/` separators: a
//! file, a directory (ending in `/`, covering everything under it), or a
//! glob (`*` within a directory, `**` across them, `?` one character).
//!
//! A claim holds until it expires, its worktree is removed, or the agent
//! process that made it exits. Everything is read from the filesystem, never
//! by starting git, because a hook asks on every edit.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// One claim, as stored and as `jouet claim ls --json` prints it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// What is claimed, relative to the repository's root.
    pub pattern: String,
    pub session: String,
    /// `claude`, `codex`, or the agent `AI_AGENT` names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// The agent process holding the claim; the claim lapses when it exits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// When that process started, so a reused PID does not keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_start: Option<String>,
    /// The worktree the session works in.
    pub worktree: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub claimed_at: String,
    pub expires_at: String,
}

/// Whether a claim still holds, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    Live,
    Expired,
    WorktreeGone,
    SessionGone,
}

impl Standing {
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Expired => "expired",
            Self::WorktreeGone => "worktree gone",
            Self::SessionGone => "session gone",
        }
    }
}

impl Claim {
    pub fn standing(&self, now: i64) -> Standing {
        if jouet_output::time::parse_utc(&self.expires_at).is_none_or(|t| t <= now) {
            return Standing::Expired;
        }
        if !Path::new(&self.worktree).join(".git").exists() {
            return Standing::WorktreeGone;
        }
        if let Some(pid) = self.pid
            && !agent::running(pid, self.pid_start.as_deref())
        {
            return Standing::SessionGone;
        }
        Standing::Live
    }

    /// The session, as a person reads it: the harness and the ID's first
    /// eight characters.
    pub fn who(&self) -> String {
        let id: String = self.session.chars().take(8).collect();
        match &self.harness {
            Some(h) => format!("{h} session {id}"),
            None => format!("session {id}"),
        }
    }

    /// Whether this claim covers `path`, relative to the repository's root.
    pub fn covers(&self, path: &str) -> bool {
        covers(&self.pattern, path)
    }
}

/// The session making a claim: its ID, and the agent process behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    pub session: String,
    pub harness: Option<String>,
    pub pid: Option<u32>,
}

impl Holder {
    /// The session jouet runs in: `JOUET_SESSION_ID`, else the one Claude
    /// Code (`CLAUDE_CODE_SESSION_ID`) or Codex (`CODEX_THREAD_ID`) sets for
    /// the commands it runs. The agent process is `CLAUDE_PID`, or else the
    /// nearest `codex` ancestor.
    pub fn from_env() -> Option<Self> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let (session, harness) = [
            ("JOUET_SESSION_ID", None),
            ("CLAUDE_CODE_SESSION_ID", Some("claude")),
            ("CODEX_THREAD_ID", Some("codex")),
        ]
        .into_iter()
        .find_map(|(name, harness)| Some((var(name)?, harness.map(str::to_string))))?;
        let pid = match harness.as_deref() {
            Some("claude") => var("CLAUDE_PID").and_then(|p| p.parse().ok()),
            Some("codex") => agent::ancestor("codex"),
            _ => None,
        };
        Some(Self {
            session,
            harness,
            pid,
        })
    }
}

/// The claims of one repository.
pub struct Store {
    dir: PathBuf,
    /// The root of the worktree the store was opened from.
    root: PathBuf,
}

/// Why nothing was claimed.
#[derive(Debug)]
pub enum Refused {
    /// Other sessions hold claims that overlap the ones asked for.
    Conflicts(Vec<Conflict>),
    /// The claim could not be written.
    Io(String),
}

/// A claim another session holds that overlaps one asked for.
#[derive(Debug)]
pub struct Conflict {
    pub wanted: String,
    pub held: Claim,
}

impl Store {
    /// The claims of the repository containing `cwd`, kept under `state`
    /// (`<state dir>/claims`). None outside a git work tree.
    pub fn open(state: &Path, cwd: &Path) -> Option<Self> {
        let root = jouet_output::paths::work_tree(cwd)?;
        let common = common_dir(&root)?;
        let digest = Sha256::digest(common.to_string_lossy().as_bytes());
        let key: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        Some(Self {
            dir: state.join(key),
            root,
        })
    }

    /// The worktree's root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every stored claim, with whether it still holds.
    pub fn all(&self, now: i64) -> Vec<(Claim, Standing)> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut claims: Vec<(Claim, Standing)> = entries
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| std::fs::read(e.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<Claim>(&bytes).ok())
            .map(|c| {
                let standing = c.standing(now);
                (c, standing)
            })
            .collect();
        claims.sort_by(|a, b| (&a.0.pattern, &a.0.session).cmp(&(&b.0.pattern, &b.0.session)));
        claims
    }

    /// The claims that still hold.
    pub fn live(&self, now: i64) -> Vec<Claim> {
        self.all(now)
            .into_iter()
            .filter(|(_, s)| *s == Standing::Live)
            .map(|(c, _)| c)
            .collect()
    }

    /// Removes the claims that no longer hold, and returns them.
    pub fn prune(&self, now: i64) -> Vec<(Claim, Standing)> {
        let dead: Vec<(Claim, Standing)> = self
            .all(now)
            .into_iter()
            .filter(|(_, s)| *s != Standing::Live)
            .collect();
        for (claim, _) in &dead {
            let _ = std::fs::remove_file(self.file(&claim.session, &claim.pattern));
        }
        dead
    }

    /// Claims `patterns` for `holder` until `expires` (Unix seconds), or
    /// claims nothing and returns the live claims of other sessions that
    /// overlap them. Claiming a pattern again renews it.
    pub fn claim(
        &self,
        holder: &Holder,
        patterns: &[String],
        expires: i64,
        note: Option<&str>,
        now: i64,
    ) -> Result<Vec<Claim>, Refused> {
        let others: Vec<Claim> = self
            .live(now)
            .into_iter()
            .filter(|c| c.session != holder.session)
            .collect();
        let conflicts: Vec<Conflict> = patterns
            .iter()
            .flat_map(|p| {
                others
                    .iter()
                    .filter(|c| overlaps(p, &c.pattern))
                    .map(|c| Conflict {
                        wanted: p.clone(),
                        held: c.clone(),
                    })
            })
            .collect();
        if !conflicts.is_empty() {
            return Err(Refused::Conflicts(conflicts));
        }
        let worktree = self.root.to_string_lossy().into_owned();
        let branch = branch(&self.root);
        let pid_start = holder.pid.and_then(agent::start_time);
        let claims: Vec<Claim> = patterns
            .iter()
            .map(|pattern| Claim {
                pattern: pattern.clone(),
                session: holder.session.clone(),
                harness: holder.harness.clone(),
                pid: holder.pid.filter(|_| pid_start.is_some()),
                pid_start: pid_start.clone(),
                worktree: worktree.clone(),
                branch: branch.clone(),
                note: note.map(str::to_string),
                claimed_at: jouet_output::time::format_utc(now),
                expires_at: jouet_output::time::format_utc(expires),
            })
            .collect();
        for claim in &claims {
            self.save(claim).map_err(Refused::Io)?;
        }
        Ok(claims)
    }

    fn save(&self, claim: &Claim) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("creating {}: {e}", self.dir.display()))?;
        let path = self.file(&claim.session, &claim.pattern);
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_vec_pretty(claim).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("writing {}: {e}", path.display()))
    }

    /// Releases the session's claims on `patterns`, or all of its claims
    /// when `patterns` is empty, and returns them.
    pub fn release(&self, session: &str, patterns: &[String], now: i64) -> Vec<Claim> {
        let mine: Vec<Claim> = self
            .all(now)
            .into_iter()
            .map(|(c, _)| c)
            .filter(|c| c.session == session)
            .filter(|c| patterns.is_empty() || patterns.contains(&c.pattern))
            .collect();
        for claim in &mine {
            let _ = std::fs::remove_file(self.file(&claim.session, &claim.pattern));
        }
        mine
    }

    fn file(&self, session: &str, pattern: &str) -> PathBuf {
        let digest = Sha256::digest(format!("{session}\0{pattern}").as_bytes());
        let name: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{name}.json"))
    }

    /// `arg`, a path or glob relative to `cwd` (or absolute), as a pattern
    /// relative to the repository's root. A directory that exists gets a
    /// trailing `/`.
    pub fn pattern(&self, cwd: &Path, arg: &str) -> Result<String, String> {
        let glob = arg.contains(['*', '?', '[']);
        let mut pattern = relative(&self.root, cwd, arg).ok_or_else(|| {
            format!(
                "`{arg}` is outside this repository ({})",
                self.root.display()
            )
        })?;
        if pattern.is_empty() {
            return Err(format!(
                "`{arg}` is the whole repository: claim the files or directories you work on"
            ));
        }
        if !glob && (arg.ends_with('/') || cwd.join(arg).is_dir()) {
            pattern.push('/');
        }
        Ok(pattern)
    }
}

/// `path` (relative to `cwd`, or absolute) relative to the root of the
/// worktree holding it: `root`, or another worktree of the same repository
/// nested inside it. None when it is outside them.
pub fn relative(root: &Path, cwd: &Path, path: &str) -> Option<String> {
    let resolved = resolve(&cwd.join(path));
    let holder = resolved
        .ancestors()
        .find(|dir| dir.exists())
        .and_then(jouet_output::paths::work_tree)
        .filter(|r| r.starts_with(root) && r != root)
        .filter(|r| common_dir(r).is_some() && common_dir(r) == common_dir(root));
    let rel = resolved
        .strip_prefix(holder.as_deref().unwrap_or(root))
        .ok()?;
    Some(
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    )
}

/// `path` with `.` and `..` removed and symlinks resolved as far as it
/// exists, so a file not yet created still resolves.
fn resolve(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut existing = clean.as_path();
    let mut rest = Vec::new();
    while !existing.exists() {
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            return clean;
        };
        rest.push(name.to_os_string());
        existing = parent;
    }
    let mut out = existing.canonicalize().unwrap_or_else(|_| existing.into());
    out.extend(rest.iter().rev());
    out
}

/// The repository's common git directory, from the worktree's `.git`: the
/// directory itself, or, in a linked worktree, the `commondir` its `gitdir`
/// names.
pub fn common_dir(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return git.canonicalize().ok();
    }
    let gitdir = git_dir(root)?;
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok();
    let dir = match common {
        Some(c) => gitdir.join(c.trim()),
        None => gitdir,
    };
    dir.canonicalize().ok()
}

/// The worktree's own git directory.
pub fn git_dir(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return Some(git);
    }
    let text = std::fs::read_to_string(&git).ok()?;
    let dir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    Some(root.join(dir))
}

/// The branch checked out in the worktree, from its `HEAD`.
pub fn branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir(root)?.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

/// Whether `pattern` covers `path`, both relative to the repository's root.
pub fn covers(pattern: &str, path: &str) -> bool {
    let path = path.trim_end_matches('/');
    if let Some(dir) = pattern.strip_suffix('/') {
        return path == dir || path.starts_with(pattern);
    }
    if pattern.contains(['*', '?']) {
        return wild(
            &pattern.chars().collect::<Vec<_>>(),
            &path.chars().collect::<Vec<_>>(),
        );
    }
    pattern == path
}

/// Whether some path could be covered by both patterns. For two globs this
/// compares what comes before their first wildcard, so it may say yes when
/// they share no file.
pub fn overlaps(a: &str, b: &str) -> bool {
    let literal = |p: &str| !p.contains(['*', '?']);
    match (literal(a), literal(b)) {
        (true, _) if covers(b, a) || covers(a, b) => true,
        (_, true) if covers(a, b) || covers(b, a) => true,
        (true, true) => {
            // A directory covers what is inside it, either way round.
            a.ends_with('/') && b.starts_with(a) || b.ends_with('/') && a.starts_with(b)
        }
        (false, false) => {
            let stem = |p: &str| p[..p.find(['*', '?']).unwrap_or(p.len())].to_string();
            let (x, y) = (stem(a), stem(b));
            x.starts_with(&y) || y.starts_with(&x)
        }
        // A directory and a glob inside it, or a glob that reaches into it.
        (true, false) => a.ends_with('/') && b.starts_with(a),
        (false, true) => b.ends_with('/') && a.starts_with(b),
    }
}

/// Wildcard match for paths: `*` and `?` stay within a directory, `**`
/// crosses any number of them.
fn wild(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') if p.get(1) == Some(&'*') => {
            let rest = &p[2..];
            let after_slash = rest.strip_prefix(&['/']).unwrap_or(rest);
            (0..=t.len()).any(|i| wild(rest, &t[i..]))
                || (rest.len() != after_slash.len() && wild(after_slash, t))
        }
        Some('*') => {
            for i in 0..=t.len() {
                if wild(&p[1..], &t[i..]) {
                    return true;
                }
                if t.get(i) == Some(&'/') {
                    return false;
                }
            }
            false
        }
        Some('?') => t.first().is_some_and(|c| *c != '/') && wild(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && wild(&p[1..], &t[1..]),
    }
}

/// The agent process behind a claim: whether it still runs, and finding it.
mod agent {
    use std::process::{Command, Stdio};

    /// Whether the process runs and, when a start time was recorded, is the
    /// same process. Where jouet cannot tell, it counts as running, and the
    /// claim lasts until it expires.
    pub(super) fn running(pid: u32, recorded: Option<&str>) -> bool {
        if cfg!(windows) {
            return true;
        }
        match (start_time(pid), recorded) {
            (None, _) => false,
            (Some(now), Some(then)) => now == then,
            (Some(_), None) => true,
        }
    }

    /// When the process started: clock ticks since boot on Linux, `ps`'s
    /// start time elsewhere. None when it does not run.
    #[cfg(target_os = "linux")]
    pub(super) fn start_time(pid: u32) -> Option<String> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let (_, rest) = stat.rsplit_once(')')?;
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.first() == Some(&"Z") {
            return None;
        }
        // Field 22 of stat; `rest` starts at field 3.
        fields.get(19).map(|s| (*s).to_string())
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn start_time(pid: u32) -> Option<String> {
        let out = Command::new("ps")
            .args(["-o", "lstart=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (out.status.success() && !text.is_empty()).then_some(text)
    }

    /// The nearest ancestor of this process whose name starts with `name`.
    pub(super) fn ancestor(name: &str) -> Option<u32> {
        let (mut pid, _) = info(std::process::id())?;
        for _ in 0..16 {
            if pid <= 1 {
                return None;
            }
            let (parent, comm) = info(pid)?;
            if comm.starts_with(name) {
                return Some(pid);
            }
            pid = parent;
        }
        None
    }

    /// A process's parent and its name, from `ps`.
    fn info(pid: u32) -> Option<(u32, String)> {
        let out = Command::new("ps")
            .args(["-o", "ppid=,comm=", "-p", &pid.to_string()])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut fields = text.split_whitespace();
        let ppid = fields.next()?.parse().ok()?;
        let comm = fields.collect::<Vec<_>>().join(" ");
        let base = comm.rsplit('/').next().unwrap_or(&comm).to_string();
        Some((ppid, base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_cover_files_directories_and_globs() {
        assert!(covers("src/a.rs", "src/a.rs"));
        assert!(!covers("src/a.rs", "src/a.rsx"));
        assert!(covers("src/", "src/deep/b.rs"));
        assert!(covers("src/", "src"));
        assert!(!covers("src/", "srcs/b.rs"));
        assert!(covers("src/*.rs", "src/a.rs"));
        assert!(!covers("src/*.rs", "src/deep/a.rs"));
        assert!(covers("src/**/*.rs", "src/deep/a.rs"));
        assert!(covers("src/**/*.rs", "src/a.rs"));
        assert!(covers("docs/**", "docs/a/b.md"));
    }

    #[test]
    fn overlap_is_symmetric_and_errs_toward_yes() {
        for (a, b, want) in [
            ("src/a.rs", "src/a.rs", true),
            ("src/", "src/a.rs", true),
            ("src/a.rs", "docs/", false),
            ("src/*.rs", "src/a.rs", true),
            ("src/*.rs", "docs/a.rs", false),
            ("src/", "src/**/*.rs", true),
            ("src/**", "src/cmd/*.rs", true),
            ("docs/*.md", "src/*.rs", false),
            ("src/cmd/", "src/", true),
        ] {
            assert_eq!(overlaps(a, b), want, "{a} vs {b}");
            assert_eq!(overlaps(b, a), want, "{b} vs {a}");
        }
    }

    fn holder(id: &str) -> Holder {
        Holder {
            session: id.into(),
            harness: Some("claude".into()),
            pid: None,
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn worktrees_of_one_repository_share_claims() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir_all(main.join("src")).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("src/a.rs"), "a").unwrap();
        git(&main, &["add", "."]);
        git(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ],
        );
        git(&main, &["worktree", "add", "-q", "-b", "other", "../wt"]);
        let wt = tmp.path().join("wt");
        let state = tmp.path().join("state");
        let a = Store::open(&state, &main.join("src")).unwrap();
        let b = Store::open(&state, &wt).unwrap();
        assert_eq!(a.dir, b.dir);
        assert_eq!(branch(&b.root).as_deref(), Some("other"));

        let now = 1_000_000;
        let pattern = a.pattern(&main.join("src"), "a.rs").unwrap();
        assert_eq!(pattern, "src/a.rs");
        assert_eq!(a.pattern(&main, "src").unwrap(), "src/");
        assert!(a.pattern(&main, ".").is_err());
        assert!(a.pattern(&main, "../elsewhere").is_err());
        let claimed = a
            .claim(&holder("s1"), &[pattern], now + 60, Some("fixing"), now)
            .unwrap();
        assert_eq!(claimed[0].branch.as_deref(), Some("main"));

        // Another session in the other worktree sees it, and is refused an
        // overlapping claim.
        let held = b.live(now);
        assert_eq!(held.len(), 1);
        assert_eq!(held[0].session, "s1");
        let Err(Refused::Conflicts(conflicts)) =
            b.claim(&holder("s2"), &["src/".into()], now + 60, None, now)
        else {
            panic!("an overlapping claim should be refused");
        };
        assert_eq!(conflicts[0].held.pattern, "src/a.rs");
        // The same session renews its claim.
        a.claim(&holder("s1"), &["src/a.rs".into()], now + 120, None, now)
            .unwrap();
        assert_eq!(a.live(now).len(), 1);

        // It expires, and pruning removes it.
        assert!(a.live(now + 200).is_empty());
        assert_eq!(a.prune(now + 200)[0].1, Standing::Expired);
        assert!(a.all(now).is_empty());

        // A claim lapses with its worktree.
        b.claim(&holder("s2"), &["docs/".into()], now + 60, None, now)
            .unwrap();
        git(&main, &["worktree", "remove", "--force", "../wt"]);
        assert_eq!(a.all(now)[0].1, Standing::WorktreeGone);

        assert!(a.release("s9", &[], now).is_empty());
        assert_eq!(a.release("s2", &[], now).len(), 1);
    }

    #[test]
    fn a_claim_lapses_when_its_agent_exits() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let start = agent::start_time(pid);
        assert!(start.is_some());
        assert!(agent::running(pid, start.as_deref()));
        assert!(!agent::running(pid, Some("not-its-start")));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!agent::running(pid, start.as_deref()));
    }
}
