//! OSV (osv.dev): the advisories that affect package versions.
//!
//! One `querybatch` request asks about up to 1000 (ecosystem, name,
//! version) triples and answers with advisory IDs; each advisory's details
//! come from `/v1/vulns/ID`, fetched in parallel, cached for a day, and
//! shared by every package it affects. OSV lists one vulnerability under
//! each database that published it (GHSA and RUSTSEC, say), so advisories
//! that alias each other are reported once.

use super::cache;
use super::http::{Http, Service};
use super::{same_name, version};
use crate::docs::Lang;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// The most queries OSV takes in one batch.
const BATCH: usize = 1000;
/// Advisory details fetched at once.
const THREADS: usize = 8;

/// An advisory as `jouet deps` reports it.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Advisory {
    pub(crate) id: String,
    /// The same advisory in other databases: CVE, GHSA, RUSTSEC, PYSEC.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) aliases: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    /// `low`, `medium`, `high` or `critical`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) severity: Option<String>,
    /// RustSec's kind for an advisory that is not a vulnerability:
    /// `unmaintained`, `unsound` or `notice`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) informational: Option<String>,
    /// The first version after the one checked that fixes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fixed: Option<String>,
    pub(crate) url: String,
}

/// OSV's name for an ecosystem.
fn ecosystem(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "crates.io",
        Lang::Ts => "npm",
        Lang::Python => "PyPI",
        Lang::Go => "Go",
    }
}

/// A package version to ask about.
#[derive(Debug, Clone)]
pub(crate) struct Query {
    pub(crate) lang: Lang,
    pub(crate) name: String,
    pub(crate) version: String,
}

impl Query {
    /// Go's versions start with `v`, as go.mod writes them.
    fn osv_version(&self) -> String {
        if self.lang == Lang::Go && !self.version.starts_with('v') {
            format!("v{}", self.version)
        } else {
            self.version.clone()
        }
    }

    fn cache_key(&self) -> String {
        format!("{}@{}", self.name, self.osv_version())
    }
}

/// What jouet keeps of one OSV record.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Vuln {
    id: String,
    #[serde(default)]
    aliases: Vec<String>,
    summary: Option<String>,
    severity: Option<String>,
    informational: Option<String>,
    #[serde(default)]
    withdrawn: bool,
    #[serde(default)]
    affected: Vec<Affected>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Affected {
    ecosystem: String,
    name: String,
    /// Every `fixed` event of its ranges.
    fixed: Vec<String>,
}

/// The advisories affecting each query's version, in the queries' order.
pub(crate) fn advisories(http: &Http, queries: &[Query]) -> Result<Vec<Vec<Advisory>>, String> {
    let mut ids: Vec<Option<Vec<String>>> = queries
        .iter()
        .map(|q| {
            if http.refresh {
                return None;
            }
            cache::read(&["osv", ecosystem(q.lang), &q.cache_key()], cache::OSV_TTL)
        })
        .collect();
    let missing: Vec<usize> = (0..queries.len()).filter(|&i| ids[i].is_none()).collect();
    for chunk in missing.chunks(BATCH) {
        let body = serde_json::json!({
            "queries": chunk.iter().map(|&i| {
                let q = &queries[i];
                serde_json::json!({
                    "package": { "name": q.name, "ecosystem": ecosystem(q.lang) },
                    "version": q.osv_version(),
                })
            }).collect::<Vec<_>>()
        });
        let resp = http.post(Service::Osv, "/v1/querybatch", &body.to_string())?;
        if resp.status != 200 {
            return Err(format!("OSV returned {}", resp.status));
        }
        let doc: Value = serde_json::from_str(&resp.body)
            .map_err(|e| format!("OSV sent an answer jouet could not read: {e}"))?;
        let results = doc
            .get("results")
            .and_then(Value::as_array)
            .filter(|r| r.len() == chunk.len())
            .ok_or("OSV's answer did not match the questions")?;
        for (&i, result) in chunk.iter().zip(results) {
            let found: Vec<String> = result
                .get("vulns")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.get("id")?.as_str().map(str::to_string))
                .collect();
            let q = &queries[i];
            cache::write(&["osv", ecosystem(q.lang), &q.cache_key()], &found);
            ids[i] = Some(found);
        }
    }
    let ids: Vec<Vec<String>> = ids.into_iter().map(Option::unwrap_or_default).collect();
    let mut unique: Vec<&String> = ids.iter().flatten().collect();
    unique.sort();
    unique.dedup();
    let vulns = details(http, &unique)?;
    Ok(queries
        .iter()
        .zip(&ids)
        .map(|(q, ids)| {
            let found: Vec<&Vuln> = ids
                .iter()
                .filter_map(|id| vulns.get(id))
                .filter(|v| !v.withdrawn)
                .collect();
            merge(q, &found)
        })
        .collect())
}

