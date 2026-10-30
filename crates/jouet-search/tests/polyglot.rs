//! Symbols, search and the repository map, on a small repository with a
//! file in each supported language.
#![cfg(feature = "grammars")]

use jouet_search::map::{self, Options as MapOptions};
use jouet_search::memory::Memory;
use jouet_search::search::{self, Budget, Options, Report};
use jouet_search::symbols;
use std::path::{Path, PathBuf};

const FIXTURE: &str = "tests/fixtures/polyglot";

fn defs(path: &str) -> Vec<(String, String)> {
    let path = Path::new(FIXTURE).join(path);
    let source = std::fs::read(&path).unwrap();
    symbols::parse(&path, &source)
        .unwrap()
        .defs
        .into_iter()
        .map(|d| (d.kind, d.name))
        .collect()
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter()
        .map(|(k, n)| (k.to_string(), n.to_string()))
        .collect()
}

#[test]
fn every_language_yields_its_definitions() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "src/lib.rs",
            &[
                ("class", "Downloader"),
                ("method", "new"),
                ("method", "download"),
                ("function", "join_url"),
                ("function", "fetch_bytes"),
            ],
        ),
        (
            "app/client.py",
            &[
                ("class", "Client"),
                ("function", "__init__"),
                ("function", "fetch"),
                ("function", "_request"),
            ],
        ),
        (
            "web/api.ts",
            &[
                ("interface", "Api"),
                ("method", "request"),
                ("class", "Session"),
                ("method", "request"),
                ("function", "fetchText"),
            ],
        ),
        ("web/index.js", &[("function", "main")]),
        (
            "go/server.go",
            &[
                ("type", "Server"),
                ("function", "NewServer"),
                ("method", "Serve"),
                ("function", "listen"),
            ],
        ),
        (
            "ruby/app.rb",
            &[
                ("module", "Shop"),
                ("class", "Cart"),
                ("method", "initialize"),
                ("method", "add"),
                ("method", "total"),
            ],
        ),
        (
            "java/App.java",
            &[
                ("class", "App"),
                ("method", "main"),
                ("class", "Greeter"),
                ("method", "greet"),
            ],
        ),
        (
            "c/util.c",
            &[("class", "buffer"), ("function", "buffer_len")],
        ),
        (
            "cpp/thing.cpp",
            &[
                ("class", "Circle"),
                ("function", "Circle"),
                ("function", "area"),
                ("method", "area"),
            ],
        ),
    ];
    for (path, expected) in cases {
        assert_eq!(defs(path), pairs(expected), "{path}");
    }
}

#[test]
fn references_include_rust_types_and_path_calls() {
    let path = Path::new(FIXTURE).join("src/main.rs");
    let source = std::fs::read(&path).unwrap();
    let refs: Vec<String> = symbols::parse(&path, &source)
        .unwrap()
        .refs
        .into_iter()
        .map(|r| r.name)
        .collect();
    for name in ["Downloader", "new", "download"] {
        assert!(refs.contains(&name.to_string()), "{name} in {refs:?}");
    }
}

