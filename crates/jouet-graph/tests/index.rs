//! Indexing a work tree, and keeping the graph in step as files change.

use jouet_graph::Graph;
use std::path::Path;
use std::time::Duration;

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    dir
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn names(graph: &Graph, rel: &str) -> Vec<String> {
    graph
        .outline(Path::new(rel))
        .unwrap()
        .into_iter()
        .map(|s| s.qualified_name)
        .collect()
}

fn open(root: &Path, cache: &Path) -> Graph {
    Graph::open_at(root, &cache.join("graph.db")).unwrap()
}

/// Bumps a file's mtime, so a rewrite within the filesystem's timestamp
/// resolution is still seen.
fn age(root: &Path, rel: &str) {
    let path = root.join(rel);
    let f = std::fs::File::options().write(true).open(path).unwrap();
    let t = f.metadata().unwrap().modified().unwrap() - Duration::from_secs(10);
    f.set_modified(t).unwrap();
}

#[test]
fn indexes_then_reindexes_only_what_changed() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/lib.rs", "pub fn alpha() {}\npub struct Beta;\n");
    write(root, "app/main.py", "def gamma():\n    pass\n");
    write(root, "README.md", "# not code\n");
    let mut graph = open(root, cache.path());

    let first = graph.refresh(None).unwrap();
    assert_eq!((first.checked, first.indexed), (2, 2));
    assert_eq!(names(&graph, "src/lib.rs"), ["alpha", "Beta"]);
    assert_eq!(names(&graph, "app/main.py"), ["gamma"]);

    let again = graph.refresh(None).unwrap();
    assert_eq!((again.checked, again.indexed, again.touched), (2, 0, 0));

    age(root, "app/main.py");
    let touched = graph.refresh(None).unwrap();
    assert_eq!((touched.indexed, touched.touched), (0, 1));

    write(root, "src/lib.rs", "pub fn alpha() {}\npub fn delta() {}\n");
    age(root, "src/lib.rs");
    std::fs::remove_file(root.join("app/main.py")).unwrap();
    let changed = graph.refresh(None).unwrap();
    assert_eq!((changed.indexed, changed.removed), (1, 1));
    assert_eq!(names(&graph, "src/lib.rs"), ["alpha", "delta"]);
    assert!(names(&graph, "app/main.py").is_empty());
    assert_eq!(graph.status().unwrap().files, 1);
}

#[test]
fn honours_gitignore_and_skips_vendored_directories() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, ".gitignore", "generated/\n");
    write(root, "src/a.ts", "export function a() {}\n");
    write(root, "generated/b.ts", "export function b() {}\n");
    write(root, "node_modules/c/index.js", "function c() {}\n");
    let mut graph = open(root, cache.path());
    let report = graph.refresh(None).unwrap();
    assert_eq!(report.checked, 1);
    assert_eq!(names(&graph, "src/a.ts"), ["a"]);
}

#[test]
fn reindexes_named_paths_without_a_walk() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a.go", "package a\n\nfunc One() {}\n");
    write(root, "b.go", "package a\n\nfunc Two() {}\n");
    let mut graph = open(root, cache.path());
    graph.refresh(None).unwrap();

    write(root, "a.go", "package a\n\nfunc Uno() {}\n");
    age(root, "a.go");
    write(root, "b.go", "package a\n\nfunc Dos() {}\n");
    age(root, "b.go");
    let report = graph
        .reindex_paths(
            &[
                root.join("a.go"),
                "notes.txt".into(),
                "/elsewhere/x.go".into(),
            ],
            None,
        )
        .unwrap();
    assert_eq!((report.checked, report.indexed), (1, 1));
    assert_eq!(names(&graph, "a.go"), ["Uno"]);
    // b.go was not named, so it is stale until the next refresh.
    assert_eq!(names(&graph, "b.go"), ["Two"]);

    std::fs::remove_file(root.join("a.go")).unwrap();
    let report = graph.reindex_paths(&["a.go".into()], None).unwrap();
    assert_eq!(report.removed, 1);
    graph.refresh(None).unwrap();
    assert_eq!(names(&graph, "b.go"), ["Dos"]);
}