/// Each advisory's record, from the cache or OSV.
fn details(http: &Http, ids: &[&String]) -> Result<HashMap<String, Vuln>, String> {
    let mut out: HashMap<String, Vuln> = HashMap::new();
    let mut fetch: Vec<&String> = Vec::new();
    for id in ids {
        let cached = (!http.refresh)
            .then(|| cache::read::<Vuln>(&["osv", "vulns", id], cache::OSV_TTL))
            .flatten();
        match cached {
            Some(v) => {
                out.insert((*id).clone(), v);
            }
            None => fetch.push(id),
        }
    }
    let chunk = fetch.len().div_ceil(THREADS).max(1);
    let fetched: Vec<Result<Vuln, String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = fetch
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|id| {
                            let resp = http.get(Service::Osv, &format!("/v1/vulns/{id}"))?;
                            if resp.status != 200 {
                                return Err(format!("OSV returned {} for {id}", resp.status));
                            }
                            let doc: Value = serde_json::from_str(&resp.body).map_err(|e| {
                                format!("OSV sent an answer jouet could not read: {e}")
                            })?;
                            let vuln = trim(&doc);
                            cache::write(&["osv", "vulns", id], &vuln);
                            Ok(vuln)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| {
                w.join()
                    .unwrap_or_else(|_| vec![Err("an OSV request failed".into())])
            })
            .collect()
    });
    for vuln in fetched {
        let vuln = vuln?;
        out.insert(vuln.id.clone(), vuln);
    }
    Ok(out)
}

/// The parts of an OSV record jouet reports.
fn trim(doc: &Value) -> Vuln {
    let text = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let affected_list: Vec<&Value> = doc
        .get("affected")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let affected = affected_list
        .iter()
        .filter_map(|a| {
            let package = a.get("package")?;
            Some(Affected {
                ecosystem: text(package, "ecosystem")?,
                name: text(package, "name")?,
                fixed: a
                    .get("ranges")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .flat_map(|r| {
                        r.get("events")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                    })
                    .filter_map(|e| e.get("fixed")?.as_str().map(str::to_string))
                    .collect(),
            })
        })
        .collect();
    let id = text(doc, "id").unwrap_or_default();
    // OSV's MAL- records are packages published to attack their users.
    let informational = if id.starts_with("MAL-") {
        Some("malicious".to_string())
    } else {
        affected_list.iter().find_map(|a| {
            a.pointer("/database_specific/informational")?
                .as_str()
                .map(str::to_string)
        })
    };
    Vuln {
        id,
        aliases: doc
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str().map(str::to_string))
            .collect(),
        summary: text(doc, "summary"),
        severity: severity(doc),
        informational,
        withdrawn: doc.get("withdrawn").is_some_and(|w| !w.is_null()),
        affected,
    }
}

/// GitHub's rating where there is one, else the rating of a CVSS 3 vector.
fn severity(doc: &Value) -> Option<String> {
    if let Some(rating) = doc
        .pointer("/database_specific/severity")
        .and_then(Value::as_str)
    {
        let rating = match rating.to_ascii_lowercase().as_str() {
            "moderate" | "medium" => "medium",
            "low" => "low",
            "high" => "high",
            "critical" => "critical",
            _ => return None,
        };
        return Some(rating.to_string());
    }
    doc.get("severity")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| {
            s.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t == "CVSS_V3")
        })
        .find_map(|s| cvss3(s.get("score")?.as_str()?))
        .map(|score| rating(score).to_string())
}

