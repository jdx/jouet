//! The repository map: definitions ranked by how the rest of the code uses
//! them, cut to a token budget.

use jouet_graph::{Graph, MapOptions, render_map};
fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Graph) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    for (rel, text) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let cache = tempfile::tempdir().unwrap();
    let mut graph = Graph::open_at(dir.path(), &cache.path().join("g.db")).unwrap();
    graph.refresh(None).unwrap();
    (dir, cache, graph)
}

/// A store every handler uses, a cache with a same-named `load` that only
/// one file calls, and a helper nothing calls.
const FILES: &[(&str, &str)] = &[
    (
        "src/store.rs",
        "pub struct Store {}\n\npub fn load(path: &str) -> Store {\n    Store {}\n}\n",
    ),
    (
        "src/cache.rs",
        "pub fn load(key: &str) {}\n\npub fn unused_helper() {}\n",
    ),
    (
        "src/a.rs",
        "use crate::store::{self, Store};\n\npub fn a() -> Store {\n    store::load(\"a\")\n}\n",
    ),
    (
        "src/b.rs",
        "use crate::store::{self, Store};\n\npub fn b() -> Store {\n    store::load(\"b\")\n}\n",
    ),
    (
        "src/c.rs",
        "use crate::store::{self, Store};\nuse crate::cache;\n\npub fn c() -> Store {\n    cache::load(\"c\");\n    store::load(\"c\")\n}\n",
    ),
    (
        "src/lib.rs",
        "mod a;\nmod b;\nmod c;\nmod cache;\nmod store;\n",
    ),
];

fn listed(map: &jouet_graph::RepoMap) -> Vec<String> {
    map.files
        .iter()
        .flat_map(|f| f.defs.iter().map(move |d| format!("{}#{}", f.path, d.name)))
        .collect()
}

#[test]
fn the_definitions_others_use_come_first_and_same_names_are_told_apart() {
    let (_dir, _cache, graph) = repo(FILES);
    let map = graph
        .map(&MapOptions {
            budget: 22,
            ..MapOptions::default()
        })
        .unwrap();
    // The store is what everything uses; the cache's `load` shares a name
    // with it, but only one call is the cache's.
    assert_eq!(
        listed(&map),
        ["src/store.rs#Store", "src/store.rs#load"],
        "{}",
        render_map(&map)
    );
    assert!(map.more_defs > 0 && map.more_files > 0, "{map:?}");
    assert!(
        render_map(&map).ends_with("(raise --budget, or --focus a file or name)\n"),
        "{}",
        render_map(&map)
    );
    assert_eq!(map.files_ranked, 6);
}

#[test]
fn everything_fits_a_large_budget_and_a_focused_file_is_left_out() {
    let (_dir, _cache, graph) = repo(FILES);
    let map = graph
        .map(&MapOptions {
            budget: 10_000,
            focus_files: vec!["src/store.rs".into()],
            ..MapOptions::default()
        })
        .unwrap();
    let all = listed(&map);
    assert!(
        !all.iter().any(|d| d.starts_with("src/store.rs")),
        "{all:?}"
    );
    assert!(
        all.contains(&"src/cache.rs#unused_helper".to_string()),
        "{all:?}"
    );
    assert_eq!((map.more_defs, map.more_files), (0, 0));
    let line = &map
        .files
        .iter()
        .find(|f| f.path == "src/a.rs")
        .unwrap()
        .defs[0];
    assert_eq!(
        (line.line, line.text.as_str()),
        (3, "pub fn a() -> Store {")
    );
}

#[test]
fn a_focused_name_comes_first_and_within_keeps_to_a_directory() {
    let (_dir, _cache, graph) = repo(FILES);
    let map = graph
        .map(&MapOptions {
            budget: 12,
            focus_idents: vec!["unused_helper".into()],
            ..MapOptions::default()
        })
        .unwrap();
    assert_eq!(listed(&map)[0], "src/cache.rs#unused_helper");
    let src = graph
        .map(&MapOptions {
            budget: 10_000,
            within: Some("src".into()),
            ..MapOptions::default()
        })
        .unwrap();
    assert!(!src.files.is_empty(), "{src:?}");
    // A prefix of a directory's name is not the directory.
    let sibling = graph
        .map(&MapOptions {
            budget: 10_000,
            within: Some("sr".into()),
            ..MapOptions::default()
        })
        .unwrap();
    assert!(sibling.files.is_empty(), "{sibling:?}");
}