#[test]
fn a_spent_budget_defers_the_rest() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..20 {
        write(
            root,
            &format!("m{i}.py"),
            &format!("def f{i}():\n    pass\n"),
        );
    }
    let mut graph = open(root, cache.path());
    let report = graph.refresh(Some(Duration::ZERO)).unwrap();
    assert_eq!(report.indexed, 0);
    assert_eq!(report.deferred, 20);
    let report = graph.refresh(None).unwrap();
    assert_eq!(report.indexed, 20);
}

#[test]
fn records_files_it_cannot_extract_without_retrying_them() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("big.js"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    std::fs::write(root.join("latin1.c"), b"int caf\xe9;\n").unwrap();
    let mut graph = open(root, cache.path());
    let report = graph.refresh(None).unwrap();
    assert_eq!((report.indexed, report.skipped), (2, 2));
    assert_eq!(graph.status().unwrap().files_without_symbols, 2);
    assert_eq!(graph.refresh(None).unwrap().indexed, 0);
}

#[test]
fn a_deeply_nested_file_fails_alone() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    let depth = 200_000;
    let deep = format!("int f() {{ {} }}\n", "{".repeat(depth) + &"}".repeat(depth));
    write(root, "deep.c", &deep);
    write(root, "ok.c", "int g(void) { return 1; }\n");
    let mut graph = open(root, cache.path());
    let report = graph.refresh(None).unwrap();
    assert_eq!(report.indexed, 2);
    assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
    assert_eq!(report.failed[0].0, "deep.c");
    assert_eq!(names(&graph, "ok.c"), ["g"]);
}

#[test]
fn a_database_for_another_schema_or_work_tree_is_rebuilt() {
    let a = repo();
    let b = repo();
    let cache = tempfile::tempdir().unwrap();
    write(a.path(), "a.rb", "def a; end\n");
    write(b.path(), "b.rb", "def b; end\n");
    let mut graph = open(a.path(), cache.path());
    graph.refresh(None).unwrap();
    drop(graph);
    let mut graph = open(b.path(), cache.path());
    assert_eq!(graph.status().unwrap().files, 0);
    graph.refresh(None).unwrap();
    assert_eq!(names(&graph, "b.rb"), ["b"]);
}

#[test]
fn a_large_first_index_fills_in_bulk_and_stays_incremental() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let root = dir.path();
    for i in 0..250 {
        write(
            root,
            &format!("pkg/m{i}.rs"),
            &format!("pub fn f{i}() {{ f{}(); }}\n", (i + 1) % 250),
        );
    }
    let mut graph = open(root, cache.path());
    assert_eq!(graph.refresh(None).unwrap().indexed, 250);
    assert_eq!(graph.status().unwrap().nodes, 500);
    write(root, "pkg/m7.rs", "pub fn seven() {}\n");
    age(root, "pkg/m7.rs");
    assert_eq!(graph.refresh(None).unwrap().indexed, 1);
    assert_eq!(names(&graph, "pkg/m7.rs"), ["seven"]);
    assert_eq!(graph.status().unwrap().nodes, 500);
}

#[test]
fn a_corrupt_database_is_rebuilt() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    write(dir.path(), "a.java", "class A { void m() {} }\n");
    std::fs::write(
        cache.path().join("graph.db"),
        b"this is not sqlite, but it is long enough to have a header....",
    )
    .unwrap();
    let mut graph = open(dir.path(), cache.path());
    graph.refresh(None).unwrap();
    assert_eq!(names(&graph, "a.java"), ["A", "A::m"]);
}

#[cfg(unix)]
#[test]
fn paths_through_a_symlink_to_the_work_tree_are_the_same_files() {
    let dir = repo();
    let cache = tempfile::tempdir().unwrap();
    let link_dir = tempfile::tempdir().unwrap();
    let link = link_dir.path().join("via");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    write(dir.path(), "a.rb", "def one; end\n");
    let mut graph = open(&link, cache.path());
    graph.refresh(None).unwrap();
    write(dir.path(), "a.rb", "def two; end\n");
    age(dir.path(), "a.rb");
    let report = graph.reindex_paths(&[link.join("a.rb")], None).unwrap();
    assert_eq!(report.indexed, 1);
    assert_eq!(names(&graph, &link.join("a.rb").to_string_lossy()), ["two"]);
    std::fs::remove_file(dir.path().join("a.rb")).unwrap();
    assert_eq!(
        graph
            .reindex_paths(&[link.join("a.rb")], None)
            .unwrap()
            .removed,
        1
    );
}
