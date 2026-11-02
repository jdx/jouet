//! Definitions and search on a small repository with a file in each
//! supported language.
#![cfg(feature = "grammars")]

use jouet_search::memory::Memory;
use jouet_search::search::{self, Budget, Options, Report};
use jouet_search::symbols;
use std::path::{Path, PathBuf};

const FIXTURE: &str = "tests/fixtures/polyglot";

fn defs(path: &str) -> Vec<(String, String)> {
    let path = Path::new(FIXTURE).join(path);
    let source = std::fs::read(&path).unwrap();
    symbols::definitions(&path, &source)
        .unwrap()
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
fn a_definition_encloses_its_lines_and_the_innermost_wins() {
    let path = Path::new(FIXTURE).join("java/App.java");
    let defs = symbols::definitions(&path, &std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(symbols::enclosing(&defs, 3).unwrap().name, "main");
    assert_eq!(symbols::enclosing(&defs, 1).unwrap().name, "App");
    assert_eq!(symbols::enclosing(&defs, 10).unwrap().name, "greet");
    assert!(symbols::enclosing(&defs, 7).is_none());
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
fn max_count_shows_at_most_that_many_matches_from_each_file() {
    let mut o = options("self");
    o.max_count = Some(1);
    let (report, text) = run(&o);
    assert!(report.files.iter().all(|f| f.shown.len() <= 1), "{text}");
    let lib = report
        .files
        .iter()
        .find(|f| f.path.ends_with("src/lib.rs"))
        .unwrap();
    assert_eq!(lib.shown.len() + lib.more, lib.matches, "{text}");
    assert!(lib.more > 0, "{text}");
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

fn outline(path: &str) -> Vec<String> {
    let path = Path::new(FIXTURE).join(path);
    let source = std::fs::read(&path).unwrap();
    symbols::outline(&path, &source)
        .unwrap()
        .into_iter()
        .map(|l| format!("{} {}", l.def.kind, l.qualified()))
        .collect()
}

#[test]
fn an_outline_names_each_definition_with_its_scope() {
    assert_eq!(
        outline("src/lib.rs"),
        [
            "class Downloader",
            "method Downloader::new",
            "method Downloader::download",
            "function join_url",
            "function fetch_bytes"
        ]
    );
    assert_eq!(
        outline("app/client.py"),
        [
            "class Client",
            "function Client.__init__",
            "function Client.fetch",
            "function _request"
        ]
    );
    assert_eq!(
        outline("go/server.go"),
        [
            "type Server",
            "function NewServer",
            "method Server.Serve",
            "function listen"
        ]
    );
    assert_eq!(
        outline("ruby/app.rb"),
        [
            "module Shop",
            "class Shop.Cart",
            "method Shop.Cart.initialize",
            "method Shop.Cart.add",
            "method Shop.Cart.total"
        ]
    );
    assert_eq!(
        outline("java/App.java"),
        [
            "class App",
            "method App.main",
            "class Greeter",
            "method Greeter.greet"
        ]
    );
}

#[test]
fn a_selector_picks_definitions_by_name_kind_and_scope() {
    let path = Path::new(FIXTURE).join("web/api.ts");
    let source = std::fs::read(&path).unwrap();
    let names = |selector: &str| {
        symbols::select(&path, &source, &symbols::Selector::parse(selector).unwrap())
            .map(|found| found.iter().map(|l| l.def.line).collect::<Vec<_>>())
    };
    assert_eq!(names("request"), Ok(vec![2, 8]));
    assert_eq!(names("Session.request"), Ok(vec![8]));
    assert_eq!(names("fn fetchText"), Ok(vec![13]));
    assert_eq!(names("class Session"), Ok(vec![5]));
    assert_eq!(names("class request"), Err(symbols::SelectError::NotFound));
    assert_eq!(
        names("Missing.request"),
        Err(symbols::SelectError::NotFound)
    );
    assert!(matches!(
        symbols::Selector::parse("widget request"),
        Err(symbols::SelectError::Invalid(_))
    ));
    let rust = Path::new(FIXTURE).join("src/lib.rs");
    let source = std::fs::read(&rust).unwrap();
    let found = symbols::select(
        &rust,
        &source,
        &symbols::Selector::parse("Downloader::download").unwrap(),
    )
    .unwrap();
    let text = std::str::from_utf8(&source[found[0].start_byte..found[0].end_byte]).unwrap();
    assert!(
        text.starts_with("pub fn download(") && text.ends_with('}'),
        "{text}"
    );
}

#[test]
fn syntax_errors_are_counted() {
    let path = Path::new("x.rs");
    assert_eq!(symbols::syntax_errors(path, b"fn a() {}\n"), Some(0));
    assert!(symbols::syntax_errors(path, b"fn a() {\n").unwrap() > 0);
    assert_eq!(
        symbols::syntax_errors(Path::new("x.txt"), b"anything"),
        None
    );
}

fn structural(
    pattern: &str,
    lang: &str,
    file: &str,
    rewrite: Option<&str>,
) -> Vec<(u32, String, Option<String>)> {
    let st = jouet_search::structural::Structural::new(pattern, lang, rewrite).unwrap();
    let path = Path::new(FIXTURE).join(file);
    assert!(st.applies_to(&path), "{file}");
    st.find(&std::fs::read_to_string(&path).unwrap())
        .into_iter()
        .map(|m| (m.line, m.text, m.replacement))
        .collect()
}

#[test]
fn structural_patterns_match_code_in_each_language() {
    assert_eq!(
        structural(
            "fetch_bytes($U)",
            "rust",
            "src/lib.rs",
            Some("fetch_bytes($U, 3)")
        ),
        [(
            14,
            "fetch_bytes(&url)".to_string(),
            Some("fetch_bytes(&url, 3)".to_string())
        )]
    );
    assert_eq!(
        structural("_request($$$ARGS)", "python", "app/client.py", None),
        [(6, "_request(self.base + path)".to_string(), None)]
    );
    assert_eq!(
        structural("new $C($$$)", "javascript", "web/index.js", None)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        [4]
    );
    assert_eq!(
        structural("return listen($A)", "go", "go/server.go", None)
            .iter()
            .map(|m| m.0)
            .collect::<Vec<_>>(),
        [12]
    );
}

#[test]
fn a_pattern_must_be_whole_code_in_a_known_language() {
    use jouet_search::structural::{Error, Structural};
    assert!(matches!(
        Structural::new("f($A", "rust", None),
        Err(Error::Pattern(_))
    ));
    assert!(matches!(
        Structural::new("f($A)", "cobol", None),
        Err(Error::UnknownLanguage(_))
    ));
    let st = Structural::new("f($A)", "ts", None).unwrap();
    assert!(st.applies_to(Path::new("a.ts")));
    assert!(!st.applies_to(Path::new("a.tsx")) && !st.applies_to(Path::new("a.rs")));
}

#[test]
fn a_structural_search_groups_and_budgets_like_a_text_one() {
    let mut o = options("$X.base");
    o.structural = Some("rust".into());
    let (report, text) = run(&o);
    assert_eq!(report.files_searched, 2, "only Rust files are read");
    assert!(
        text.starts_with("tests/fixtures/polyglot/src/lib.rs\n  method download (12-15)\n"),
        "{text}"
    );
}
