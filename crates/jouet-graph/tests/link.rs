//! Linking references across files, and keeping links right as files
//! change, through the public queries.

use jouet_graph::{Graph, SymbolQuery};
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Repo {
    dir: tempfile::TempDir,
    _cache: tempfile::TempDir,
    graph: Graph,
}

impl Repo {
    fn new(files: &[(&str, &str)]) -> Repo {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        for (rel, text) in files {
            write(dir.path(), rel, text);
        }
        let cache = tempfile::tempdir().unwrap();
        let mut graph = Graph::open_at(dir.path(), &cache.path().join("g.db")).unwrap();
        graph.refresh(None).unwrap();
        Repo {
            dir,
            _cache: cache,
            graph,
        }
    }

    fn edit(&mut self, rel: &str, text: &str) {
        write(self.dir.path(), rel, text);
        self.graph.refresh(None).unwrap();
    }

    fn id(&self, spec: &str) -> String {
        let found = self.graph.find(spec).unwrap();
        assert_eq!(found.len(), 1, "{spec}: {found:?}");
        found[0].id.clone()
    }

    fn callers(&self, spec: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .graph
            .callers(&self.id(spec), 1)
            .unwrap()
            .into_iter()
            .map(|r| format!("{}#{}", r.symbol.path, r.symbol.qualified_name))
            .collect();
        v.sort();
        v
    }
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    // Distinct mtimes, so a rewrite within one clock tick is still seen.
    let f = std::fs::File::options().write(true).open(&path).unwrap();
    let age = Duration::from_secs(1000 - text.len().min(999) as u64);
    f.set_modified(std::time::SystemTime::now() - age).unwrap();
}

#[test]
fn python_calls_resolve_through_imports_and_relink_when_the_target_moves() {
    let mut repo = Repo::new(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", "def helper():\n    return 1\n"),
        (
            "pkg/main.py",
            "from .util import helper\n\n\ndef run():\n    return helper()\n",
        ),
    ]);
    assert_eq!(repo.callers("pkg/util.py#helper"), ["pkg/main.py#run"]);
    // helper moves down a line: a new node, and run's call follows it.
    repo.edit("pkg/util.py", "# moved\ndef helper():\n    return 2\n");
    assert_eq!(repo.callers("pkg/util.py#helper"), ["pkg/main.py#run"]);
}

#[test]
fn an_unresolved_call_links_once_a_file_defines_it() {
    let mut repo = Repo::new(&[("a.go", "package a\n\nfunc Run() {\n\tLater()\n}\n")]);
    assert!(repo.graph.find("Later").unwrap().is_empty());
    repo.edit("b.go", "package a\n\nfunc Later() {}\n");
    assert_eq!(repo.callers("Later"), ["a.go#Run"]);
    std::fs::remove_file(repo.dir.path().join("b.go")).unwrap();
    repo.graph.refresh(None).unwrap();
    assert!(repo.graph.find("Later").unwrap().is_empty());
}

#[test]
fn typescript_imports_resolve_files_names_and_re_exports() {
    let repo = Repo::new(&[
        (
            "src/core/charge.ts",
            "export function charge(n: number) { return n; }\n",
        ),
        ("src/core/index.ts", "export { charge } from './charge';\n"),
        (
            "src/app.ts",
            "import { charge } from './core';\n\nexport function pay() {\n  return charge(1);\n}\n",
        ),
    ]);
    assert_eq!(repo.callers("charge"), ["src/app.ts#pay"]);
    let deps = repo.graph.impact(&repo.id("charge"), 3).unwrap();
    assert!(
        deps.iter().any(|r| r.symbol.path == "src/app.ts"),
        "{deps:?}"
    );
}

#[test]
fn rust_paths_resolve_through_modules_and_self_calls_to_their_type() {
    let repo = Repo::new(&[
        ("Cargo.toml", "[package]\nname = \"demo\"\n"),
        ("src/lib.rs", "pub mod store;\npub mod api;\n"),
        (
            "src/store.rs",
            "pub struct Store;\nimpl Store {\n    pub fn put(&self) {}\n    pub fn save(&self) { self.put(); }\n}\npub fn open() -> Store { Store }\n",
        ),
        (
            "src/api.rs",
            "use crate::store;\n\npub fn handle() {\n    let s = store::open();\n    s.save();\n}\n",
        ),
    ]);
    assert_eq!(repo.callers("store::open"), ["src/api.rs#handle"]);
    assert_eq!(repo.callers("Store::put"), ["src/store.rs#Store::save"]);
}

#[test]
fn java_extends_and_ruby_mixins_resolve() {
    let repo = Repo::new(&[
        (
            "src/main/java/com/x/Base.java",
            "package com.x;\npublic class Base { public void go() {} }\n",
        ),
        (
            "src/main/java/com/x/sub/Child.java",
            "package com.x.sub;\nimport com.x.Base;\npublic class Child extends Base {}\n",
        ),
        (
            "lib/util/helpers.rb",
            "module Helpers\n  def help; end\nend\n",
        ),
        (
            "lib/thing.rb",
            "require 'util/helpers'\nclass Thing\n  include Helpers\nend\n",
        ),
    ]);
    let base = repo.graph.impact(&repo.id("Base"), 1).unwrap();
    assert!(
        base.iter()
            .any(|r| r.symbol.name == "Child" && r.via == "extends"),
        "{base:?}"
    );
    let helpers = repo.graph.impact(&repo.id("Helpers"), 1).unwrap();
    assert!(
        helpers.iter().any(|r| r.symbol.name == "Thing"),
        "{helpers:?}"
    );
}

