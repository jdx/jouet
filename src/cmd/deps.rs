//! `jouet deps info PKG...` and `jouet deps audit`: packages checked
//! against their registries and OSV.

use super::Output;
use crate::deps::osv::{self, Advisory, Query};
use crate::deps::registry::{self, Facts};
use crate::deps::{Http, ecosystem, registry_name, same_name};
use crate::docs::{self, Lang, Locked};
use crate::{DepsArgs, DepsAuditArgs, DepsCmd, DepsInfoArgs};
use jouet_output::exit::Exit;
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;

pub(crate) fn run(args: &DepsArgs, out: &Output) -> Exit {
    match &args.cmd {
        DepsCmd::Info(args) => info(args, out),
        DepsCmd::Audit(args) => audit(args, out),
    }
}

// --- info --------------------------------------------------------------------

/// A package to look up, and where its version came from.
struct Target {
    lang: Lang,
    name: String,
    version: Option<String>,
    /// `given`, or the lockfile that locks it.
    from: From,
    notes: Vec<String>,
}

enum From {
    Given,
    Lockfile(String),
    Latest,
}

#[derive(Serialize)]
struct InfoReport {
    packages: Vec<Package>,
}

#[derive(Serialize)]
struct Package {
    #[serde(skip)]
    lang: Lang,
    ecosystem: &'static str,
    name: String,
    /// The registry's spelling, when it is not the name asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    registry_name: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    not_found: bool,
    /// Close names the registry's search found, for one that does not
    /// exist.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suggestions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest: Option<String>,
    /// The version checked: the one given, else the locked one, else the
    /// latest.
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    /// The lockfile `version` came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    lockfile: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    version_not_found: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    yanked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    yanked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deprecated: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    advisories: Vec<Advisory>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
}

impl Package {
    fn exit(&self) -> Exit {
        if self.not_found || self.version_not_found {
            Exit::NotFound
        } else if !self.advisories.is_empty() || self.yanked || self.deprecated.is_some() {
            Exit::Failed
        } else {
            Exit::Ok
        }
    }
}

fn info(args: &DepsInfoArgs, out: &Output) -> Exit {
    let cwd = std::env::current_dir().unwrap_or_default();
    let locked = docs::lockfiles(&cwd);
    let mut targets = Vec::new();
    for spec in &args.packages {
        match target(&cwd, &locked, spec) {
            Ok(t) => targets.push(t),
            Err((exit, message, hint)) => {
                return out.error_with_hint(exit, &message, hint.as_deref());
            }
        }
    }
    let http = Http::new(args.refresh);
    let mut packages = Vec::new();
    let mut queries = Vec::new();
    for t in targets {
        let mut facts = match registry::lookup(&http, t.lang, &t.name, t.version.as_deref()) {
            Ok(facts) => facts,
            Err(e) => return out.error(Exit::Remote, &e),
        };
        // The latest version is checked like any other: it may be yanked.
        if t.version.is_none()
            && let Some(latest) = facts.as_ref().and_then(|f| f.latest.clone())
        {
            facts = match registry::lookup(&http, t.lang, &t.name, Some(&latest)) {
                Ok(facts) => facts,
                Err(e) => return out.error(Exit::Remote, &e),
            };
        }
        let package = describe(&http, t, facts);
        if let Some(version) = package
            .version
            .as_ref()
            .filter(|_| package.exit() != Exit::NotFound)
        {
            queries.push((
                packages.len(),
                Query {
                    lang: package.lang,
                    name: package
                        .registry_name
                        .clone()
                        .unwrap_or_else(|| package.name.clone()),
                    version: version.clone(),
                },
            ));
        }
        packages.push(package);
    }
    let asked: Vec<Query> = queries.iter().map(|(_, q)| q.clone()).collect();
    let found = match osv::advisories(&http, &asked) {
        Ok(found) => found,
        Err(e) => return out.error(Exit::Remote, &e),
    };
    for ((i, _), advisories) in queries.iter().zip(found) {
        packages[*i].advisories = advisories;
    }
    let exit = packages
        .iter()
        .map(Package::exit)
        .max_by_key(|e| match e {
            Exit::NotFound => 2,
            Exit::Failed => 1,
            _ => 0,
        })
        .unwrap_or(Exit::Ok);
    let report = InfoReport { packages };
    out.result(&report, || render_info(&report));
    exit
}

