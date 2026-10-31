//! `jouet deps`: whether a package exists, its latest version, licence,
//! repository and deprecation, from its registry (crates.io, npm, PyPI, the
//! Go module proxy), and the OSV advisories that affect a version of it.
//!
//! Only package names and versions leave the machine: a registry is asked
//! about one package by name, and OSV about (ecosystem, name, version)
//! triples. A package its lockfile says comes from a private registry, a
//! path or a git repository is never sent ([`crate::docs::Pin::public`]).
//!
//! Answers are cached under jouet's cache directory: a registry's for ten
//! minutes, OSV's for a day.

mod cache;
mod http;
pub(crate) mod osv;
pub(crate) mod registry;
mod version;

pub(crate) use http::Http;
pub(crate) use version::compare;

use crate::docs::Lang;

/// How `jouet deps` names an ecosystem: the prefix of `cargo:serde`.
pub(crate) fn ecosystem(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "cargo",
        Lang::Ts => "npm",
        Lang::Python => "pypi",
        Lang::Go => "go",
    }
}

/// The registry that answers for an ecosystem, as a person names it.
pub(crate) fn registry_name(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "crates.io",
        Lang::Ts => "npm",
        Lang::Python => "PyPI",
        Lang::Go => "the Go module proxy",
    }
}

/// A package name as its ecosystem compares names: PyPI's are
/// case-insensitive with `-`, `_` and `.` alike, crates.io's alike in `-`
/// and `_`.
pub(crate) fn same_name(lang: Lang, a: &str, b: &str) -> bool {
    match lang {
        Lang::Python => {
            let norm = |s: &str| s.to_ascii_lowercase().replace(['_', '.'], "-");
            norm(a) == norm(b)
        }
        Lang::Rust => a.replace('-', "_") == b.replace('-', "_"),
        Lang::Ts | Lang::Go => a == b,
    }
}
