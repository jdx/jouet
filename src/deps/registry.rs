//! What a package's registry says about it: crates.io's API, the npm
//! registry, PyPI's JSON API and the Go module proxy. Each is asked about
//! one package by name, and about one version of it when one is given.

use super::cache;
use super::http::{Http, Service};
use super::version;
use crate::docs::{Lang, go_escape};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A package as its registry describes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Facts {
    /// As the registry spells it: crates.io answers `serde-json` for
    /// `serde_json`.
    pub(crate) name: String,
    pub(crate) latest: Option<String>,
    /// Of the latest version.
    pub(crate) license: Option<String>,
    pub(crate) repository: Option<String>,
    /// Why the package is deprecated, when it is: npm's message on its
    /// latest version, a Go module's `// Deprecated:` comment, PyPI's
    /// "Inactive" classifier.
    pub(crate) deprecated: Option<String>,
    /// The version asked about, when one was.
    pub(crate) release: Option<Release>,
}

/// One version of a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Release {
    pub(crate) version: String,
    pub(crate) exists: bool,
    /// Yanked (crates.io, PyPI) or retracted (Go): the reason, or "" when
    /// none was given.
    pub(crate) yanked: Option<String>,
    /// npm deprecates versions one by one.
    pub(crate) deprecated: Option<String>,
    pub(crate) license: Option<String>,
}

/// The package `name` (at `version`), or None when the registry has no
/// such package. An error is the registry's: unreachable, or an answer
/// jouet could not read.
pub(crate) fn lookup(
    http: &Http,
    lang: Lang,
    name: &str,
    version: Option<&str>,
) -> Result<Option<Facts>, String> {
    let key = format!("{name}@{}", version.unwrap_or(""));
    let parts = ["registry", super::ecosystem(lang), key.as_str()];
    if !http.refresh
        && let Some(cached) = cache::read::<Option<Facts>>(&parts, cache::REGISTRY_TTL)
    {
        return Ok(cached);
    }
    let found = match lang {
        Lang::Rust => crates(http, name, version),
        Lang::Ts => npm(http, name, version),
        Lang::Python => pypi(http, name, version),
        Lang::Go => go(http, name, version),
    }?;
    cache::write(&parts, &found);
    Ok(found)
}

/// Downloads a suggested name needs: crates.io's in the last 90 days, npm's
/// in the last week.
const MIN_DOWNLOADS: u64 = 10_000;

/// Names the registry's search finds that are a typo away from `name` and
/// well used, the closest first. Only crates.io and npm have a search; PyPI's was turned
/// off, and the Go proxy has none.
pub(crate) fn suggest(http: &Http, lang: Lang, name: &str) -> Vec<String> {
    let search = |query: &str| -> Vec<(String, u64)> {
        let (path, list, field, downloads) = match lang {
            Lang::Rust => (
                format!("/api/v1/crates?per_page=20&q={}", encode(query)),
                "/crates",
                "/name",
                "/recent_downloads",
            ),
            Lang::Ts => (
                format!("/-/v1/search?size=20&text={}", encode(query)),
                "/objects",
                "/package/name",
                "/downloads/weekly",
            ),
            Lang::Python | Lang::Go => return Vec::new(),
        };
        let Ok(resp) = http.get(Service::Registry(lang), &path) else {
            return Vec::new();
        };
        let Ok(doc) = serde_json::from_str::<Value>(&resp.body) else {
            return Vec::new();
        };
        doc.pointer(list)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|hit| {
                let name = hit.pointer(field)?.as_str()?.to_string();
                Some((name, hit.pointer(downloads)?.as_u64()?))
            })
            .collect()
    };
    let norm = |s: &str| s.to_ascii_lowercase().replace('_', "-");
    let wanted = norm(name);
    let near = (wanted.chars().count() / 5).clamp(1, 2);
    // A name a typo away that few download is as likely a typosquat as
    // the package meant: only well-used ones are offered.
    let close = |found: Vec<(String, u64)>| -> Vec<(usize, u64, String)> {
        let mut out: Vec<(usize, u64, String)> = found
            .into_iter()
            .filter(|(_, downloads)| *downloads >= MIN_DOWNLOADS)
            .map(|(n, downloads)| (distance(&norm(&n), &wanted), downloads, n))
            .filter(|(d, _, _)| (1..=near).contains(d))
            .collect();
        out.sort_by_key(|(d, downloads, _)| (*d, std::cmp::Reverse(*downloads)));
        out
    };
    let mut found = close(search(name));
    // Search matches words, so a typo in the last letters finds nothing;
    // the name without them often does.
    if found.is_empty() && name.chars().count() > 4 {
        let shorter: String = name.chars().take(name.chars().count() - 1).collect();
        found = close(search(&shorter));
    }
    let mut names: Vec<String> = Vec::new();
    for (_, _, n) in found {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    names.truncate(3);
    names
}

