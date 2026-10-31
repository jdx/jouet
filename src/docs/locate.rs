//! Which version of a package the project locked, and where its sources
//! are installed at that version. Nothing here downloads or runs a package
//! manager: a package that is locked but not installed is a [`Missing`]
//! with the command that installs it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A package ecosystem, by the language its API is read in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Lang {
    Rust,
    /// npm packages: their `.d.ts` files, else their JavaScript.
    Ts,
    Python,
    Go,
}

impl Lang {
    pub(crate) const ALL: [Lang; 4] = [Lang::Rust, Lang::Ts, Lang::Python, Lang::Go];

    pub(crate) fn parse(text: &str) -> Option<Lang> {
        Some(match text.to_ascii_lowercase().as_str() {
            "rust" | "rs" | "cargo" | "crates" | "crates.io" => Lang::Rust,
            "ts" | "typescript" | "js" | "javascript" | "node" | "npm" => Lang::Ts,
            "python" | "py" | "pypi" | "pip" => Lang::Python,
            "go" | "golang" => Lang::Go,
            _ => return None,
        })
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Ts => "ts",
            Lang::Python => "python",
            Lang::Go => "go",
        }
    }
}

/// A package found at its locked version, installed.
#[derive(Debug, Serialize)]
pub(crate) struct Package {
    pub(crate) lang: Lang,
    /// As its ecosystem names it: a crate, an npm package, a Python
    /// distribution, a Go package path.
    pub(crate) name: String,
    pub(crate) version: String,
    /// The lockfile (or metadata) the version came from.
    pub(crate) lockfile: PathBuf,
    /// The installed package.
    pub(crate) root: PathBuf,
    /// Where its API is read: `root`, or its `@types` package, or a Go
    /// package within its module.
    pub(crate) api_root: PathBuf,
    /// Python: the import packages the distribution installs.
    #[serde(skip)]
    pub(crate) modules: Vec<String>,
    /// Anything the reader should know: another locked version, types read
    /// from `@types`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<String>,
}

/// Why a package could not be read.
#[derive(Debug)]
pub(crate) enum Missing {
    /// No lockfile or installed metadata from here up names it, or not at
    /// the version asked for.
    NotFound {
        message: String,
        hint: Option<String>,
    },
    /// Locked, but not installed at that version, or not something jouet
    /// reads.
    Precondition {
        message: String,
        hint: Option<String>,
    },
}

type Probe = Option<Result<Package, Missing>>;

/// Finds `name` (optionally `name@version`) in the lockfiles of `cwd` and
/// the directories above it, up to the work tree's root.
pub(crate) fn locate(cwd: &Path, spec: &str, only: Option<Lang>) -> Result<Package, Missing> {
    let (name, version) = split_spec(spec);
    for dir in cwd.ancestors() {
        for lang in Lang::ALL {
            if only.is_some_and(|l| l != lang) {
                continue;
            }
            let probe = match lang {
                Lang::Rust => rust(dir, name, version),
                Lang::Ts => npm(dir, name, version),
                Lang::Python => python(dir, name, version),
                Lang::Go => go(dir, name, version),
            };
            if let Some(result) = probe {
                return result;
            }
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    let what = only.map_or(
        "Cargo.lock, an npm lockfile, uv.lock, poetry.lock, a virtualenv or go.mod",
        |l| match l {
            Lang::Rust => "Cargo.lock",
            Lang::Ts => "an npm lockfile or node_modules",
            Lang::Python => "uv.lock, poetry.lock or a virtualenv",
            Lang::Go => "go.mod",
        },
    );
    Err(Missing::NotFound {
        message: format!("no {what} from here to the repository's root names `{name}`"),
        hint: Some("run it in the project that depends on the package".into()),
    })
}

/// `name@version`; a leading `@` is an npm scope, not a version.
pub(crate) fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec[1.min(spec.len())..].rfind('@') {
        Some(at) => {
            let at = at + 1.min(spec.len());
            (&spec[..at], Some(&spec[at + 1..]).filter(|v| !v.is_empty()))
        }
        None => (spec, None),
    }
}

fn not_installed(message: String, hint: String) -> Missing {
    Missing::Precondition {
        message,
        hint: Some(hint),
    }
}

/// Versions compared by their numeric parts, for choosing the newest.
fn version_key(version: &str) -> Vec<u64> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse().ok())
        .collect()
}

/// Picks the version asked for among those locked, or the newest; notes the
/// others.
fn choose(
    lockfile: &Path,
    name: &str,
    mut versions: Vec<String>,
    wanted: Option<&str>,
    preferred: Option<&str>,
    notes: &mut Vec<String>,
) -> Result<String, Missing> {
    versions.sort_by_key(|v| version_key(v));
    versions.dedup();
    let lock = lockfile
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    if let Some(wanted) = wanted {
        return versions
            .iter()
            .find(|v| *v == wanted)
            .cloned()
            .ok_or_else(|| Missing::NotFound {
                message: format!("{lock} locks {name} {}, not {wanted}", versions.join(", ")),
                hint: Some(format!(
                    "jouet docs {name}@{}",
                    versions.last().cloned().unwrap_or_default()
                )),
            });
    }
    let chosen = preferred
        .filter(|p| versions.iter().any(|v| v == p))
        .map(str::to_string)
        .or_else(|| versions.last().cloned())
        .unwrap_or_default();
    if versions.len() > 1 {
        notes.push(format!(
            "{lock} locks {} versions of {name} ({}); this is {chosen}, and {name}@VERSION picks another",
            versions.len(),
            versions.join(", ")
        ));
    }
    Ok(chosen)
}