#[test]
fn a_definition_encloses_its_lines_and_the_innermost_wins() {
    let path = Path::new(FIXTURE).join("java/App.java");
    let symbols = symbols::parse(&path, &std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(symbols.enclosing(3).unwrap().name, "main");
    assert_eq!(symbols.enclosing(1).unwrap().name, "App");
    assert_eq!(symbols.enclosing(10).unwrap().name, "greet");
    assert!(symbols.enclosing(7).is_none());
}

fn options(pattern: &str) -> Options {
    Options {
        pattern: pattern.into(),
        paths: vec![PathBuf::from(FIXTURE)],
        budget: Budget {
            total: 0,
            per_file: 0,
        },
        ..Options::default()
    }
}

fn run(options: &Options) -> (Report, String) {
    let report = search::run(options, &mut Memory::none()).unwrap();
    let text = search::render(&report, options.files_only);
    (report, text)
}

#[test]
fn matches_are_grouped_by_file_and_enclosing_symbol() {
    let (report, text) = run(&options("fetch_bytes|join_url"));
    assert_eq!(report.files_matched, 1);
    assert_eq!(
        text,
        "tests/fixtures/polyglot/src/lib.rs
  method download (12-15)
    13: let url = join_url(&self.base, path);
    14: fetch_bytes(&url)
  function join_url (18-20)
    18: fn join_url(base: &str, path: &str) -> String {
  function fetch_bytes (22-24)
    22: fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
"
    );
}

#[test]
fn a_file_budget_says_how_many_matches_it_left_out() {
    let mut o = options("self");
    o.budget.per_file = 60;
    let (report, text) = run(&o);
    let lib = report
        .files
        .iter()
        .find(|f| f.path.ends_with("src/lib.rs"))
        .unwrap();
    assert!(lib.more > 0, "{text}");
    assert_eq!(lib.shown.len() + lib.more, lib.matches);
    assert!(
        text.contains(&format!("  ... {} more in {}\n", lib.more, lib.path)),
        "{text}"
    );
}

#[test]
fn a_total_budget_names_the_files_it_never_reached() {
    let mut o = options("(?i)request|fetch");
    o.budget.total = 120;
    let (report, text) = run(&o);
    assert!(!report.omitted.is_empty(), "{text}");
    let shown: usize = report
        .files
        .iter()
        .map(|f| f.shown.len() + f.more + f.already_shown)
        .sum();
    let omitted: usize = report.omitted.iter().map(|o| o.matches).sum();
    assert_eq!(
        shown + omitted,
        report.matches,
        "every match is accounted for"
    );
    let last = text.lines().last().unwrap();
    assert!(
        last.starts_with(&format!(
            "... {omitted} matches in {}",
            plural_files(report.omitted.len())
        )) && last.contains(&report.omitted[0].path)
            && last.ends_with("(narrow the search, or raise --budget)"),
        "{last}"
    );
}

fn plural_files(n: usize) -> String {
    format!("{n} more {}", if n == 1 { "file:" } else { "files:" })
}

#[test]
fn files_with_matches_lists_counts() {
    let mut o = options("Session");
    o.files_only = true;
    let (_, text) = run(&o);
    assert_eq!(
        text,
        "tests/fixtures/polyglot/web/api.ts (1)\ntests/fixtures/polyglot/web/index.js (2)\n"
    );
}

#[test]
fn no_match_says_how_much_was_searched() {
    let (report, text) = run(&options("no such text anywhere"));
    assert_eq!(report.matches, 0);
    assert_eq!(
        text,
        format!("no matches ({} files searched)\n", report.files_searched)
    );
}

#[test]
fn a_bad_pattern_or_missing_path_is_an_error() {
    assert!(matches!(
        search::run(&options("("), &mut Memory::none()),
        Err(search::Error::Usage(_))
    ));
    let mut o = options("x");
    o.paths = vec![PathBuf::from("tests/fixtures/missing")];
    assert!(matches!(
        search::run(&o, &mut Memory::none()),
        Err(search::Error::NotFound(_))
    ));
}

#[test]
fn matches_already_shown_collapse_until_the_file_changes() {
    let repo = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let file = repo.path().join("a.py");
    std::fs::write(
        &file,
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n",
    )
    .unwrap();
    let mut o = options("beta");
    o.paths = vec![repo.path().to_path_buf()];

    let mut memory = Memory::open(state.path(), "session");
    let first = search::run(&o, &mut memory).unwrap();
    memory.save();
    assert_eq!(first.files[0].shown.len(), 2);

    let mut memory = Memory::open(state.path(), "session");
    let second = search::run(&o, &mut memory).unwrap();
    assert_eq!(second.files[0].already_shown, 2);
    assert!(second.files[0].shown.is_empty());
    let text = search::render(&second, false);
    assert!(
        text.ends_with("a.py: 2 matches already shown, unchanged\n"),
        "{text}"
    );

    // Another session has seen nothing.
    let other = search::run(&o, &mut Memory::open(state.path(), "other")).unwrap();
    assert_eq!(other.files[0].shown.len(), 2);

    // A changed file is new again; a new match beside old ones shows alone.
    std::fs::write(
        &file,
        "def alpha():\n    return beta()\n\n\ndef beta():\n    return 1\n# beta\n",
    )
    .unwrap();
    let mut memory = Memory::open(state.path(), "session");
    let third = search::run(&o, &mut memory).unwrap();
    memory.save();
    assert_eq!(third.files[0].shown.len(), 3);
    let mut memory = Memory::open(state.path(), "session");
    let fourth = search::run(&o, &mut memory).unwrap();
    assert_eq!(fourth.files[0].already_shown, 3);
}

fn map_options(budget: usize) -> MapOptions {
    MapOptions {
        root: PathBuf::from(FIXTURE),
        budget,
        ..MapOptions::default()
    }
}

#[test]
fn the_map_fits_the_budget_and_says_what_it_left_out() {
    let full = map::build(&map_options(10_000));
    assert_eq!(full.more_defs, 0);
    assert_eq!(full.files_scanned, 11);
    let small = map::build(&map_options(60));
    assert!(small.tokens <= 60, "{}", small.tokens);
    assert!(small.more_defs > 0);
    let text = map::render(&small);
    assert!(
        text.ends_with(&format!(
            "... {} more definitions in {} more files (raise --budget, or --focus a file or name)\n",
            small.more_defs, small.more_files
        )),
        "{text}"
    );
}

#[test]
fn a_focused_file_ranks_what_it_uses_and_is_left_out() {
    let mut options = map_options(25);
    options.focus_files = vec!["src/main.rs".into()];
    let map = map::build(&options);
    let text = map::render(&map);
    assert!(
        text.starts_with("src/lib.rs\n  3: pub struct Downloader {\n"),
        "{text}"
    );
    assert!(map.files.iter().all(|f| f.path != "src/main.rs"));
}

#[test]
fn a_focused_identifier_ranks_its_definitions_first() {
    let mut options = map_options(30);
    options.focus_idents = vec!["Session".into()];
    let map = map::build(&options);
    let text = map::render(&map);
    assert!(text.starts_with("web/api.ts\n"), "{text}");
    assert!(text.contains("export class Session {"), "{text}");
}

#[test]
fn the_map_snapshot() {
    let map = map::build(&map_options(10_000));
    assert_eq!(map::render(&map), include_str!("fixtures/polyglot.map.txt"));
}

#[test]
fn tags_are_cached_by_modification_time() {
    let repo = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    for (name, body) in [("a.rs", "fn a() { b(); }\n"), ("b.rs", "fn b() {}\n")] {
        std::fs::write(repo.path().join(name), body).unwrap();
    }
    let options = MapOptions {
        root: repo.path().to_path_buf(),
        budget: 1000,
        cache: Some(cache.path().to_path_buf()),
        ..MapOptions::default()
    };
    let cold = map::build(&options);
    assert_eq!((cold.files_scanned, cold.cached), (2, 0));
    let warm = map::build(&options);
    assert_eq!((warm.files_scanned, warm.cached), (2, 2));
    // A changed file is read again, and its new definitions appear.
    std::fs::write(repo.path().join("b.rs"), "fn b() {}\nfn c() {}\n").unwrap();
    let changed = map::build(&options);
    assert_eq!(changed.cached, 1);
    assert!(map::render(&changed).contains("fn c() {}"));
}

#[test]
fn definitions_alone_are_what_a_full_parse_finds() {
    for entry in ignore::Walk::new(FIXTURE).flatten() {
        let path = entry.path();
        if !symbols::supported(path) {
            continue;
        }
        let source = std::fs::read(path).unwrap();
        assert_eq!(
            symbols::definitions(path, &source).unwrap(),
            symbols::parse(path, &source).unwrap().defs,
            "{}",
            path.display()
        );
    }
}

#[test]
fn cached_definitions_give_the_same_groups_and_follow_changes() {
    let cache = tempfile::tempdir().unwrap();
    let mut o = options("fetch_bytes|join_url");
    let (_, plain) = run(&o);
    o.cache = Some(cache.path().to_path_buf());
    let (_, cold) = run(&o);
    let (_, warm) = run(&o);
    assert_eq!(cold, plain);
    assert_eq!(warm, plain);
    assert!(std::fs::read_dir(cache.path()).unwrap().count() >= 1);

    let repo = tempfile::tempdir().unwrap();
    let file = repo.path().join("a.rs");
    std::fs::write(&file, "fn one() {\n    target();\n}\n").unwrap();
    let mut o = options("target");
    o.paths = vec![repo.path().to_path_buf()];
    o.cache = Some(cache.path().to_path_buf());
    assert_eq!(
        run(&o).0.files[0].shown[0].symbol.as_ref().unwrap().name,
        "one"
    );
    std::fs::write(&file, "fn second() {\n    target();\n}\n").unwrap();
    assert_eq!(
        run(&o).0.files[0].shown[0].symbol.as_ref().unwrap().name,
        "second"
    );
}