/// Percent-encodes everything but unreserved characters.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// The edit distance between two names.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(prev + usize::from(ca != *cb));
            prev = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// A 200's JSON, None for a 404 (or the Go proxy's 410), and an error for
/// anything else.
fn fetch(http: &Http, lang: Lang, path: &str) -> Result<Option<Value>, String> {
    let service = Service::Registry(lang);
    let resp = http.get(service, path)?;
    match resp.status {
        200 => serde_json::from_str(&resp.body).map(Some).map_err(|e| {
            format!(
                "{} sent an answer jouet could not read: {e}",
                service.name()
            )
        }),
        404 | 410 => Ok(None),
        status => Err(format!("{} returned {status}", service.name())),
    }
}

fn text(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

// --- crates.io ---------------------------------------------------------------

fn crates(http: &Http, name: &str, version: Option<&str>) -> Result<Option<Facts>, String> {
    let path = format!("/api/v1/crates/{}?include=default_version", encode(name));
    let Some(doc) = fetch(http, Lang::Rust, &path)? else {
        return Ok(None);
    };
    let name = text(&doc, "/crate/name").unwrap_or_else(|| name.to_string());
    let latest = text(&doc, "/crate/max_stable_version")
        .or_else(|| text(&doc, "/crate/default_version"))
        .or_else(|| text(&doc, "/crate/newest_version"));
    let release = match version {
        None => None,
        Some(v) => {
            let path = format!("/api/v1/crates/{}/{}", encode(&name), encode(v));
            Some(match fetch(http, Lang::Rust, &path)? {
                None => missing_release(v),
                Some(doc) => Release {
                    version: v.to_string(),
                    exists: true,
                    yanked: (doc.pointer("/version/yanked") == Some(&Value::Bool(true)))
                        .then(|| text(&doc, "/version/yank_message").unwrap_or_default()),
                    deprecated: None,
                    license: text(&doc, "/version/license"),
                },
            })
        }
    };
    Ok(Some(Facts {
        name,
        latest,
        license: text(&doc, "/versions/0/license"),
        repository: text(&doc, "/crate/repository"),
        deprecated: None,
        release,
    }))
}

fn missing_release(version: &str) -> Release {
    Release {
        version: version.to_string(),
        exists: false,
        yanked: None,
        deprecated: None,
        license: None,
    }
}

// --- npm ---------------------------------------------------------------------

/// `@scope/name` is one path segment, `@scope%2Fname`.
fn npm_path(name: &str) -> String {
    match name.strip_prefix('@').and_then(|n| n.split_once('/')) {
        Some((scope, rest)) => format!("/@{}%2F{}", encode(scope), encode(rest)),
        None => format!("/{}", encode(name)),
    }
}

/// npm's `license` is a string or, in old packages, `{ "type": ... }`.
fn npm_license(doc: &Value) -> Option<String> {
    text(doc, "/license").or_else(|| text(doc, "/license/type"))
}

fn npm_deprecated(doc: &Value) -> Option<String> {
    match doc.get("deprecated")? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Bool(true) => Some(String::new()),
        _ => None,
    }
}

fn npm(http: &Http, name: &str, version: Option<&str>) -> Result<Option<Facts>, String> {
    let base = npm_path(name);
    let Some(latest) = fetch(http, Lang::Ts, &format!("{base}/latest"))? else {
        return Ok(None);
    };
    let repository = latest
        .get("repository")
        .and_then(|r| r.as_str().map(str::to_string).or_else(|| text(r, "/url")))
        .and_then(|r| repository_url(&r));
    let release = match version {
        None => None,
        Some(v) => Some(
            match fetch(http, Lang::Ts, &format!("{base}/{}", encode(v)))? {
                None => missing_release(v),
                Some(doc) => Release {
                    version: v.to_string(),
                    exists: true,
                    yanked: None,
                    deprecated: npm_deprecated(&doc),
                    license: npm_license(&doc),
                },
            },
        ),
    };
    Ok(Some(Facts {
        name: text(&latest, "/name").unwrap_or_else(|| name.to_string()),
        latest: text(&latest, "/version"),
        license: npm_license(&latest),
        repository,
        deprecated: npm_deprecated(&latest),
        release,
    }))
}