/// A package name compared across spellings: `serde-core`, `serde_core`,
/// `Typing.Extensions`.
pub(crate) fn name_key(name: &str) -> String {
    name.to_ascii_lowercase().replace(['-', '.'], "_")
}

/// The other packages `from`'s lockfile locks, by [`name_key`]: (name,
/// version). For Rust, the version `from` itself depends on wins over the
/// newest. Where a re-export of another package leads.
pub(crate) fn neighbours(from: &Package) -> HashMap<String, (String, String)> {
    let mut out: HashMap<String, (String, String)> = HashMap::new();
    let Ok(text) = std::fs::read_to_string(&from.lockfile) else {
        return out;
    };
    match from.lang {
        Lang::Rust => {
            let Ok(lock) = toml::from_str::<CargoLock>(&text) else {
                return out;
            };
            let mut sorted: Vec<&CargoPackage> = lock.package.iter().collect();
            sorted.sort_by_key(|p| version_key(&p.version));
            for p in sorted {
                out.insert(name_key(&p.name), (p.name.clone(), p.version.clone()));
            }
            if let Some(own) = lock
                .package
                .iter()
                .find(|p| p.name == from.name && p.version == from.version)
            {
                for dep in &own.dependencies {
                    let mut words = dep.split(' ');
                    if let (Some(name), Some(version)) = (words.next(), words.next()) {
                        out.insert(name_key(name), (name.to_string(), version.to_string()));
                    }
                }
            }
        }
        Lang::Python => {
            if let Ok(lock) = toml::from_str::<PyLock>(&text) {
                for p in lock.package {
                    if let Some(version) = p.version {
                        out.insert(name_key(&p.name), (p.name, version));
                    }
                }
            }
        }
        Lang::Ts | Lang::Go => {}
    }
    out
}

/// A package version a lockfile locks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pin {
    pub(crate) name: String,
    pub(crate) version: String,
    /// From the ecosystem's public registry (crates.io, npm, PyPI, the Go
    /// module proxy), not a private registry, a path or a git repository.
    pub(crate) public: bool,
}

/// A lockfile and what it locks.
#[derive(Debug)]
pub(crate) struct Locked {
    pub(crate) lang: Lang,
    pub(crate) path: PathBuf,
    /// Sorted by name, then version.
    pub(crate) pins: Vec<Pin>,
}

/// The nearest lockfile of each ecosystem, from `cwd` up to the work
/// tree's root: `Cargo.lock`, an npm-family lockfile, `uv.lock` or
/// `poetry.lock`, and `go.mod`.
pub(crate) fn lockfiles(cwd: &Path) -> Vec<Locked> {
    lockfiles_with(cwd, &go_env("GOPRIVATE").unwrap_or_default())
}

/// [`lockfiles`], with GOPRIVATE's patterns given.
fn lockfiles_with(cwd: &Path, goprivate: &str) -> Vec<Locked> {
    let mut out: Vec<Locked> = Vec::new();
    for dir in cwd.ancestors() {
        for lang in Lang::ALL {
            if !out.iter().any(|l| l.lang == lang)
                && let Some(locked) = locked_in(dir, lang, goprivate)
            {
                out.push(locked);
            }
        }
        if dir.join(".git").exists() {
            break;
        }
    }
    out
}

fn locked_in(dir: &Path, lang: Lang, goprivate: &str) -> Option<Locked> {
    let (path, mut pins) = match lang {
        Lang::Rust => {
            let path = dir.join("Cargo.lock");
            let lock: CargoLock = toml::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            let pins = lock
                .package
                .into_iter()
                // Workspace members have no source.
                .filter_map(|p| {
                    let source = p.source?;
                    Some(Pin {
                        public: source
                            .starts_with("registry+https://github.com/rust-lang/crates.io-index")
                            || source.starts_with("sparse+https://index.crates.io/"),
                        name: p.name,
                        version: p.version,
                    })
                })
                .collect();
            (path, pins)
        }
        Lang::Ts => {
            let (file, _) = NPM_LOCKS
                .iter()
                .find(|(file, _)| dir.join(file).is_file())?;
            let path = dir.join(file);
            let pins = npm_packages(&path);
            (path, pins)
        }
        Lang::Python => {
            let path = ["uv.lock", "poetry.lock"]
                .iter()
                .map(|f| dir.join(f))
                .find(|p| p.is_file())?;
            let lock: PyLock = toml::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            let pins = lock
                .package
                .into_iter()
                .filter_map(|p| {
                    // uv: `{ registry = URL }`, `{ editable = "." }`, `{ git
                    // = URL }`; poetry names a source only when it is not
                    // PyPI.
                    let public = p.source.as_ref().is_none_or(|s| {
                        s.get("registry")
                            .and_then(toml::Value::as_str)
                            .is_some_and(|url| url.starts_with("https://pypi.org/"))
                    });
                    Some(Pin {
                        name: p.name,
                        version: p.version?,
                        public,
                    })
                })
                .collect();
            (path, pins)
        }
        Lang::Go => {
            let path = dir.join("go.mod");
            let (requires, replaces, _) = go_mod(&std::fs::read_to_string(&path).ok()?);
            let pins = requires
                .into_iter()
                .map(|(module, version)| Pin {
                    public: !replaces.iter().any(|(from, _)| *from == module)
                        && !go_private(goprivate, &module),
                    name: module,
                    version,
                })
                .collect();
            (path, pins)
        }
    };
    pins.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    pins.dedup_by(|a, b| a.name == b.name && a.version == b.version);
    Some(Locked { lang, path, pins })
}