fn rank(severity: Option<&str>) -> u8 {
    match severity {
        Some("critical") => 4,
        Some("high") => 3,
        Some("medium") => 2,
        Some("low") => 1,
        _ => 0,
    }
}

fn rating(score: f64) -> &'static str {
    if score >= 9.0 {
        "critical"
    } else if score >= 7.0 {
        "high"
    } else if score >= 4.0 {
        "medium"
    } else if score > 0.0 {
        "low"
    } else {
        "none"
    }
}

/// A CVSS 3.0 or 3.1 vector's base score.
fn cvss3(vector: &str) -> Option<f64> {
    let metrics: HashMap<&str, &str> = vector
        .split('/')
        .skip(1)
        .filter_map(|m| m.split_once(':'))
        .collect();
    let get = |key: &str| metrics.get(key).copied();
    let changed = get("S")? == "C";
    let av = match get("AV")? {
        "N" => 0.85,
        "A" => 0.62,
        "L" => 0.55,
        "P" => 0.2,
        _ => return None,
    };
    let ac = match get("AC")? {
        "L" => 0.77,
        "H" => 0.44,
        _ => return None,
    };
    let pr = match (get("PR")?, changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match get("UI")? {
        "N" => 0.85,
        "R" => 0.62,
        _ => return None,
    };
    let cia = |key: &str| -> Option<f64> {
        Some(match get(key)? {
            "H" => 0.56,
            "L" => 0.22,
            "N" => 0.0,
            _ => return None,
        })
    };
    let iss = 1.0 - (1.0 - cia("C")?) * (1.0 - cia("I")?) * (1.0 - cia("A")?);
    let impact = if changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02).powi(15)
    } else {
        6.42 * iss
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability = 8.22 * av * ac * pr * ui;
    let base = if changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };
    Some(round_up(base))
}

/// CVSS 3.1's Roundup: the smallest one-decimal number not below `x`.
fn round_up(x: f64) -> f64 {
    // Whole hundred-thousandths, which f64 holds exactly at this scale.
    let int = (x * 100_000.0).round() as i64;
    if int % 10_000 == 0 {
        int as f64 / 100_000.0
    } else {
        ((int / 10_000) + 1) as f64 / 10.0
    }
}

/// The ID a project in this ecosystem knows an advisory by, the first of
/// these prefixes found.
fn preferred(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => &["RUSTSEC-", "GHSA-"],
        Lang::Go => &["GO-", "GHSA-"],
        Lang::Ts | Lang::Python => &["GHSA-", "PYSEC-"],
    }
}