/// A browsable repository URL from npm's forms: `git+https://h/a/b.git`,
/// `git://h/a/b`, `git@github.com:a/b.git`, `github:a/b`, `a/b`.
fn repository_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let mut url = raw
        .trim_start_matches("git+")
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string();
    if let Some(rest) = url.strip_prefix("git://") {
        url = format!("https://{rest}");
    } else if let Some(rest) = url.strip_prefix("ssh://git@") {
        url = format!("https://{rest}");
    } else if let Some(rest) = url.strip_prefix("git@") {
        url = format!("https://{}", rest.replacen(':', "/", 1));
    } else if let Some(rest) = url.strip_prefix("github:") {
        url = format!("https://github.com/{rest}");
    } else if !url.contains(':') && url.matches('/').count() == 1 {
        url = format!("https://github.com/{url}");
    }
    (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
}

// --- PyPI --------------------------------------------------------------------

fn pypi(http: &Http, name: &str, version: Option<&str>) -> Result<Option<Facts>, String> {
    let path = format!("/pypi/{}/json", encode(name));
    let Some(doc) = fetch(http, Lang::Python, &path)? else {
        return Ok(None);
    };
    let info = doc.get("info").cloned().unwrap_or(Value::Null);
    let classifiers: Vec<&str> = info
        .get("classifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let license = text(&info, "/license_expression")
        .or_else(|| {
            // Some packages paste the whole licence text here.
            text(&info, "/license").filter(|l| l.len() <= 80 && !l.contains('\n'))
        })
        .or_else(|| {
            classifiers
                .iter()
                .filter_map(|c| c.strip_prefix("License :: "))
                .filter_map(|c| c.rsplit(" :: ").next())
                .next()
                .map(str::to_string)
        });
    let deprecated = classifiers
        .contains(&"Development Status :: 7 - Inactive")
        .then(|| "marked inactive (Development Status :: 7 - Inactive)".to_string());
    let release = version.map(|v| {
        let releases = doc.get("releases").and_then(Value::as_object);
        let files = releases.and_then(|r| {
            r.get(v).or_else(|| {
                r.iter()
                    .find(|(k, _)| version::compare(k, v).is_eq())
                    .map(|(_, f)| f)
            })
        });
        match files {
            None => missing_release(v),
            Some(files) => {
                let files: Vec<&Value> = files.as_array().into_iter().flatten().collect();
                let yanked = !files.is_empty()
                    && files
                        .iter()
                        .all(|f| f.get("yanked") == Some(&Value::Bool(true)));
                Release {
                    version: v.to_string(),
                    exists: true,
                    yanked: yanked.then(|| {
                        files
                            .iter()
                            .find_map(|f| text(f, "/yanked_reason"))
                            .unwrap_or_default()
                    }),
                    deprecated: None,
                    license: None,
                }
            }
        }
    });
    Ok(Some(Facts {
        name: text(&info, "/name").unwrap_or_else(|| name.to_string()),
        latest: text(&info, "/version"),
        license,
        repository: pypi_repository(&info),
        deprecated,
        release,
    }))
}

/// The source repository among a project's URLs: the one labelled as
/// source, else any on a code host.
fn pypi_repository(info: &Value) -> Option<String> {
    let urls: Vec<(String, String)> = info
        .get("project_urls")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((k.to_ascii_lowercase(), v.as_str()?.to_string())))
        .chain(text(info, "/home_page").map(|h| ("homepage".to_string(), h)))
        .collect();
    let labelled = ["source", "source code", "repository", "code", "github"];
    let hosts = [
        "https://github.com/",
        "https://gitlab.com/",
        "https://codeberg.org/",
        "https://bitbucket.org/",
    ];
    labelled
        .iter()
        .find_map(|label| urls.iter().find(|(k, _)| k == label))
        .or_else(|| {
            urls.iter()
                .find(|(_, url)| hosts.iter().any(|h| url.starts_with(h)))
        })
        .map(|(_, url)| url.trim_end_matches('/').to_string())
}

// --- Go ----------------------------------------------------------------------