// --- Rust -----------------------------------------------------------------

#[derive(Deserialize)]
struct CargoLock {
    #[serde(default)]
    package: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    source: Option<String>,
    #[serde(default)]
    dependencies: Vec<String>,
}

fn cargo_home() -> Option<PathBuf> {
    jouet_output::paths::env_dir("CARGO_HOME")
        .or_else(|| std::env::home_dir().map(|h| h.join(".cargo")))
}

fn rust(dir: &Path, name: &str, wanted: Option<&str>) -> Probe {
    let lockfile = dir.join("Cargo.lock");
    let text = std::fs::read_to_string(&lockfile).ok()?;
    let lock: CargoLock = toml::from_str(&text).ok()?;
    let norm = |s: &str| s.replace('-', "_");
    let found: Vec<&CargoPackage> = lock
        .package
        .iter()
        .filter(|p| norm(&p.name) == norm(name))
        .collect();
    let first = found.first()?;
    let crate_name = first.name.clone();
    // The version this workspace's own packages depend on, when they name one.
    let direct = lock
        .package
        .iter()
        .filter(|p| p.source.is_none())
        .flat_map(|p| &p.dependencies)
        .find_map(|d| {
            let mut words = d.split(' ');
            (words.next() == Some(crate_name.as_str())).then(|| words.next().map(str::to_string))?
        });
    let mut notes = Vec::new();
    let version = match choose(
        &lockfile,
        &crate_name,
        found.iter().map(|p| p.version.clone()).collect(),
        wanted,
        direct.as_deref(),
        &mut notes,
    ) {
        Ok(v) => v,
        Err(e) => return Some(Err(e)),
    };
    let package = found.iter().find(|p| p.version == version)?;
    let Some(source) = &package.source else {
        return Some(Err(Missing::Precondition {
            message: format!("{crate_name} is a package of this workspace, not a dependency"),
            hint: Some("read it with `jouet read --map` or `jouet graph`".into()),
        }));
    };
    let home = cargo_home();
    let dirname = format!("{crate_name}-{version}");
    let mut candidates: Vec<PathBuf> = Vec::new();
    for vendor in ["vendor", "third_party/vendor"] {
        candidates.push(dir.join(vendor).join(&dirname));
        candidates.push(dir.join(vendor).join(&crate_name));
    }
    if source.starts_with("registry+") || source.starts_with("sparse+") {
        if let Some(home) = &home
            && let Ok(indexes) = std::fs::read_dir(home.join("registry").join("src"))
        {
            for index in indexes.flatten() {
                candidates.push(index.path().join(&dirname));
            }
        }
    } else if let Some(rev) = source
        .strip_prefix("git+")
        .and_then(|s| s.rsplit_once('#'))
        .map(|(_, r)| r)
        && let Some(home) = &home
        && let Ok(checkouts) = std::fs::read_dir(home.join("git").join("checkouts"))
    {
        let short: String = rev.chars().take(7).collect();
        for checkout in checkouts.flatten() {
            let at = checkout.path().join(&short);
            candidates.extend(cargo_packages(&at, 3));
        }
    }
    let root = candidates
        .into_iter()
        .find(|c| manifest_names(&c.join("Cargo.toml"), &crate_name, &version));
    let Some(root) = root else {
        let hint = if source.starts_with("git+") {
            "`cargo fetch` checks it out (or any cargo build)"
        } else {
            "`cargo fetch` downloads it (or any cargo build)"
        };
        return Some(Err(not_installed(
            format!(
                "Cargo.lock locks {crate_name} {version}, but its source is not in {}",
                home.map_or_else(
                    || "the cargo home".to_string(),
                    |h| h.join("registry/src").display().to_string()
                )
            ),
            format!("{hint} in {}", dir.display()),
        )));
    };
    Some(Ok(Package {
        lang: Lang::Rust,
        name: crate_name,
        version,
        lockfile,
        api_root: root.clone(),
        root,
        modules: Vec::new(),
        notes,
    }))
}

/// Directories under `dir`, to `depth`, holding a `Cargo.toml`.
fn cargo_packages(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if dir.join("Cargo.toml").is_file() {
        out.push(dir.to_path_buf());
    }
    if depth > 0
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && !entry.file_name().to_string_lossy().starts_with('.')
            {
                out.extend(cargo_packages(&entry.path(), depth - 1));
            }
        }
    }
    out
}

/// Whether the manifest at `path` is package `name` at `version`.
fn manifest_names(path: &Path, name: &str, version: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    let package = value.get("package");
    let field = |key: &str| {
        package
            .and_then(|p| p.get(key))
            .and_then(toml::Value::as_str)
    };
    field("name") == Some(name) && field("version").is_none_or(|v| v == version)
}

// --- npm ------------------------------------------------------------------

const NPM_LOCKS: [(&str, &str); 4] = [
    ("package-lock.json", "npm ci"),
    ("pnpm-lock.yaml", "pnpm install"),
    ("yarn.lock", "yarn install"),
    ("bun.lock", "bun install"),
];