#[test]
fn c_includes_link_files_and_calls_reach_the_header() {
    let repo = Repo::new(&[
        ("src/util.h", "int add(int a, int b);\n"),
        (
            "src/util.c",
            "#include \"util.h\"\nint add(int a, int b) { return a + b; }\n",
        ),
        (
            "src/main.c",
            "#include \"util.h\"\nint main(void) { return add(1, 2); }\n",
        ),
    ]);
    let users = repo
        .graph
        .affected_tests(&[PathBuf::from("src/util.h")])
        .unwrap();
    assert!(users.unknown.is_empty());
    let callers: Vec<_> = repo
        .graph
        .find("add")
        .unwrap()
        .iter()
        .flat_map(|s| repo.graph.callers(&s.id, 1).unwrap())
        .map(|r| r.symbol.qualified_name)
        .collect();
    assert!(callers.contains(&"main".to_string()), "{callers:?}");
}

#[test]
fn common_methods_on_unknown_receivers_and_library_paths_stay_unlinked() {
    let repo = Repo::new(&[
        (
            "a.py",
            "class Cache:\n    def get(self, k):\n        return k\n",
        ),
        (
            "b.py",
            "import os\n\n\ndef f(d):\n    os.path.join('a')\n    return d.get(1)\n",
        ),
    ]);
    assert!(repo.callers("Cache::get").is_empty());
}

#[test]
fn symbols_rank_exact_names_then_words() {
    let repo = Repo::new(&[(
        "pay.ts",
        "/** Retries a failed charge. */\nexport function retryCharge() {}\nexport function charge() {}\n",
    )]);
    let names = |q: &str| -> Vec<String> {
        repo.graph
            .symbols(&SymbolQuery {
                text: Some(q.into()),
                ..SymbolQuery::default()
            })
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect()
    };
    assert_eq!(names("charge"), ["charge", "retryCharge"]);
    assert_eq!(names("retry"), ["retryCharge"]);
    assert_eq!(names("failed"), ["retryCharge"]);
    let explored = repo
        .graph
        .explore("how are failed charges retried")
        .unwrap();
    assert_eq!(explored[0].symbol.name, "retryCharge");
}

#[test]
fn affected_follows_dependents_to_tests_and_flags_unknown_files() {
    let repo = Repo::new(&[
        ("src/util.ts", "export function f() { return 1; }\n"),
        (
            "src/app.ts",
            "import { f } from './util';\nexport function g() { return f(); }\n",
        ),
        (
            "src/__tests__/app.test.ts",
            "import { g } from '../app';\ng();\n",
        ),
        ("src/__tests__/other.test.ts", "export const x = 1;\n"),
    ]);
    let a = repo
        .graph
        .affected_tests(&[PathBuf::from("src/util.ts"), PathBuf::from("README.md")])
        .unwrap();
    let tests: Vec<_> = a.tests.iter().map(|t| t.path.clone()).collect();
    assert_eq!(tests, [PathBuf::from("src/__tests__/app.test.ts")]);
    assert_eq!(
        a.tests[0].via,
        ["src/util.ts", "src/app.ts", "src/__tests__/app.test.ts"].map(PathBuf::from)
    );
    assert_eq!(a.unknown, [PathBuf::from("README.md")]);
}

#[test]
fn uses_of_a_name_say_which_definition_each_means_and_where_the_call_starts() {
    let repo = Repo::new(&[
        ("src/config.rs", "pub fn load() {}\n"),
        ("src/cache.rs", "pub fn load() {}\n"),
        (
            "src/lib.rs",
            "mod cache;\nmod config;\n\nfn start(r: &dyn Fn()) {\n    config::load();\n    let _ = cache::load();\n    r.load();\n}\n",
        ),
        ("app/a.py", "def load():\n    pass\n"),
        (
            "app/b.py",
            "from .a import load\n\n\ndef run(x):\n    load()\n    x.load()\n",
        ),
    ]);
    let config = repo.id("src/config.rs#load");
    let cache = repo.id("src/cache.rs#load");
    let python = repo.id("app/a.py#load");
    let sites: Vec<(String, u32, Option<u32>, Vec<String>)> = repo
        .graph
        .uses_of("load")
        .unwrap()
        .into_iter()
        .map(|s| (s.path, s.line, s.col, s.targets))
        .collect();
    assert_eq!(
        sites,
        [
            ("app/b.py".into(), 5, Some(4), vec![python]),
            ("app/b.py".into(), 6, Some(4), vec![]),
            ("src/lib.rs".into(), 5, Some(4), vec![config]),
            ("src/lib.rs".into(), 6, Some(12), vec![cache]),
            ("src/lib.rs".into(), 7, Some(4), vec![]),
        ]
    );
    let files = repo.graph.files().unwrap();
    assert!(files.contains(&"src/lib.rs".to_string()), "{files:?}");
}