fn go(http: &Http, module: &str, version: Option<&str>) -> Result<Option<Facts>, String> {
    let base = format!("/{}", go_escape(module));
    let Some(latest_doc) = fetch(http, Lang::Go, &format!("{base}/@latest"))? else {
        return Ok(None);
    };
    let latest = text(&latest_doc, "/Version");
    let repository = text(&latest_doc, "/Origin/URL").or_else(|| {
        let mut parts = module.split('/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(host @ ("github.com" | "gitlab.com" | "codeberg.org")), Some(a), Some(b)) => {
                Some(format!("https://{host}/{a}/{b}"))
            }
            _ => None,
        }
    });
    // The latest go.mod says whether the module is deprecated and which
    // versions are retracted.
    let (deprecated, retracted) = match &latest {
        Some(v) => {
            let resp = http.get(
                Service::Registry(Lang::Go),
                &format!("{base}/@v/{}.mod", go_escape(v)),
            )?;
            if resp.status == 200 {
                go_mod_notes(&resp.body)
            } else {
                (None, Vec::new())
            }
        }
        None => (None, Vec::new()),
    };
    let release = match version {
        None => None,
        Some(v) => {
            let v = if v.starts_with('v') {
                v.to_string()
            } else {
                format!("v{v}")
            };
            let path = format!("{base}/@v/{}.info", go_escape(&v));
            Some(match fetch(http, Lang::Go, &path)? {
                None => missing_release(&v),
                Some(_) => Release {
                    yanked: retracted
                        .iter()
                        .find(|r| r.covers(&v))
                        .map(|r| r.reason.clone()),
                    version: v,
                    exists: true,
                    deprecated: None,
                    license: None,
                },
            })
        }
    };
    Ok(Some(Facts {
        name: module.to_string(),
        latest,
        license: None,
        repository,
        deprecated,
        release,
    }))
}

/// A `retract` directive: one version or a closed range, and its comment.
#[derive(Debug, PartialEq)]
struct Retraction {
    low: String,
    high: String,
    reason: String,
}

impl Retraction {
    fn covers(&self, version: &str) -> bool {
        version::compare(version, &self.low).is_ge()
            && version::compare(version, &self.high).is_le()
    }
}

/// A go.mod's `// Deprecated:` comment and its retractions.
fn go_mod_notes(text: &str) -> (Option<String>, Vec<Retraction>) {
    let mut deprecated = None;
    let mut retracted = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let line = line.trim();
        if deprecated.is_none()
            && let Some(msg) = line.split_once("// Deprecated:").map(|(_, m)| m.trim())
        {
            deprecated = Some(msg.to_string());
        }
        let entry = if in_block {
            if line == ")" {
                in_block = false;
                continue;
            }
            line
        } else if let Some(rest) = line.strip_prefix("retract") {
            let rest = rest.trim();
            if rest == "(" {
                in_block = true;
                continue;
            }
            rest
        } else {
            continue;
        };
        let (spec, reason) = entry
            .split_once("//")
            .map_or((entry, ""), |(s, r)| (s, r.trim()));
        let spec = spec.trim();
        let (low, high) = match spec.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            Some(range) => match range.split_once(',') {
                Some((l, h)) => (l.trim().to_string(), h.trim().to_string()),
                None => continue,
            },
            None if !spec.is_empty() => (spec.to_string(), spec.to_string()),
            None => continue,
        };
        retracted.push(Retraction {
            low,
            high,
            reason: reason.to_string(),
        });
    }
    (deprecated, retracted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npm_repositories_become_urls() {
        let url = |s: &str| repository_url(s);
        assert_eq!(
            url("git+https://github.com/lodash/lodash.git").as_deref(),
            Some("https://github.com/lodash/lodash")
        );
        assert_eq!(
            url("git@github.com:a/b.git").as_deref(),
            Some("https://github.com/a/b")
        );
        assert_eq!(url("github:a/b").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(url("a/b").as_deref(), Some("https://github.com/a/b"));
        assert_eq!(url("file:../x"), None);
    }

    #[test]
    fn a_go_mod_names_its_deprecation_and_retractions() {
        let (deprecated, retracted) = go_mod_notes(
            "// Deprecated: Use the \"google.golang.org/protobuf\" module instead.\nmodule github.com/golang/protobuf\n\nretract v1.0.1 // published by mistake\nretract (\n\t[v1.2.0, v1.2.3] // broken\n\tv1.4.0\n)\n",
        );
        assert_eq!(
            deprecated.as_deref(),
            Some("Use the \"google.golang.org/protobuf\" module instead.")
        );
        assert_eq!(retracted.len(), 3);
        assert_eq!(retracted[0].reason, "published by mistake");
        assert!(retracted[1].covers("v1.2.2"));
        assert!(!retracted[1].covers("v1.2.4"));
        assert!(retracted[2].covers("v1.4.0"));
    }

    #[test]
    fn names_are_encoded_for_paths() {
        assert_eq!(npm_path("@types/node"), "/@types%2Fnode");
        assert_eq!(npm_path("lodash"), "/lodash");
        assert_eq!(encode("a b/c"), "a%20b%2Fc");
        assert_eq!(distance("tokioo", "tokio"), 1);
    }
}