/// One advisory per vulnerability, however many databases list it.
fn merge(q: &Query, vulns: &[&Vuln]) -> Vec<Advisory> {
    let names = |v: &Vuln| -> HashSet<String> {
        std::iter::once(v.id.clone())
            .chain(v.aliases.iter().cloned())
            .collect()
    };
    let mut groups: Vec<(HashSet<String>, Vec<&Vuln>)> = Vec::new();
    for v in vulns {
        let mine = names(v);
        let mut joined: Vec<&Vuln> = vec![v];
        let mut all = mine;
        // A vulnerability may join groups that were apart until now.
        groups.retain(|(keys, members)| {
            if keys.is_disjoint(&all) {
                true
            } else {
                joined.extend(members.iter().copied());
                all.extend(keys.iter().cloned());
                false
            }
        });
        groups.push((all, joined));
    }
    let version = q.osv_version();
    let mut out: Vec<Advisory> = groups
        .into_iter()
        .map(|(keys, members)| {
            let rank_id = |id: &str| {
                preferred(q.lang)
                    .iter()
                    .position(|p| id.starts_with(p))
                    .unwrap_or(usize::MAX)
            };
            let primary = members
                .iter()
                .min_by_key(|v| (rank_id(&v.id), v.id.clone()))
                .copied()
                .unwrap_or(members[0]);
            let mut aliases: Vec<String> = keys.into_iter().filter(|k| *k != primary.id).collect();
            aliases.sort();
            let fixed = members
                .iter()
                .flat_map(|v| &v.affected)
                .filter(|a| a.ecosystem == ecosystem(q.lang) && same_name(q.lang, &a.name, &q.name))
                .flat_map(|a| &a.fixed)
                .filter(|f| version::compare(f, &version).is_gt())
                .min_by(|a, b| version::compare(a, b))
                .cloned();
            Advisory {
                id: primary.id.clone(),
                aliases,
                summary: primary
                    .summary
                    .clone()
                    .or_else(|| members.iter().find_map(|v| v.summary.clone())),
                severity: members
                    .iter()
                    .filter_map(|v| v.severity.clone())
                    .max_by_key(|s| rank(Some(s))),
                informational: members.iter().find_map(|v| v.informational.clone()),
                fixed,
                url: format!("https://osv.dev/vulnerability/{}", primary.id),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        rank(b.severity.as_deref())
            .cmp(&rank(a.severity.as_deref()))
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cvss3_vectors_score_as_the_calculator_does() {
        let score = |v: &str| cvss3(v).unwrap();
        assert!((score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H") - 9.8).abs() < 1e-9);
        assert!((score("CVSS:3.1/AV:L/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H") - 6.2).abs() < 1e-9);
        assert!((score("CVSS:3.1/AV:N/AC:L/PR:L/UI:N/S:C/C:L/I:L/A:N") - 6.4).abs() < 1e-9);
        assert!((score("CVSS:3.0/AV:N/AC:H/PR:N/UI:R/S:U/C:L/I:N/A:N") - 3.1).abs() < 1e-9);
        assert_eq!(
            cvss3("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N"),
            Some(0.0)
        );
        assert_eq!(cvss3("CVSS:3.1/AV:X"), None);
        assert_eq!(rating(9.8), "critical");
        assert_eq!(rating(6.2), "medium");
    }

    fn vuln(id: &str, aliases: &[&str], severity: Option<&str>, fixed: &[&str]) -> Vuln {
        Vuln {
            id: id.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            summary: Some(format!("summary of {id}")),
            severity: severity.map(str::to_string),
            informational: None,
            withdrawn: false,
            affected: vec![Affected {
                ecosystem: "crates.io".into(),
                name: "time".into(),
                fixed: fixed.iter().map(|f| f.to_string()).collect(),
            }],
        }
    }

    #[test]
    fn aliased_advisories_are_one_with_the_first_fix_after_the_version() {
        let q = Query {
            lang: Lang::Rust,
            name: "time".into(),
            version: "0.2.7".into(),
        };
        let ghsa = vuln(
            "GHSA-wcg3-cvx6-7396",
            &["CVE-2020-26235", "RUSTSEC-2020-0071"],
            Some("medium"),
            &["0.2.23"],
        );
        let rustsec = vuln(
            "RUSTSEC-2020-0071",
            &["CVE-2020-26235", "GHSA-wcg3-cvx6-7396"],
            None,
            &["0.2.0", "0.2.6", "0.2.23"],
        );
        let other = vuln("RUSTSEC-2099-0001", &[], Some("high"), &["0.3.0"]);
        let merged = merge(&q, &[&ghsa, &rustsec, &other]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "RUSTSEC-2099-0001");
        let time = &merged[1];
        assert_eq!(time.id, "RUSTSEC-2020-0071");
        assert_eq!(time.aliases, vec!["CVE-2020-26235", "GHSA-wcg3-cvx6-7396"]);
        assert_eq!(time.severity.as_deref(), Some("medium"));
        assert_eq!(time.fixed.as_deref(), Some("0.2.23"));
        assert_eq!(
            time.summary.as_deref(),
            Some("summary of RUSTSEC-2020-0071")
        );
    }
}
