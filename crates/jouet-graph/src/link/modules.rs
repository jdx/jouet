//! Module resolution: the file (or files) an import names, per language.
//!
//! codegraph's `import-resolver.ts`, reduced to the rules that serve the
//! languages jouet extracts: a JS/TS specifier is found the way the runtime
//! finds it (extensions, `index` files); a Python module by its dotted path
//! from any package root; a Go import path through `go.mod`; a Rust `use`
//! path through the crate's module files; a Java import by its package
//! directories; a Ruby `require` and a C `#include` relative to the file,
//! then anywhere its path fits. tsconfig path aliases, Python namespace
//! packages and C include directories from build files are not read.

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The work tree's indexed files, with lookups by path suffix.
pub(crate) struct Files {
    root: std::path::PathBuf,
    all: HashSet<String>,
    /// Files by their dotted module path's suffixes (`requests.sessions`,
    /// `sessions`), for Python; and by package path for Java
    /// (`com.google.gson.Gson`).
    dotted: HashMap<String, Vec<String>>,
    /// Files by every `/`-suffix of their path (`job_util.rb`,
    /// `sidekiq/job_util.rb`), for Ruby, C and Go package directories.
    suffixes: HashMap<String, Vec<String>>,
    /// Files in each directory.
    dirs: HashMap<String, Vec<String>>,
    go_modules: std::cell::RefCell<HashMap<String, Option<(String, String)>>>,
    rust_crates: std::cell::RefCell<HashMap<String, Option<RustCrate>>>,
    rust_by_name: std::cell::RefCell<Option<HashMap<String, RustCrate>>>,
}

#[derive(Clone)]
pub(crate) struct RustCrate {
    /// The crate's `src/` directory, relative to the work tree.
    src: String,
    /// `lib.rs` or `main.rs`.
    root_file: String,
}

pub(crate) fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, f)| f)
}

pub(crate) fn stem(path: &str) -> &str {
    let name = file_name(path);
    name.split_once('.').map_or(name, |(s, _)| s)
}

/// `a/b/../c/./d` → `a/c/d`; `None` when it climbs out of the work tree.
pub(crate) fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            p => out.push(p),
        }
    }
    Some(out.join("/"))
}

fn join(dir: &str, rel: &str) -> Option<String> {
    if dir.is_empty() {
        normalize(rel)
    } else {
        normalize(&format!("{dir}/{rel}"))
    }
}

impl Files {
    pub(crate) fn new(root: &Path, paths: impl IntoIterator<Item = String>) -> Files {
        let mut files = Files {
            root: root.to_path_buf(),
            all: HashSet::new(),
            dotted: HashMap::new(),
            suffixes: HashMap::new(),
            dirs: HashMap::new(),
            go_modules: Default::default(),
            rust_crates: Default::default(),
            rust_by_name: Default::default(),
        };
        for path in paths {
            let parts: Vec<&str> = path.split('/').collect();
            for i in 0..parts.len() {
                files
                    .suffixes
                    .entry(parts[i..].join("/"))
                    .or_default()
                    .push(path.clone());
            }
            if path.ends_with(".py") || path.ends_with(".java") {
                let mut segs: Vec<&str> = parts.clone();
                let last = segs.pop().unwrap_or_default();
                let module = last.rsplit_once('.').map_or(last, |(s, _)| s);
                if module != "__init__" {
                    segs.push(module);
                }
                for i in 0..segs.len() {
                    files
                        .dotted
                        .entry(segs[i..].join("."))
                        .or_default()
                        .push(path.clone());
                }
            }
            files
                .dirs
                .entry(dir_of(&path).to_string())
                .or_default()
                .push(path.clone());
            files.all.insert(path);
        }
        for list in files.suffixes.values_mut().chain(files.dotted.values_mut()) {
            // Shortest first: the likeliest of several namesakes is the one
            // nearest the root.
            list.sort_by_key(|p| (p.matches('/').count(), p.clone()));
        }
        files
    }