/// `cargo:serde@1.0`, `npm:@scope/x`, or a bare name the project places.
/// Why a target was refused: the exit, the message and a hint.
type Refusal = (Exit, String, Option<String>);

fn target(cwd: &Path, locked: &[Locked], spec: &str) -> Result<Target, Refusal> {
    let (lang, rest) = match spec.split_once(':') {
        Some((prefix, rest)) => match Lang::parse(prefix) {
            Some(lang) => (Some(lang), rest),
            None => {
                return Err((
                    Exit::Usage,
                    format!("`{prefix}` is not an ecosystem: cargo, npm, pypi or go"),
                    None,
                ));
            }
        },
        None => (None, spec),
    };
    let (name, version) = docs::split_spec(rest);
    if name.is_empty() {
        return Err((Exit::Usage, format!("`{spec}` names no package"), None));
    }
    let pinned = |lang: Lang| -> Vec<(&Locked, &docs::Pin)> {
        locked
            .iter()
            .filter(|l| l.lang == lang)
            .flat_map(|l| l.pins.iter().map(move |p| (l, p)))
            .filter(|(_, p)| same_name(lang, &p.name, name))
            .collect()
    };
    let lang = match lang {
        Some(lang) => lang,
        None => {
            let in_lockfiles: Vec<Lang> = Lang::ALL
                .into_iter()
                .filter(|&l| !pinned(l).is_empty())
                .collect();
            match in_lockfiles.as_slice() {
                [one] => *one,
                [] => match manifests(cwd).as_slice() {
                    [one] => *one,
                    [] => {
                        return Err((
                            Exit::Usage,
                            format!(
                                "no lockfile or manifest here says which ecosystem `{name}` is in"
                            ),
                            Some(format!(
                                "name it: cargo:{name}, npm:{name}, pypi:{name} or go:{name}"
                            )),
                        ));
                    }
                    several => return Err(ambiguous(name, several, "this project has manifests")),
                },
                several => return Err(ambiguous(name, several, "lockfiles here lock it")),
            }
        }
    };
    let name = name.to_string();
    let mut pins = pinned(lang);
    // A name locked from anywhere but the public registry is not sent, not
    // even to look up the public package of the same name.
    if let Some(&(lockfile, _)) = pins.iter().find(|(_, p)| !p.public) {
        let shown = lockfile.path.strip_prefix(cwd).unwrap_or(&lockfile.path);
        return Err((
            Exit::Precondition,
            format!(
                "{} locks {name} from a private registry, a path, a workspace or git, so jouet deps does not send its name",
                shown.display()
            ),
            Some("check it where it is published".into()),
        ));
    }
    if let Some(v) = version {
        return Ok(Target {
            lang,
            name,
            version: Some(v.to_string()),
            from: From::Given,
            notes: Vec::new(),
        });
    }
    let Some(&(lockfile, _)) = pins.first() else {
        return Ok(Target {
            lang,
            name,
            version: None,
            from: From::Latest,
            notes: Vec::new(),
        });
    };
    pins.sort_by(|a, b| crate::deps::compare(&a.1.version, &b.1.version));
    let mut notes = Vec::new();
    let (_, newest) = pins[pins.len() - 1];
    if pins.len() > 1 {
        let others: Vec<&str> = pins[..pins.len() - 1]
            .iter()
            .map(|(_, p)| p.version.as_str())
            .collect();
        notes.push(format!(
            "the lockfile also locks {}; NAME@VERSION checks one of them",
            others.join(", ")
        ));
    }
    let shown = lockfile.path.strip_prefix(cwd).unwrap_or(&lockfile.path);
    Ok(Target {
        lang,
        name: newest.name.clone(),
        version: Some(newest.version.clone()),
        from: From::Lockfile(shown.display().to_string()),
        notes,
    })
}