fn npm(dir: &Path, name: &str, wanted: Option<&str>) -> Probe {
    let lock = NPM_LOCKS.iter().find(|(file, _)| dir.join(file).is_file());
    let installed_at = dir.join("node_modules").join(name);
    let manifest = installed_at.join("package.json");
    let installed = std::fs::read_to_string(&manifest)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get("version")?.as_str().map(str::to_string));
    let (lockfile, install) = match lock {
        Some((file, install)) => (dir.join(file), *install),
        None if installed.is_some() => (manifest.clone(), "npm install"),
        None => return None,
    };
    let locked = if lock.is_some() {
        npm_locked(&lockfile, name)
    } else {
        Vec::new()
    };
    if locked.is_empty() && installed.is_none() {
        return None;
    }
    let lock_name = lockfile
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    let Some(version) = installed else {
        return Some(Err(not_installed(
            format!(
                "{lock_name} locks {name} {}, but it is not in node_modules",
                locked.join(", ")
            ),
            format!("`{install}` in {} installs it", dir.display()),
        )));
    };
    if !locked.is_empty() && !locked.contains(&version) {
        return Some(Err(not_installed(
            format!(
                "node_modules has {name} {version}, but {lock_name} locks {}",
                locked.join(", ")
            ),
            format!(
                "`{install}` in {} installs the locked version",
                dir.display()
            ),
        )));
    }
    if let Some(wanted) = wanted
        && wanted != version
    {
        return Some(Err(Missing::NotFound {
            message: format!("node_modules has {name} {version}, not {wanted}"),
            hint: Some(format!("jouet docs {name}")),
        }));
    }
    let root = installed_at.canonicalize().unwrap_or(installed_at);
    let mut notes = Vec::new();
    let mut api_root = root.clone();
    if !super::js::has_types(&root) {
        let mangled = name
            .strip_prefix('@')
            .map_or_else(|| name.to_string(), |n| n.replace('/', "__"));
        let types = dir.join("node_modules").join("@types").join(&mangled);
        if types.is_dir() {
            let types_version = std::fs::read_to_string(types.join("package.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("version")?.as_str().map(str::to_string))
                .unwrap_or_default();
            notes.push(format!(
                "{name} ships no types; these are from @types/{mangled} {types_version}"
            ));
            api_root = types.canonicalize().unwrap_or(types);
        }
    }
    Some(Ok(Package {
        lang: Lang::Ts,
        name: name.to_string(),
        version,
        lockfile,
        root,
        api_root,
        modules: Vec::new(),
        notes,
    }))
}

/// The versions of `name` an npm-family lockfile locks.
fn npm_locked(lockfile: &Path, name: &str) -> Vec<String> {
    let mut out: Vec<String> = npm_packages(lockfile)
        .into_iter()
        .filter(|p| p.name == name)
        .map(|p| p.version)
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every (name, version) an npm-family lockfile locks from the registry:
/// not workspace packages, links or tarballs.
fn npm_packages(lockfile: &Path) -> Vec<Pin> {
    let Ok(text) = std::fs::read_to_string(lockfile) else {
        return Vec::new();
    };
    let file = lockfile.file_name().and_then(|f| f.to_str()).unwrap_or("");
    let registry = |v: &str| v.starts_with(|c: char| c.is_ascii_digit());
    // A tarball URL elsewhere is another registry's package.
    let public = |resolved: Option<&str>| {
        resolved.is_none_or(|url| {
            [
                "https://registry.npmjs.org/",
                "https://registry.yarnpkg.com/",
            ]
            .iter()
            .any(|r| url.starts_with(r))
        })
    };
    let mut out: Vec<Pin> = match file {
        "package-lock.json" => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                return Vec::new();
            };
            let field = |v: &serde_json::Value, key: &str| -> Option<String> {
                v.get(key)?.as_str().map(str::to_string)
            };
            let mut found: Vec<Pin> = value
                .get("packages")
                .and_then(|p| p.as_object())
                .into_iter()
                .flatten()
                .filter(|(_, v)| v.get("link").is_none())
                .filter_map(|(key, v)| {
                    let (_, at) = key.rsplit_once("node_modules/")?;
                    Some(Pin {
                        // An alias (`"x": "npm:y@1"`) names the real package.
                        name: field(v, "name").unwrap_or_else(|| at.to_string()),
                        version: field(v, "version")?,
                        public: public(field(v, "resolved").as_deref()),
                    })
                })
                .collect();
            // Lockfile version 1: nested `dependencies`.
            let mut stack: Vec<&serde_json::Value> =
                value.get("dependencies").into_iter().collect();
            while let Some(deps) = stack.pop() {
                for (name, dep) in deps.as_object().into_iter().flatten() {
                    if let Some(version) = field(dep, "version") {
                        found.push(Pin {
                            name: name.clone(),
                            version,
                            public: public(field(dep, "resolved").as_deref()),
                        });
                    }
                    stack.extend(dep.get("dependencies"));
                }
            }
            found
        }
        "pnpm-lock.yaml" => text
            .lines()
            .filter(|l| l.starts_with("  ") && !l.starts_with("   ") && l.trim_end().ends_with(':'))
            .filter_map(|l| {
                let key = l.trim().trim_end_matches(':').trim_matches(['\'', '"']);
                let key = key.strip_prefix('/').unwrap_or(key);
                let key = key.split('(').next().unwrap_or(key);
                // v6 and later: `name@1.2.3(peer@x)`; v5: `/name/1.2.3`.
                let (name, version) = key
                    .rfind('@')
                    .filter(|&at| at > 0)
                    .map(|at| (&key[..at], &key[at + 1..]))
                    .filter(|(_, v)| registry(v))
                    .or_else(|| key.rsplit_once('/'))?;
                Some(Pin {
                    name: name.to_string(),
                    version: version.to_string(),
                    public: true,
                })
            })
            .collect(),
        "yarn.lock" => {
            let mut found: Vec<Pin> = Vec::new();
            // The block's names, and where its pins start in `found`.
            let mut names: Vec<String> = Vec::new();
            let mut start = 0;
            for line in text.lines() {
                if !line.starts_with(' ') && !line.is_empty() && !line.starts_with('#') {
                    names = line
                        .trim_end_matches(':')
                        .split(',')
                        .filter_map(|spec| {
                            let spec = spec.trim().trim_matches('"');
                            let at = spec.get(1..)?.find('@')? + 1;
                            let range = &spec[at + 1..];
                            let local = ["workspace:", "link:", "portal:", "file:"];
                            (!local.iter().any(|l| range.starts_with(l)))
                                .then(|| spec[..at].to_string())
                        })
                        .collect();
                    names.dedup();
                    start = found.len();
                } else if let Some(v) = line.trim().strip_prefix("version") {
                    let v = v.trim_start_matches(':').trim().trim_matches('"');
                    for name in names.drain(..) {
                        found.push(Pin {
                            name,
                            version: v.to_string(),
                            public: true,
                        });
                    }
                } else if let Some(url) = line.trim().strip_prefix("resolved ") {
                    let url = url.trim().trim_matches('"');
                    for pin in &mut found[start..] {
                        pin.public = public(Some(url));
                    }
                }
            }
            found
        }
        "bun.lock" => regex::Regex::new(r#""[^"]+": \["((?:@[^"@/]+/)?[^"@]+)@([^"]+)""#)
            .map(|re| {
                re.captures_iter(&text)
                    .map(|c| Pin {
                        name: c[1].to_string(),
                        version: c[2].to_string(),
                        public: true,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    out.retain(|p| !p.name.is_empty() && registry(&p.version));
    out.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    out.dedup_by(|a, b| a.name == b.name && a.version == b.version);
    out
}

// --- Python ---------------------------------------------------------------

#[derive(Deserialize)]
struct PyLock {
    #[serde(default)]
    package: Vec<PyPackage>,
}

#[derive(Deserialize)]
struct PyPackage {
    name: String,
    version: Option<String>,
    source: Option<toml::Value>,
}

/// PEP 503: lowercase, runs of `-`, `_` and `.` as one `-`.
fn pep503(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// The site-packages directories of the project's virtualenv:
/// `$VIRTUAL_ENV`, `.venv` or `venv`.
fn site_packages(dir: &Path) -> Vec<PathBuf> {
    let mut venvs: Vec<PathBuf> = Vec::new();
    if let Some(active) = jouet_output::paths::env_dir("VIRTUAL_ENV") {
        venvs.push(active);
    }
    venvs.push(dir.join(".venv"));
    venvs.push(dir.join("venv"));
    let mut out = Vec::new();
    for venv in venvs {
        let windows = venv.join("Lib").join("site-packages");
        if windows.is_dir() {
            out.push(windows);
        }
        if let Ok(entries) = std::fs::read_dir(venv.join("lib")) {
            for entry in entries.flatten() {
                let site = entry.path().join("site-packages");
                if entry.file_name().to_string_lossy().starts_with("python") && site.is_dir() {
                    out.push(site);
                }
            }
        }
    }
    out
}

/// An installed distribution: its `.dist-info` directory, name and version.
struct Dist {
    info: PathBuf,
    name: String,
    version: String,
}

/// The installed distribution named `name`, or installing the import
/// package `name`.
fn installed_dist(site: &Path, name: &str) -> Option<Dist> {
    let wanted = pep503(name);
    let mut by_module = None;
    for entry in std::fs::read_dir(site).ok()?.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let Some(stem) = file.strip_suffix(".dist-info") else {
            continue;
        };
        let Some((dist, version)) = stem.rsplit_once('-') else {
            continue;
        };
        // The directory's name is normalised; METADATA has the real one.
        let real_name = std::fs::read_to_string(entry.path().join("METADATA"))
            .ok()
            .and_then(|m| {
                m.lines()
                    .find_map(|l| l.strip_prefix("Name:").map(|n| n.trim().to_string()))
            })
            .unwrap_or_else(|| dist.to_string());
        let found = Dist {
            info: entry.path(),
            name: real_name,
            version: version.to_string(),
        };
        if pep503(dist) == wanted {
            return Some(found);
        }
        if by_module.is_none() && dist_modules(site, &found.info).iter().any(|m| *m == name) {
            by_module = Some(found);
        }
    }
    by_module
}

/// The import packages and modules a distribution installs.
fn dist_modules(site: &Path, info: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_to_string(info.join("top_level.txt"))
        .map(|t| {
            t.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if out.is_empty()
        && let Ok(record) = std::fs::read_to_string(info.join("RECORD"))
    {
        for line in record.lines() {
            let path = line.split(',').next().unwrap_or("");
            let first = path.split('/').next().unwrap_or("");
            let module = if path.contains('/') {
                first.to_string()
            } else if let Some(stem) = first.strip_suffix(".py") {
                stem.to_string()
            } else {
                continue;
            };
            if module.ends_with(".dist-info")
                || module.starts_with("..")
                || module == "__pycache__"
                || module.contains('.')
            {
                continue;
            }
            if !out.contains(&module) {
                out.push(module);
            }
        }
    }
    out.retain(|m| {
        site.join(m).is_dir()
            || site.join(format!("{m}.py")).is_file()
            || site.join(format!("{m}.pyi")).is_file()
    });
    out
}

fn python(dir: &Path, name: &str, wanted: Option<&str>) -> Probe {
    let lock = [("uv.lock", "uv sync"), ("poetry.lock", "poetry install")]
        .into_iter()
        .find(|(file, _)| dir.join(file).is_file());
    let sites = site_packages(dir);
    let installed = sites
        .iter()
        .find_map(|site| installed_dist(site, name).map(|d| (site.clone(), d)));
    let locked: Vec<(String, String)> = lock
        .and_then(|(file, _)| std::fs::read_to_string(dir.join(file)).ok())
        .and_then(|t| toml::from_str::<PyLock>(&t).ok())
        .map(|l| {
            l.package
                .into_iter()
                .filter_map(|p| Some((p.name, p.version?)))
                .collect()
        })
        .unwrap_or_default();
    let dist_name = installed.as_ref().map_or(name, |(_, d)| d.name.as_str());
    let locked_versions: Vec<String> = locked
        .iter()
        .filter(|(n, _)| pep503(n) == pep503(name) || pep503(n) == pep503(dist_name))
        .map(|(_, v)| v.clone())
        .collect();
    if locked_versions.is_empty() && installed.is_none() {
        return None;
    }
    let install = lock.map_or("pip install", |(_, install)| install);
    let lockfile = match (lock, &installed) {
        (Some((file, _)), _) => dir.join(file),
        (None, Some((_, d))) => d.info.join("METADATA"),
        (None, None) => return None,
    };
    let lock_name = lockfile
        .file_name()
        .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
    let Some((site, dist)) = installed else {
        let venv = if sites.is_empty() {
            "there is no virtualenv (.venv, venv or $VIRTUAL_ENV)".to_string()
        } else {
            format!("it is not installed in {}", sites[0].display())
        };
        return Some(Err(not_installed(
            format!(
                "{lock_name} locks {name} {}, but {venv}",
                locked_versions.join(", ")
            ),
            format!("`{install}` in {} installs it", dir.display()),
        )));
    };
    if !locked_versions.is_empty() && !locked_versions.iter().any(|v| v == &dist.version) {
        return Some(Err(not_installed(
            format!(
                "the virtualenv has {} {}, but {lock_name} locks {}",
                dist.name,
                dist.version,
                locked_versions.join(", ")
            ),
            format!(
                "`{install}` in {} installs the locked version",
                dir.display()
            ),
        )));
    }
    if let Some(wanted) = wanted
        && wanted != dist.version
    {
        return Some(Err(Missing::NotFound {
            message: format!(
                "the virtualenv has {} {}, not {wanted}",
                dist.name, dist.version
            ),
            hint: Some(format!("jouet docs {name}")),
        }));
    }
    let modules = dist_modules(&site, &dist.info);
    if modules.is_empty() {
        return Some(Err(Missing::Precondition {
            message: format!(
                "{} {} installs no Python package jouet can read",
                dist.name, dist.version
            ),
            hint: None,
        }));
    }
    Some(Ok(Package {
        lang: Lang::Python,
        name: dist.name,
        version: dist.version,
        lockfile,
        root: site.clone(),
        api_root: site,
        modules,
        notes: Vec::new(),
    }))
}

// --- Go -------------------------------------------------------------------

/// (module, version) or (module, replacement) pairs.
type Pairs = Vec<(String, String)>;

/// `go.mod`'s requirements, its replacements and its own module path.
fn go_mod(text: &str) -> (Pairs, Pairs, Option<String>) {
    let mut requires = Vec::new();
    let mut replaces = Vec::new();
    let mut module = None;
    let mut block: Option<&str> = None;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == ")" {
            block = None;
            continue;
        }
        let (verb, rest) = match block {
            Some(verb) => (verb, line),
            None => {
                let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
                let rest = rest.trim();
                if rest == "(" {
                    block = Some(match verb {
                        "require" => "require",
                        "replace" => "replace",
                        _ => "other",
                    });
                    continue;
                }
                (verb, rest)
            }
        };
        match verb {
            "module" => module = Some(rest.trim_matches('"').to_string()),
            "require" => {
                let mut words = rest.split_whitespace();
                if let (Some(m), Some(v)) = (words.next(), words.next()) {
                    requires.push((m.to_string(), v.to_string()));
                }
            }
            "replace" => {
                if let Some((from, to)) = rest.split_once("=>") {
                    let from = from.split_whitespace().next().unwrap_or("").to_string();
                    replaces.push((from, to.trim().to_string()));
                }
            }
            _ => {}
        }
    }
    (requires, replaces, module)
}

/// The module cache: `$GOMODCACHE`, else `$GOPATH/pkg/mod`, reading `go env
/// -w`'s file for either, else `~/go/pkg/mod`.
fn gomodcache() -> Option<PathBuf> {
    if let Some(dir) = go_env("GOMODCACHE") {
        return Some(PathBuf::from(dir));
    }
    let gopath = go_env("GOPATH").and_then(|p| std::env::split_paths(&p).next());
    gopath
        .or_else(|| std::env::home_dir().map(|h| h.join("go")))
        .map(|p| p.join("pkg").join("mod"))
}

/// A Go setting: the environment variable, else `go env -w`'s file.
fn go_env(key: &str) -> Option<String> {
    if let Some(value) = std::env::var(key).ok().filter(|v| !v.is_empty()) {
        return Some(value);
    }
    let home = std::env::home_dir()?;
    [
        home.join(".config/go/env"),
        home.join("Library/Application Support/go/env"),
    ]
    .iter()
    .filter_map(|f| std::fs::read_to_string(f).ok())
    .find_map(|t| {
        t.lines().find_map(|l| {
            l.strip_prefix(key)?
                .strip_prefix('=')
                .map(|v| v.trim().to_string())
        })
    })
}

/// Whether `module` matches GOPRIVATE's comma-separated path prefixes, each
/// a glob per path element (`*.corp.example.com,github.com/me/*`).
fn go_private(patterns: &str, module: &str) -> bool {
    patterns
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .any(|pattern| {
            let want: Vec<&str> = pattern.split('/').collect();
            let have: Vec<&str> = module.split('/').collect();
            have.len() >= want.len()
                && want
                    .iter()
                    .zip(&have)
                    .all(|(w, h)| glob(w.as_bytes(), h.as_bytes()))
        })
}

/// `*` and `?` within one path element.
fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        Some((&c, rest)) => text
            .split_first()
            .is_some_and(|(&t, tail)| (c == b'?' || c == t) && glob(rest, tail)),
    }
}

/// The module cache's case encoding: `Azure` is `!azure`.
pub(crate) fn go_escape(path: &str) -> String {
    let mut out = String::new();
    for c in path.chars() {
        if c.is_ascii_uppercase() {
            out.push('!');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn go(dir: &Path, name: &str, wanted: Option<&str>) -> Probe {
    let lockfile = dir.join("go.mod");
    let text = std::fs::read_to_string(&lockfile).ok()?;
    let (requires, replaces, own) = go_mod(&text);
    if let Some(own) = &own
        && (name == own || name.starts_with(&format!("{own}/")))
    {
        return Some(Err(Missing::Precondition {
            message: format!("{name} is in this module ({own}), not a dependency"),
            hint: Some("read it with `jouet read --map` or `jouet graph`".into()),
        }));
    }
    let (module, version) = requires
        .iter()
        .filter(|(m, _)| name == m || name.starts_with(&format!("{m}/")))
        .max_by_key(|(m, _)| m.len())?;
    let sub = name[module.len()..].trim_start_matches('/');
    if let Some(wanted) = wanted
        && wanted != version
    {
        return Some(Err(Missing::NotFound {
            message: format!("go.mod requires {module} {version}, not {wanted}"),
            hint: Some(format!("jouet docs {name}")),
        }));
    }
    let mut at_module: Option<PathBuf> = None;
    let mut cache_module = (module.clone(), version.clone());
    if let Some((_, to)) = replaces.iter().find(|(from, _)| from == module) {
        let mut words = to.split_whitespace();
        let target = words.next().unwrap_or("");
        if target.starts_with('.') || target.starts_with('/') {
            at_module = Some(dir.join(target));
        } else if let Some(v) = words.next() {
            cache_module = (target.to_string(), v.to_string());
        }
    }
    if at_module.is_none() && dir.join("vendor").join("modules.txt").is_file() {
        at_module = Some(dir.join("vendor").join(module));
    }
    let cache = gomodcache();
    let module_dir = at_module.or_else(|| {
        cache
            .as_ref()
            .map(|c| c.join(format!("{}@{}", go_escape(&cache_module.0), cache_module.1)))
    });
    let Some(module_dir) = module_dir.filter(|d| d.is_dir()) else {
        return Some(Err(not_installed(
            format!(
                "go.mod requires {module} {version}, but it is not in the module cache{}",
                cache.map_or_else(String::new, |c| format!(" ({})", c.display()))
            ),
            format!(
                "`go mod download {module}` in {} downloads it",
                dir.display()
            ),
        )));
    };
    let package_dir = if sub.is_empty() {
        module_dir.clone()
    } else {
        module_dir.join(sub)
    };
    if !package_dir.is_dir() {
        return Some(Err(Missing::NotFound {
            message: format!("{module} {version} has no package {name}"),
            hint: None,
        }));
    }
    Some(Ok(Package {
        lang: Lang::Go,
        name: name.to_string(),
        version: version.clone(),
        lockfile,
        root: module_dir,
        api_root: package_dir,
        modules: Vec::new(),
        notes: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_split_at_the_version() {
        assert_eq!(split_spec("ureq"), ("ureq", None));
        assert_eq!(split_spec("ureq@3.5.0"), ("ureq", Some("3.5.0")));
        assert_eq!(split_spec("@tanstack/query"), ("@tanstack/query", None));
        assert_eq!(
            split_spec("@tanstack/query@5.1.0"),
            ("@tanstack/query", Some("5.1.0"))
        );
    }

    #[test]
    fn go_mod_requires_and_replaces_are_read() {
        let (requires, replaces, module) = go_mod(
            "module example.com/me\n\nrequire (\n\tgithub.com/spf13/cobra v1.8.0\n\tgolang.org/x/sys v0.1.0 // indirect\n)\nrequire github.com/a/b v1.0.0\nreplace github.com/a/b => ../b\n",
        );
        assert_eq!(module.as_deref(), Some("example.com/me"));
        assert_eq!(requires.len(), 3);
        assert_eq!(requires[1], ("golang.org/x/sys".into(), "v0.1.0".into()));
        assert_eq!(replaces, vec![("github.com/a/b".into(), "../b".into())]);
        assert_eq!(go_escape("github.com/Azure/go"), "github.com/!azure/go");
    }

    #[test]
    fn npm_lockfiles_name_their_versions() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let path = dir.path().join(name);
            std::fs::write(&path, text).unwrap();
            path
        };
        let pnpm = write(
            "pnpm-lock.yaml",
            "lockfileVersion: '9.0'\n\npackages:\n\n  zod@4.1.0:\n    resolution: {}\n\n  '@scope/x@1.0.0(react@18.0.0)':\n    resolution: {}\n",
        );
        assert_eq!(npm_locked(&pnpm, "zod"), vec!["4.1.0"]);
        assert_eq!(npm_locked(&pnpm, "@scope/x"), vec!["1.0.0"]);
        let yarn = write(
            "yarn.lock",
            "\"zod@^4.0.0\", zod@^4.1:\n  version \"4.1.0\"\n  resolved \"x\"\n\nzodiac@1:\n  version \"1.0.0\"\n",
        );
        assert_eq!(npm_locked(&yarn, "zod"), vec!["4.1.0"]);
        let bun = write(
            "bun.lock",
            "{\n  \"packages\": {\n    \"zod\": [\"zod@4.1.0\", \"\", {}, \"sha\"],\n  },\n}\n",
        );
        assert_eq!(npm_locked(&bun, "zod"), vec!["4.1.0"]);
        let npm = write(
            "package-lock.json",
            r#"{"packages": {"": {}, "node_modules/zod": {"version": "4.1.0"}, "node_modules/a/node_modules/zod": {"version": "3.0.0"}}}"#,
        );
        assert_eq!(npm_locked(&npm, "zod"), vec!["3.0.0", "4.1.0"]);
    }

    #[test]
    fn lockfiles_list_their_pins_and_which_are_public() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join(".git")).unwrap();
        let write = |name: &str, text: &str| std::fs::write(root.join(name), text).unwrap();
        write(
            "uv.lock",
            "[[package]]\nname = \"app\"\nversion = \"0.1.0\"\nsource = { editable = \".\" }\n\n[[package]]\nname = \"requests\"\nversion = \"2.19.0\"\nsource = { registry = \"https://pypi.org/simple\" }\n\n[[package]]\nname = \"internal\"\nversion = \"1.0.0\"\nsource = { registry = \"https://pypi.corp.example.com/simple\" }\n",
        );
        write(
            "yarn.lock",
            "\"lodash@^4.17.0\":\n  version \"4.17.15\"\n  resolved \"https://registry.yarnpkg.com/lodash/-/lodash-4.17.15.tgz\"\n\n\"@corp/x@^1\":\n  version \"1.0.0\"\n  resolved \"https://npm.corp.example.com/@corp/x/-/x-1.0.0.tgz\"\n\n\"app@workspace:.\":\n  version \"0.0.0-use.local\"\n",
        );
        write(
            "go.mod",
            "module example.com/app\n\nrequire (\n\tgithub.com/spf13/cobra v1.8.0\n\tgit.corp.example.com/team/lib v0.3.0\n\tgithub.com/a/b v1.0.0\n)\nreplace github.com/a/b => ../b\n",
        );
        let pins = |lang: Lang| -> Vec<(String, bool)> {
            lockfiles_with(root, "*.corp.example.com")
                .into_iter()
                .find(|l| l.lang == lang)
                .unwrap()
                .pins
                .into_iter()
                .map(|p| (format!("{}@{}", p.name, p.version), p.public))
                .collect()
        };
        let owned = |list: &[(&str, bool)]| -> Vec<(String, bool)> {
            list.iter().map(|(n, p)| ((*n).to_string(), *p)).collect()
        };
        assert_eq!(
            pins(Lang::Python),
            owned(&[
                ("app@0.1.0", false),
                ("internal@1.0.0", false),
                ("requests@2.19.0", true)
            ])
        );
        assert_eq!(
            pins(Lang::Ts),
            owned(&[("@corp/x@1.0.0", false), ("lodash@4.17.15", true)])
        );
        assert_eq!(
            pins(Lang::Go),
            owned(&[
                ("git.corp.example.com/team/lib@v0.3.0", false),
                ("github.com/a/b@v1.0.0", false),
                ("github.com/spf13/cobra@v1.8.0", true)
            ])
        );
        assert!(go_private("github.com/me/*", "github.com/me/x/v2"));
        assert!(!go_private("github.com/me/*", "github.com/you/x"));
    }

    #[test]
    fn python_names_are_normalised() {
        assert_eq!(pep503("Jinja2"), "jinja2");
        assert_eq!(pep503("typing_extensions"), "typing-extensions");
        assert_eq!(pep503("zope.interface"), "zope-interface");
    }
}