    pub(crate) fn contains(&self, path: &str) -> bool {
        self.all.contains(path)
    }

    pub(crate) fn in_dir(&self, dir: &str) -> &[String] {
        self.dirs.get(dir).map_or(&[], Vec::as_slice)
    }

    fn first_with_suffix(&self, suffix: &str) -> Option<String> {
        self.suffixes.get(suffix).and_then(|v| v.first()).cloned()
    }

    /// The files `specifier`, imported from `from` in `language`, names.
    pub(crate) fn resolve(&self, language: &str, from: &str, specifier: &str) -> Vec<String> {
        match language {
            "typescript" | "tsx" | "javascript" | "jsx" => {
                self.js(from, specifier).into_iter().collect()
            }
            "python" => self.python(from, specifier).into_iter().collect(),
            "go" => self.go(from, specifier),
            "rust" => self
                .rust_module(from, specifier)
                .map(|(f, _)| f)
                .into_iter()
                .collect(),
            "java" => self.java(specifier).into_iter().collect(),
            "ruby" => self.ruby(from, specifier).into_iter().collect(),
            "c" | "cpp" => self.c(from, specifier).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    fn js(&self, from: &str, specifier: &str) -> Option<String> {
        if !specifier.starts_with('.') {
            return None;
        }
        let base = join(dir_of(from), specifier)?;
        let exts = ["ts", "tsx", "d.ts", "js", "jsx", "mjs", "cjs", "mts", "cts"];
        let mut tries = vec![base.clone()];
        // TypeScript's ESM spelling names the emitted `.js` of a `.ts` file.
        for (written, source) in [
            (".js", ".ts"),
            (".jsx", ".tsx"),
            (".mjs", ".mts"),
            (".cjs", ".cts"),
        ] {
            if let Some(s) = base.strip_suffix(written) {
                tries.push(format!("{s}{source}"));
            }
        }
        tries.extend(exts.iter().map(|e| format!("{base}.{e}")));
        tries.extend(exts.iter().map(|e| format!("{base}/index.{e}")));
        tries.into_iter().find(|t| self.contains(t))
    }

    fn python(&self, from: &str, specifier: &str) -> Option<String> {
        let dots = specifier.chars().take_while(|c| *c == '.').count();
        let rest = &specifier[dots..];
        if dots > 0 {
            let mut dir = dir_of(from).to_string();
            for _ in 1..dots {
                dir = dir_of(&dir).to_string();
            }
            let rel = rest.replace('.', "/");
            let base = if rel.is_empty() {
                dir
            } else {
                join(&dir, &rel)?
            };
            return [format!("{base}.py"), format!("{base}/__init__.py")]
                .into_iter()
                .find(|t| self.contains(t));
        }
        self.dotted.get(rest).and_then(|v| v.first()).cloned()
    }

    fn java(&self, specifier: &str) -> Option<String> {
        // `a.b.C`, or a static member `a.b.C.m`, or a nested `a.b.C.D`.
        let mut path = specifier.trim_end_matches(".*");
        loop {
            if path.contains('.')
                && let Some(f) = self.dotted.get(path).and_then(|v| v.first())
            {
                return Some(f.clone());
            }
            path = path.rsplit_once('.')?.0;
        }
    }

    /// The package directory a Java wildcard or an import's package names.
    pub(crate) fn java_package_files(&self, package: &str) -> Vec<String> {
        let suffix = package.replace('.', "/");
        self.dirs
            .iter()
            .filter(|(d, _)| *d == &suffix || d.ends_with(&format!("/{suffix}")))
            .flat_map(|(_, f)| f.iter().filter(|p| p.ends_with(".java")).cloned())
            .collect()
    }

    fn ruby(&self, from: &str, specifier: &str) -> Option<String> {
        let with_ext = if specifier.ends_with(".rb") {
            specifier.to_string()
        } else {
            format!("{specifier}.rb")
        };
        if let Some(p) = join(dir_of(from), &with_ext).filter(|p| self.contains(p)) {
            return Some(p);
        }
        self.first_with_suffix(with_ext.trim_start_matches("./"))
    }

    fn c(&self, from: &str, specifier: &str) -> Option<String> {
        if let Some(p) = join(dir_of(from), specifier).filter(|p| self.contains(p)) {
            return Some(p);
        }
        self.first_with_suffix(specifier.trim_start_matches("./"))
    }

    /// The `go.mod` governing `dir`: its directory and module path.
    fn go_module(&self, dir: &str) -> Option<(String, String)> {
        if let Some(hit) = self.go_modules.borrow().get(dir) {
            return hit.clone();
        }
        let here = std::fs::read_to_string(self.root.join(dir).join("go.mod"))
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|l| l.trim().strip_prefix("module "))
                    .map(|m| (dir.to_string(), m.trim().trim_matches('"').to_string()))
            });
        let found = here.or_else(|| {
            (!dir.is_empty())
                .then(|| self.go_module(dir_of(dir)))
                .flatten()
        });
        self.go_modules
            .borrow_mut()
            .insert(dir.to_string(), found.clone());
        found
    }

    /// A Go import path's package: the non-test files of its directory.
    fn go(&self, from: &str, specifier: &str) -> Vec<String> {
        let Some((mod_dir, module)) = self.go_module(dir_of(from)) else {
            return Vec::new();
        };
        let rel = if specifier == module {
            ""
        } else if let Some(r) = specifier.strip_prefix(&format!("{module}/")) {
            r
        } else {
            return Vec::new();
        };
        let dir = match (mod_dir.is_empty(), rel.is_empty()) {
            (true, _) => rel.to_string(),
            (false, true) => mod_dir,
            (false, false) => format!("{mod_dir}/{rel}"),
        };
        self.in_dir(&dir)
            .iter()
            .filter(|p| p.ends_with(".go") && !p.ends_with("_test.go"))
            .cloned()
            .collect()
    }

    /// The Rust crate `path` belongs to: the nearest `Cargo.toml` above it
    /// with a `src/lib.rs` or `src/main.rs`.
    fn rust_crate(&self, dir: &str) -> Option<RustCrate> {
        if let Some(hit) = self.rust_crates.borrow().get(dir) {
            return hit.clone();
        }
        let root_in = |src: &str| {
            ["lib.rs", "main.rs"]
                .iter()
                .map(|f| {
                    if src.is_empty() {
                        f.to_string()
                    } else {
                        format!("{src}/{f}")
                    }
                })
                .find(|f| self.contains(f))
                .map(|root_file| RustCrate {
                    src: src.to_string(),
                    root_file,
                })
        };
        // A crate root beside its modules (a `[[bin]]` or `[lib]` path such
        // as ripgrep's `crates/core/main.rs`), or a manifest's `src/`.
        let here = root_in(dir).or_else(|| {
            self.root.join(dir).join("Cargo.toml").is_file().then(|| {
                root_in(&if dir.is_empty() {
                    "src".to_string()
                } else {
                    format!("{dir}/src")
                })
            })?
        });
        let found = match here {
            Some(c) => Some(c),
            None if dir.is_empty() => None,
            None => self.rust_crate(dir_of(dir)),
        };
        self.rust_crates
            .borrow_mut()
            .insert(dir.to_string(), found.clone());
        found
    }

    /// The source directory of the Rust crate `path` belongs to.
    pub(crate) fn rust_crate_src(&self, path: &str) -> Option<String> {
        self.rust_crate(dir_of(path)).map(|c| c.src)
    }

    /// The work tree's Rust crates by the name code uses for them
    /// (`jouet_output` for the package `jouet-output`).
    fn rust_crate_named(&self, name: &str) -> Option<RustCrate> {
        if self.rust_by_name.borrow().is_none() {
            let mut map = HashMap::new();
            for path in &self.all {
                if !path.ends_with("/lib.rs") && path != "src/lib.rs" {
                    continue;
                }
                let src = dir_of(path);
                let krate = dir_of(src);
                let Ok(manifest) =
                    std::fs::read_to_string(self.root.join(krate).join("Cargo.toml"))
                else {
                    continue;
                };
                let package = manifest
                    .lines()
                    .skip_while(|l| l.trim() != "[package]")
                    .find_map(|l| {
                        let (k, v) = l.split_once('=')?;
                        (k.trim() == "name").then(|| v.trim().trim_matches('"').replace('-', "_"))
                    });
                if let Some(package) = package {
                    map.insert(
                        package,
                        RustCrate {
                            src: src.to_string(),
                            root_file: path.clone(),
                        },
                    );
                }
            }
            *self.rust_by_name.borrow_mut() = Some(map);
        }
        self.rust_by_name
            .borrow()
            .as_ref()
            .and_then(|m| m.get(name).cloned())
    }

    /// The module file of a Rust path's longest prefix that names one, and
    /// the segments left after it (`crate::cli::args::ToolArg` →
    /// `src/cli/args.rs`, `["ToolArg"]`).
    pub(crate) fn rust_module(&self, from: &str, path: &str) -> Option<(String, Vec<String>)> {
        let segs: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
        let (first, rest) = segs.split_first()?;
        let krate = match *first {
            "crate" => self.rust_crate(dir_of(from))?,
            "self" | "super" => {
                return self.rust_relative(from, first, rest);
            }
            name => self.rust_crate_named(name)?,
        };
        let mut best = (krate.root_file.clone(), 0);
        let mut dir = krate.src.clone();
        for (i, seg) in rest.iter().enumerate() {
            let file = format!("{dir}/{seg}.rs");
            let module = format!("{dir}/{seg}/mod.rs");
            let hit = [file, module].into_iter().find(|f| self.contains(f));
            match hit {
                Some(f) => best = (f, i + 1),
                None => break,
            }
            dir = format!("{dir}/{seg}");
        }
        Some((
            best.0,
            rest[best.1..].iter().map(|s| s.to_string()).collect(),
        ))
    }

    /// `self::x` and `super::x` from `from`.
    fn rust_relative(
        &self,
        from: &str,
        first: &str,
        rest: &[&str],
    ) -> Option<(String, Vec<String>)> {
        // The directory a file's own child modules live in.
        let name = file_name(from);
        let mut dir = if matches!(name, "mod.rs" | "lib.rs" | "main.rs") {
            dir_of(from).to_string()
        } else {
            format!("{}/{}", dir_of(from), stem(from))
        };
        let mut owner = from.to_string();
        let mut rest = rest;
        if first == "super" {
            dir = dir_of(&dir).to_string();
            owner = [
                format!("{dir}.rs"),
                format!("{dir}/mod.rs"),
                format!("{dir}/lib.rs"),
                format!("{dir}/main.rs"),
            ]
            .into_iter()
            .find(|f| self.contains(f))?;
            while rest.first() == Some(&"super") {
                rest = &rest[1..];
                dir = dir_of(&dir).to_string();
                owner = [format!("{dir}.rs"), format!("{dir}/mod.rs")]
                    .into_iter()
                    .find(|f| self.contains(f))?;
            }
        }
        let mut best = (owner, 0);
        for (i, seg) in rest.iter().enumerate() {
            let hit = [format!("{dir}/{seg}.rs"), format!("{dir}/{seg}/mod.rs")]
                .into_iter()
                .find(|f| self.contains(f));
            match hit {
                Some(f) => best = (f, i + 1),
                None => break,
            }
            dir = format!("{dir}/{seg}");
        }
        Some((
            best.0,
            rest[best.1..].iter().map(|s| s.to_string()).collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Files {
        Files::new(
            Path::new("/nonexistent"),
            paths.iter().map(|p| p.to_string()),
        )
    }

    #[test]
    fn js_specifiers_find_extensions_and_index_files() {
        let f = files(&["src/a.ts", "src/lib/index.tsx", "src/util.ts", "web/x.js"]);
        assert_eq!(f.resolve("typescript", "src/b.ts", "./a"), ["src/a.ts"]);
        assert_eq!(f.resolve("typescript", "src/b.ts", "./a.js"), ["src/a.ts"]);
        assert_eq!(f.resolve("tsx", "src/b.ts", "./lib"), ["src/lib/index.tsx"]);
        assert_eq!(f.resolve("javascript", "web/y/z.js", "../x"), ["web/x.js"]);
        assert!(f.resolve("typescript", "src/b.ts", "react").is_empty());
        assert!(
            f.resolve("typescript", "src/b.ts", "../../../up")
                .is_empty()
        );
    }

    #[test]
    fn python_modules_resolve_relative_and_from_any_package_root() {
        let f = files(&[
            "src/requests/__init__.py",
            "src/requests/sessions.py",
            "src/requests/adapters.py",
        ]);
        assert_eq!(
            f.resolve("python", "src/requests/sessions.py", ".adapters"),
            ["src/requests/adapters.py"]
        );
        assert_eq!(
            f.resolve("python", "src/requests/sessions.py", "."),
            ["src/requests/__init__.py"]
        );
        assert_eq!(
            f.resolve("python", "tests/t.py", "requests.sessions"),
            ["src/requests/sessions.py"]
        );
        assert_eq!(
            f.resolve("python", "tests/t.py", "requests"),
            ["src/requests/__init__.py"]
        );
        assert!(f.resolve("python", "tests/t.py", "os").is_empty());
    }

    #[test]
    fn java_imports_find_the_class_file_and_static_members() {
        let f = files(&["gson/src/main/java/com/google/gson/Gson.java"]);
        let want = ["gson/src/main/java/com/google/gson/Gson.java"];
        assert_eq!(f.resolve("java", "x/Y.java", "com.google.gson.Gson"), want);
        assert_eq!(
            f.resolve("java", "x/Y.java", "com.google.gson.Gson.toJson"),
            want
        );
        assert!(f.resolve("java", "x/Y.java", "java.util.List").is_empty());
    }

    #[test]
    fn ruby_and_c_resolve_relative_then_by_suffix() {
        let f = files(&["lib/sidekiq/job_util.rb", "src/server.h", "db/db_impl.h"]);
        assert_eq!(
            f.resolve("ruby", "lib/sidekiq/client.rb", "sidekiq/job_util"),
            ["lib/sidekiq/job_util.rb"]
        );
        assert_eq!(f.resolve("c", "src/t_list.c", "server.h"), ["src/server.h"]);
        assert_eq!(
            f.resolve("cpp", "db/db_impl.cc", "db/db_impl.h"),
            ["db/db_impl.h"]
        );
        assert!(f.resolve("cpp", "db/db_impl.cc", "vector").is_empty());
    }

    #[test]
    fn rust_paths_resolve_through_relative_module_files() {
        let f = files(&["src/a/mod.rs", "src/a/b.rs", "src/a/b/c.rs", "src/d.rs"]);
        let m = |from, p| f.rust_module(from, p);
        assert_eq!(
            m("src/a/b.rs", "self::c::X"),
            Some(("src/a/b/c.rs".into(), vec!["X".into()]))
        );
        assert_eq!(
            m("src/a/b.rs", "super::Y"),
            Some(("src/a/mod.rs".into(), vec!["Y".into()]))
        );
        assert_eq!(
            m("src/a/mod.rs", "self::b::Z"),
            Some(("src/a/b.rs".into(), vec!["Z".into()]))
        );
    }

    #[test]
    fn normalizing_never_climbs_out() {
        assert_eq!(normalize("a/./b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize("../x"), None);
    }
}