fn ambiguous(name: &str, langs: &[Lang], why: &str) -> Refusal {
    let named: Vec<String> = langs
        .iter()
        .map(|l| format!("{}:{name}", ecosystem(*l)))
        .collect();
    (
        Exit::Usage,
        format!("`{name}` could be in {} ecosystems: {why}", langs.len()),
        Some(format!("name one: {}", named.join(" or "))),
    )
}

/// The ecosystems whose manifests are here or above, up to the work
/// tree's root.
fn manifests(cwd: &Path) -> Vec<Lang> {
    let files: [(Lang, &[&str]); 4] = [
        (Lang::Rust, &["Cargo.toml", "Cargo.lock"]),
        (Lang::Ts, &["package.json"]),
        (
            Lang::Python,
            &[
                "pyproject.toml",
                "requirements.txt",
                "setup.py",
                "Pipfile",
                "uv.lock",
                "poetry.lock",
            ],
        ),
        (Lang::Go, &["go.mod"]),
    ];
    let mut out = Vec::new();
    for dir in cwd.ancestors() {
        for (lang, names) in files {
            if !out.contains(&lang) && names.iter().any(|n| dir.join(n).is_file()) {
                out.push(lang);
            }
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    out
}

/// The report for one package, from what its registry said.
fn describe(http: &Http, t: Target, facts: Option<Facts>) -> Package {
    let mut package = Package {
        lang: t.lang,
        ecosystem: ecosystem(t.lang),
        name: t.name.clone(),
        registry_name: None,
        not_found: false,
        suggestions: Vec::new(),
        latest: None,
        version: t.version.clone(),
        lockfile: None,
        version_not_found: false,
        yanked: false,
        yanked_reason: None,
        deprecated: None,
        license: None,
        repository: None,
        advisories: Vec::new(),
        notes: t.notes,
    };
    let Some(facts) = facts else {
        package.not_found = true;
        package.version = None;
        package.suggestions = registry::suggest(http, t.lang, &t.name);
        return package;
    };
    if facts.name != t.name {
        package.registry_name = Some(facts.name.clone());
    }
    package.latest = facts.latest.clone();
    package.license = facts.license.clone();
    package.repository = facts.repository.clone();
    package.deprecated = facts.deprecated.clone();
    match (&t.from, facts.release) {
        (_, None) => package.version = facts.latest,
        (from, Some(release)) => {
            package.version = Some(release.version.clone());
            if let From::Lockfile(path) = from {
                package.lockfile = Some(path.clone());
            }
            package.version_not_found = !release.exists;
            package.yanked = release.yanked.is_some();
            package.yanked_reason = release.yanked.filter(|r| !r.is_empty());
            if release.deprecated.is_some() {
                package.deprecated = release.deprecated;
            }
            if release.license.is_some() {
                package.license = release.license;
            }
        }
    }
    package
}

fn render_info(report: &InfoReport) -> String {
    let mut out = String::new();
    for (i, p) in report.packages.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let lang = p.lang;
        let registry = registry_name(lang);
        if p.not_found {
            let _ = writeln!(out, "{} {}: not on {registry}", p.ecosystem, p.name);
            if !p.suggestions.is_empty() {
                let _ = writeln!(out, "  did you mean: {}?", p.suggestions.join(", "));
            }
            continue;
        }
        let name = p.registry_name.as_deref().unwrap_or(&p.name);
        let version = p.version.as_deref().unwrap_or("(no version)");
        if p.version_not_found {
            let _ = writeln!(
                out,
                "{} {name} {version}: {registry} has no such version; the latest is {}",
                p.ecosystem,
                p.latest.as_deref().unwrap_or("unknown")
            );
            continue;
        }
        let mut head = format!("{} {name} {version}", p.ecosystem);
        let mut said = Vec::new();
        if let Some(lockfile) = &p.lockfile {
            said.push(format!("locked in {lockfile}"));
        }
        match p.latest.as_deref() {
            Some(latest) if Some(latest) == p.version.as_deref() => said.push("latest".into()),
            Some(latest) => said.push(format!("latest {latest}")),
            None => {}
        }
        if p.registry_name.is_some() {
            said.push(format!("{registry} spells it {name}, not {}", p.name));
        }
        if !said.is_empty() {
            let _ = write!(head, " ({})", said.join("; "));
        }
        let _ = writeln!(out, "{head}");
        if p.yanked {
            let reason = p
                .yanked_reason
                .as_deref()
                .map_or_else(String::new, |r| format!(": {r}"));
            let verb = if lang == Lang::Go {
                "retracted"
            } else {
                "yanked"
            };
            let _ = writeln!(out, "  {verb}{reason}");
        }
        if let Some(why) = &p.deprecated {
            let why = if why.is_empty() { "deprecated" } else { why };
            let _ = writeln!(out, "  deprecated: {why}");
        }
        let mut about = Vec::new();
        if let Some(license) = &p.license {
            about.push(format!("licence {license}"));
        }
        if let Some(repo) = &p.repository {
            about.push(format!("repository {repo}"));
        }
        if !about.is_empty() {
            let _ = writeln!(out, "  {}", about.join("; "));
        }
        if p.advisories.is_empty() {
            let _ = writeln!(out, "  no known advisories affect {version} (OSV)");
        }
        for a in &p.advisories {
            let _ = writeln!(out, "  {}", advisory_line(a));
        }
        for note in &p.notes {
            let _ = writeln!(out, "  note: {note}");
        }
    }
    out
}

/// `RUSTSEC-2020-0071 (medium): Potential segfault; fixed in 0.2.23; also
/// CVE-2020-26235`
fn advisory_line(a: &Advisory) -> String {
    let mut line = a.id.clone();
    if let Some(kind) = a.severity.as_deref().or(a.informational.as_deref()) {
        let _ = write!(line, " ({kind})");
    }
    if let Some(summary) = &a.summary {
        let _ = write!(line, ": {summary}");
    }
    match &a.fixed {
        Some(fixed) => {
            let _ = write!(line, "; fixed in {fixed}");
        }
        None => line.push_str("; no fixed version"),
    }
    if !a.aliases.is_empty() {
        let _ = write!(line, "; also {}", a.aliases.join(", "));
    }
    line
}

// --- audit -------------------------------------------------------------------

#[derive(Serialize)]
struct AuditReport {
    lockfiles: Vec<LockfileSummary>,
    /// Package versions sent to OSV.
    checked: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    affected: Vec<Affected>,
}

#[derive(Serialize)]
struct LockfileSummary {
    ecosystem: &'static str,
    path: String,
    packages: usize,
    /// Locked from a private registry, a path or git, so not sent.
    #[serde(skip_serializing_if = "is_zero")]
    private: usize,
}

#[derive(Serialize)]
struct Affected {
    ecosystem: &'static str,
    name: String,
    version: String,
    lockfile: String,
    advisories: Vec<Advisory>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn audit(args: &DepsAuditArgs, out: &Output) -> Exit {
    let only = match args.ecosystem.as_deref().map(|e| (e, Lang::parse(e))) {
        None => None,
        Some((_, Some(lang))) => Some(lang),
        Some((text, None)) => {
            return out.error(
                Exit::Usage,
                &format!("--ecosystem takes cargo, npm, pypi or go, not `{text}`"),
            );
        }
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let locked: Vec<Locked> = docs::lockfiles(&cwd)
        .into_iter()
        .filter(|l| only.is_none_or(|o| o == l.lang))
        .collect();
    if locked.is_empty() {
        let what = match only {
            None => "Cargo.lock, npm lockfile, uv.lock, poetry.lock or go.mod".to_string(),
            Some(lang) => format!("{} lockfile", ecosystem(lang)),
        };
        return out.error_with_hint(
            Exit::NotFound,
            &format!("no {what} from here to the repository's root"),
            Some("run it in the project, or check one package with `jouet deps info ECOSYSTEM:NAME@VERSION`"),
        );
    }
    let shown = |path: &Path| {
        path.strip_prefix(&cwd)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut asked: Vec<(usize, Query)> = Vec::new();
    let mut lockfiles = Vec::new();
    for (i, l) in locked.iter().enumerate() {
        let public: Vec<&docs::Pin> = l.pins.iter().filter(|p| p.public).collect();
        lockfiles.push(LockfileSummary {
            ecosystem: ecosystem(l.lang),
            path: shown(&l.path),
            packages: l.pins.len(),
            private: l.pins.len() - public.len(),
        });
        asked.extend(public.into_iter().map(|p| {
            (
                i,
                Query {
                    lang: l.lang,
                    name: p.name.clone(),
                    version: p.version.clone(),
                },
            )
        }));
    }
    let queries: Vec<Query> = asked.iter().map(|(_, q)| q.clone()).collect();
    let found = match osv::advisories(&Http::new(args.refresh), &queries) {
        Ok(found) => found,
        Err(e) => return out.error(Exit::Remote, &e),
    };
    let affected: Vec<Affected> = asked
        .iter()
        .zip(found)
        .filter(|(_, advisories)| !advisories.is_empty())
        .map(|((i, q), advisories)| Affected {
            ecosystem: ecosystem(q.lang),
            name: q.name.clone(),
            version: q.version.clone(),
            lockfile: shown(&locked[*i].path),
            advisories,
        })
        .collect();
    let exit = if affected.is_empty() {
        Exit::Ok
    } else {
        Exit::Failed
    };
    let report = AuditReport {
        lockfiles,
        checked: queries.len(),
        affected,
    };
    out.result(&report, || render_audit(&report));
    exit
}

fn render_audit(report: &AuditReport) -> String {
    let mut out = String::new();
    let sources: Vec<String> = report
        .lockfiles
        .iter()
        .map(|l| {
            let private = if l.private > 0 {
                format!(" ({} private, not sent)", l.private)
            } else {
                String::new()
            };
            format!("{} {}{private}", l.path, l.packages)
        })
        .collect();
    let packages = if report.checked == 1 {
        "package"
    } else {
        "packages"
    };
    if report.affected.is_empty() {
        let _ = writeln!(
            out,
            "no known advisories in {} locked {packages} (OSV; {})",
            report.checked,
            sources.join(", ")
        );
        return out;
    }
    let _ = writeln!(
        out,
        "{} of {} locked {packages} have advisories (OSV; {})",
        report.affected.len(),
        report.checked,
        sources.join(", ")
    );
    for a in &report.affected {
        let _ = writeln!(
            out,
            "{} {} {} ({})",
            a.ecosystem, a.name, a.version, a.lockfile
        );
        for advisory in &a.advisories {
            let _ = writeln!(out, "  {}", advisory_line(advisory));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisories_read_as_one_line() {
        let a = Advisory {
            id: "RUSTSEC-2020-0071".into(),
            aliases: vec!["CVE-2020-26235".into()],
            summary: Some("Potential segfault in the time crate".into()),
            severity: Some("medium".into()),
            informational: None,
            fixed: Some("0.2.23".into()),
            url: String::new(),
        };
        assert_eq!(
            advisory_line(&a),
            "RUSTSEC-2020-0071 (medium): Potential segfault in the time crate; fixed in 0.2.23; also CVE-2020-26235"
        );
    }

    #[test]
    fn targets_name_their_ecosystem_or_take_the_projects() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.lock"),
            "[[package]]\nname = \"time\"\nversion = \"0.2.7\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
        )
        .unwrap();
        let locked = docs::lockfiles(dir.path());
        let t = target(dir.path(), &locked, "time").ok().unwrap();
        assert_eq!((t.lang, t.version.as_deref()), (Lang::Rust, Some("0.2.7")));
        assert!(matches!(t.from, From::Lockfile(ref p) if p == "Cargo.lock"));
        let t = target(dir.path(), &locked, "npm:@types/node@20.1.0")
            .ok()
            .unwrap();
        assert_eq!(
            (t.lang, t.name.as_str(), t.version.as_deref()),
            (Lang::Ts, "@types/node", Some("20.1.0"))
        );
        // Cargo.lock is the only manifest-like file, so a new name is a crate.
        let t = target(dir.path(), &locked, "serde").ok().unwrap();
        assert!(matches!(t.from, From::Latest));
        assert_eq!(t.lang, Lang::Rust);
        assert!(target(dir.path(), &locked, "maven:x").is_err());
    }
}
