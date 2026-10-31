use super::spec::{Format, parse};
use super::*;
use std::fs;
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
        }
        Self { dir }
    }

    fn path(&self, name: &str) -> String {
        self.dir.path().join(name).to_str().unwrap().to_string()
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.dir.path().join(name)).unwrap()
    }

    fn exists(&self, name: &str) -> bool {
        self.dir.path().join(name).exists()
    }

    /// Writes `{name}` placeholders as absolute paths into the fixture.
    fn spec(&self, text: &str, format: Format) -> Spec {
        let mut text = text.to_string();
        for entry in ["a.rs", "b.rs", "c.rs", "new.txt", "moved.rs", "bin"] {
            text = text.replace(&format!("{{{entry}}}"), &self.path(entry));
        }
        parse(&text, format).unwrap()
    }

    fn run(&self, text: &str, format: Format) -> (Report, Option<Journal>) {
        self.run_with(text, format, &Options::default())
    }

    fn run_with(&self, text: &str, format: Format, options: &Options) -> (Report, Option<Journal>) {
        let mut plan = plan(&self.spec(text, format), options).unwrap();
        if plan.report.ok() {
            let journal = write(&mut plan).unwrap();
            (plan.report, Some(journal))
        } else {
            (plan.report, None)
        }
    }
}

fn block(path: &str, old: &str, new: &str) -> String {
    format!("=== {{{path}}}\n<<<<<<< SEARCH\n{old}=======\n{new}>>>>>>> REPLACE\n")
}

#[test]
fn a_unique_match_is_replaced_and_counted() {
    let f = Fixture::new(&[("a.rs", "fn a() {\n    one();\n}\n")]);
    let (report, _) = f.run(
        &block("a.rs", "    one();\n", "    one();\n    two();\n"),
        Format::Blocks,
    );
    assert!(report.applied);
    assert_eq!(report.files_written, 1);
    assert_eq!(report.files[0].status, FileStatus::Written);
    assert_eq!((report.files[0].added, report.files[0].removed), (1, 0));
    assert_eq!(report.files[0].edits[0].lines, vec![2]);
    assert_eq!(f.read("a.rs"), "fn a() {\n    one();\n    two();\n}\n");
}

#[test]
fn one_failing_file_means_no_file_is_written() {
    let f = Fixture::new(&[("a.rs", "alpha\n"), ("b.rs", "beta\n")]);
    let spec = block("a.rs", "alpha\n", "ALPHA\n") + &block("b.rs", "gamma\n", "GAMMA\n");
    let (report, journal) = f.run(&spec, Format::Blocks);
    assert!(journal.is_none());
    assert!(!report.applied);
    assert_eq!(report.files_written, 0);
    assert_eq!(report.files[0].status, FileStatus::NotWritten);
    assert_eq!(report.files[1].status, FileStatus::Failed);
    assert_eq!(report.files[1].edits[0].status, EditStatus::NoMatch);
    assert_eq!(f.read("a.rs"), "alpha\n");
}

#[test]
fn a_reformatted_line_is_shown_as_the_closest_match() {
    let f = Fixture::new(&[(
        "a.rs",
        "fn main() {\n    let resp = client.get(&url).send()?;\n    ok()\n}\n",
    )]);
    let (report, _) = f.run(
        &block("a.rs", "    let resp = client.get(url).send()?;\n", "x\n"),
        Format::Blocks,
    );
    let closest = report.files[0].edits[0]
        .closest
        .clone()
        .expect("a closest match");
    assert_eq!(closest.line, 2);
    assert!(closest.similarity > 0.9, "{closest:?}");
    assert_eq!(
        closest.diff,
        "-    let resp = client.get(url).send()?;\n+    let resp = client.get(&url).send()?;\n"
    );
}

#[test]
fn a_search_holding_part_of_a_line_says_which_line_holds_it() {
    let text = "intro\n- **Docs.** `docs/benchmarks.md` now recommends it.\nend\n";
    let f = Fixture::new(&[("a.rs", text)]);
    let (report, _) = f.run(
        &block(
            "a.rs",
            "- **Docs.** `docs/benchmarks.md` now recommends\n",
            "x\n",
        ),
        Format::Blocks,
    );
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::NoMatch);
    assert_eq!(
        edit.near_miss,
        Some(NearMiss::InsideLine {
            lines: vec![2],
            multiline: false
        })
    );
    // Text that ends inside a later line spans lines.
    let (report, _) = f.run(
        &block("a.rs", "intro\n- **Docs.**\n", "x\n"),
        Format::Blocks,
    );
    assert_eq!(
        report.files[0].edits[0].near_miss,
        Some(NearMiss::InsideLine {
            lines: vec![1],
            multiline: true
        })
    );
    // Every line the text is inside, once each.
    let f = Fixture::new(&[("a.rs", "a.b a.b\nx\nfoo(a.b)\n")]);
    let (report, _) = f.run(&block("a.rs", "a.b\n", "x\n"), Format::Blocks);
    assert_eq!(
        report.files[0].edits[0].near_miss,
        Some(NearMiss::InsideLine {
            lines: vec![1, 3],
            multiline: false
        })
    );
    // Text found nowhere has no near miss, and `jouet edit replace`'s
    // edits match inside lines anyway.
    let (report, _) = f.run(&block("a.rs", "nowhere\n", "x\n"), Format::Blocks);
    assert_eq!(report.files[0].edits[0].near_miss, None);
    let json = r#"[{"path": "{a.rs}", "old": "nowhere", "new": "x"}]"#;
    let (report, _) = f.run(json, Format::Json);
    assert_eq!(report.files[0].edits[0].near_miss, None);
}

#[test]
fn a_search_with_other_indentation_is_a_whitespace_near_miss() {
    let f = Fixture::new(&[("a.rs", "fn a() {\n\tone();\n\ttwo();  \n}\n")]);
    let (report, _) = f.run(
        &block("a.rs", "    one();\n    two();\n", "x\n"),
        Format::Blocks,
    );
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::NoMatch);
    assert_eq!(
        edit.near_miss,
        Some(NearMiss::Whitespace { lines: vec![2] })
    );
    // Missing indentation is a whitespace difference, not text inside a line.
    let (report, _) = f.run(&block("a.rs", "one();\n", "x\n"), Format::Blocks);
    assert_eq!(
        report.files[0].edits[0].near_miss,
        Some(NearMiss::Whitespace { lines: vec![2] })
    );
    // Patch hunks get the same hint.
    let spec = "*** Begin Patch\n*** Update File: {a.rs}\n-  one();\n+  uno();\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert_eq!(
        report.files[0].edits[0].near_miss,
        Some(NearMiss::Whitespace { lines: vec![2] })
    );
}

#[test]
fn too_many_matches_lists_every_line() {
    let f = Fixture::new(&[("a.rs", "x\ny\nx\nx\n")]);
    let (report, _) = f.run(&block("a.rs", "x\n", "z\n"), Format::Blocks);
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::TooManyMatches);
    assert_eq!(edit.found, 3);
    assert_eq!(edit.lines, vec![1, 3, 4]);
    assert_eq!(f.read("a.rs"), "x\ny\nx\nx\n");
}

#[test]
fn counts_can_be_exact_or_all() {
    let f = Fixture::new(&[("a.rs", "x\ny\nx\n"), ("b.rs", "q\nq\nq\n")]);
    let spec = "=== {a.rs}\n<<<<<<< SEARCH count=2\nx\n=======\nz\n>>>>>>> REPLACE\n=== {b.rs}\n<<<<<<< SEARCH count=all\nq\n=======\n>>>>>>> REPLACE\n";
    let (report, _) = f.run(spec, Format::Blocks);
    assert!(report.applied, "{report:?}");
    assert_eq!(f.read("a.rs"), "z\ny\nz\n");
    assert_eq!(f.read("b.rs"), "");
}

#[test]
fn too_few_matches_is_its_own_status() {
    let f = Fixture::new(&[("a.rs", "x\ny\n")]);
    let spec = "=== {a.rs}\n<<<<<<< SEARCH count=2\nx\n=======\nz\n>>>>>>> REPLACE\n";
    let (report, _) = f.run(spec, Format::Blocks);
    assert_eq!(report.files[0].edits[0].status, EditStatus::TooFewMatches);
}

#[test]
fn blocks_only_match_whole_lines_but_json_matches_anywhere() {
    let f = Fixture::new(&[("a.rs", "barfoo\nfoo\n"), ("b.rs", "barfoo\n")]);
    let (report, _) = f.run(&block("a.rs", "foo\n", "baz\n"), Format::Blocks);
    assert!(report.applied);
    assert_eq!(f.read("a.rs"), "barfoo\nbaz\n");
    // Built with serde_json so a Windows path's backslashes are escaped.
    let json =
        serde_json::json!([{ "path": f.path("b.rs"), "old": "foo", "new": "baz" }]).to_string();
    let (report, _) = f.run(&json, Format::Json);
    assert!(report.applied);
    assert_eq!(f.read("b.rs"), "barbaz\n");
}

#[test]
fn crlf_files_keep_their_line_endings() {
    let f = Fixture::new(&[("a.rs", "one\r\ntwo\r\nthree\r\n")]);
    let (report, _) = f.run(&block("a.rs", "two\nthree\n", "2\n3\n"), Format::Blocks);
    assert!(report.applied, "{report:?}");
    assert_eq!(f.read("a.rs"), "one\r\n2\r\n3\r\n");
}

#[test]
fn a_file_without_a_final_newline_keeps_it_that_way() {
    let f = Fixture::new(&[("a.rs", "one\ntwo")]);
    let (report, _) = f.run(&block("a.rs", "two\n", "three\n"), Format::Blocks);
    assert!(report.applied, "{report:?}");
    assert_eq!(f.read("a.rs"), "one\nthree");
}

#[test]
fn edits_to_one_file_apply_in_order_and_every_failure_is_reported() {
    let f = Fixture::new(&[("a.rs", "one\n")]);
    let spec = block("a.rs", "one\n", "two\n") + &block("a.rs", "two\n", "three\n");
    let (report, _) = f.run(&spec, Format::Blocks);
    assert!(report.applied);
    assert_eq!(f.read("a.rs"), "three\n");

    let spec = block("a.rs", "nope\n", "x\n")
        + &block("a.rs", "three\n", "four\n")
        + &block("a.rs", "also nope\n", "y\n");
    let (report, _) = f.run(&spec, Format::Blocks);
    let statuses: Vec<_> = report.files[0].edits.iter().map(|e| e.status).collect();
    assert_eq!(
        statuses,
        vec![EditStatus::NoMatch, EditStatus::Ok, EditStatus::NoMatch]
    );
    assert_eq!(f.read("a.rs"), "three\n");
}

#[test]
fn patch_hunks_follow_anchors_and_earlier_hunks() {
    let f = Fixture::new(&[("a.rs", "fn a() {\n    x();\n}\n\nfn b() {\n    x();\n}\n")]);
    let spec = "*** Begin Patch\n*** Update File: {a.rs}\n@@ fn b() {\n-    x();\n+    y();\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert!(report.applied, "{report:?}");
    assert_eq!(
        f.read("a.rs"),
        "fn a() {\n    x();\n}\n\nfn b() {\n    y();\n}\n"
    );

    // Without an anchor the same hunk is ambiguous.
    let spec = "*** Begin Patch\n*** Update File: {a.rs}\n-    x();\n+    z();\n*** End Patch\n";
    let f = Fixture::new(&[("a.rs", "fn a() {\n    x();\n}\nfn b() {\n    x();\n}\n")]);
    let (report, _) = f.run(spec, Format::Patch);
    assert_eq!(report.files[0].edits[0].status, EditStatus::TooManyMatches);
}

#[test]
fn a_missing_anchor_is_reported_by_name() {
    let f = Fixture::new(&[("a.rs", "fn a() {}\n")]);
    let spec = "*** Begin Patch\n*** Update File: {a.rs}\n@@ fn missing()\n-fn a() {}\n+fn b() {}\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::AnchorNotFound);
    assert_eq!(edit.anchor.as_deref(), Some("fn missing()"));
}

#[test]
fn end_of_file_hunks_must_match_at_the_end() {
    let f = Fixture::new(&[("a.rs", "x\ny\nx\n")]);
    let spec = "*** Begin Patch\n*** Update File: {a.rs}\n-x\n+z\n*** End of File\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert!(report.applied, "{report:?}");
    assert_eq!(f.read("a.rs"), "x\ny\nz\n");
}

#[test]
fn patches_add_delete_and_move_files() {
    let f = Fixture::new(&[("a.rs", "keep\nold\n"), ("b.rs", "bye\n")]);
    let spec = "*** Begin Patch\n*** Add File: {new.txt}\n+hello\n*** Delete File: {b.rs}\n*** Update File: {a.rs}\n*** Move to: {moved.rs}\n keep\n-old\n+new\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert!(report.applied, "{report:?}");
    let statuses: Vec<_> = report.files.iter().map(|f| f.status).collect();
    assert_eq!(
        statuses,
        vec![FileStatus::Created, FileStatus::Deleted, FileStatus::Moved]
    );
    assert_eq!(report.files_written, 4);
    assert_eq!(f.read("new.txt"), "hello\n");
    assert!(!f.exists("b.rs"));
    assert!(!f.exists("a.rs"));
    assert_eq!(f.read("moved.rs"), "keep\nnew\n");
}

#[test]
fn file_preconditions_have_their_own_codes() {
    let f = Fixture::new(&[("a.rs", "a\n")]);
    let spec = "*** Begin Patch\n*** Add File: {a.rs}\n+x\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert_eq!(
        report.files[0].error.as_ref().unwrap().code,
        FileErrorCode::AlreadyExists
    );
    assert!(!report.only_missing_files());

    let spec = "*** Begin Patch\n*** Delete File: {b.rs}\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert_eq!(
        report.files[0].error.as_ref().unwrap().code,
        FileErrorCode::NotFound
    );
    assert!(report.only_missing_files());
}

#[test]
fn a_binary_file_cannot_be_updated() {
    let f = Fixture::new(&[]);
    fs::write(f.dir.path().join("bin"), [0xff, 0xfe, 0x00]).unwrap();
    let (report, _) = f.run(&block("bin", "x\n", "y\n"), Format::Blocks);
    assert_eq!(
        report.files[0].error.as_ref().unwrap().code,
        FileErrorCode::NotText
    );
}

#[test]
fn a_stale_hash_refuses_the_edit() {
    let f = Fixture::new(&[("a.rs", "one\n")]);
    let good = sha256_hex(b"one\n");
    let options = Options {
        expect_hashes: vec![(f.path("a.rs"), good)],
        ..Options::default()
    };
    let (report, _) = f.run_with(&block("a.rs", "one\n", "two\n"), Format::Blocks, &options);
    assert!(report.applied);

    let (report, _) = f.run_with(&block("a.rs", "two\n", "three\n"), Format::Blocks, &options);
    assert!(!report.applied);
    assert_eq!(
        report.files[0].error.as_ref().unwrap().code,
        FileErrorCode::HashMismatch
    );
    assert_eq!(f.read("a.rs"), "two\n");
}

#[test]
fn revert_restores_bytes_and_removes_created_files() {
    let f = Fixture::new(&[("a.rs", "one\n")]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(f.dir.path().join("a.rs"), fs::Permissions::from_mode(0o751)).unwrap();
    }
    let spec = block("a.rs", "one\n", "two\n")
        + "=== {new.txt}\n<<<<<<< SEARCH\n=======\nhi\n>>>>>>> REPLACE\n";
    let (report, journal) = f.run(&spec, Format::Blocks);
    assert!(report.applied);
    assert_eq!(f.read("a.rs"), "two\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(f.dir.path().join("a.rs"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o751, "the write keeps the file's mode");
    }
    revert(&journal.unwrap()).unwrap();
    assert_eq!(f.read("a.rs"), "one\n");
    assert!(!f.exists("new.txt"));
}

#[test]
fn the_diff_option_adds_a_unified_diff() {
    let f = Fixture::new(&[("a.rs", "one\n")]);
    let options = Options {
        diff: true,
        ..Options::default()
    };
    let (report, _) = f.run_with(&block("a.rs", "one\n", "two\n"), Format::Blocks, &options);
    let diff = report.files[0].diff.as_deref().unwrap();
    assert!(diff.contains("-one\n+two\n"), "{diff}");
}

#[test]
fn json_omits_empty_and_false_fields() {
    let f = Fixture::new(&[("a.rs", "one\n")]);
    let (report, _) = f.run(&block("a.rs", "one\n", "two\n"), Format::Blocks);
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("null"), "{json}");
    assert!(!json.contains("false"), "{json}");
    assert!(!json.contains("dry_run"), "{json}");
    assert!(json.contains(r#""expected":1"#), "{json}");
}

#[test]
fn two_spellings_of_one_path_edit_the_same_file() {
    let f = Fixture::new(&[("a.rs", "one two\n")]);
    // `dir/./a.rs` would not do: Path comparison already ignores a `.`.
    fs::create_dir(f.dir.path().join("sub")).unwrap();
    let alias = f.dir.path().join("sub").join("..").join("a.rs");
    let spec = format!(
        "=== {}\n<<<<<<< SEARCH\none two\n=======\n1 two\n>>>>>>> REPLACE\n=== {}\n<<<<<<< SEARCH\n1 two\n=======\n1 2\n>>>>>>> REPLACE\n",
        f.path("a.rs"),
        alias.display()
    );
    let (report, _) = f.run(&spec, Format::Blocks);
    assert!(report.applied, "{report:?}");
    assert_eq!(report.files_written, 1);
    assert_eq!(f.read("a.rs"), "1 2\n");
}

#[cfg(unix)]
#[test]
fn a_moved_file_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(&[("a.rs", "#!/bin/sh\necho hi\n")]);
    fs::set_permissions(f.dir.path().join("a.rs"), fs::Permissions::from_mode(0o755)).unwrap();
    let spec = "*** Begin Patch\n*** Update File: {a.rs}\n*** Move to: {moved.rs}\n #!/bin/sh\n-echo hi\n+echo bye\n*** End Patch\n";
    let (report, _) = f.run(spec, Format::Patch);
    assert!(report.applied, "{report:?}");
    let mode = fs::metadata(f.dir.path().join("moved.rs"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o755);
}

/// A stand-in for tree-sitter: `fn NAME` up to the first `}` at the same
/// indentation, and unbalanced braces as syntax errors.
#[derive(Debug)]
struct Braces;

impl Symbols for Braces {
    fn locate(&self, _: &Path, text: &str, selector: &str) -> Result<Vec<Span>, SymbolError> {
        if selector.contains(' ') {
            return Err(SymbolError::Invalid(format!("bad selector `{selector}`")));
        }
        let needle = format!("fn {selector}(");
        let spans: Vec<Span> = text
            .match_indices(&needle)
            .map(|(start, _)| {
                let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
                let indent = &text[line_start..start];
                let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
                let end = if text[start..line_end].trim_end().ends_with('}') {
                    line_end
                } else {
                    let close = format!("\n{indent}}}");
                    text[start..]
                        .find(&close)
                        .map_or(text.len(), |i| start + i + close.len())
                };
                Span {
                    start,
                    end,
                    line: text[..start].lines().count()
                        + usize::from(line_start == start && start > 0),
                    describe: format!("fn {selector}"),
                }
            })
            .collect();
        if spans.is_empty() {
            let all = text
                .match_indices("fn ")
                .map(|(i, _)| text[i..].split('(').next().unwrap_or("").to_string())
                .collect();
            return Err(SymbolError::NotFound(all));
        }
        Ok(spans)
    }

    fn syntax_errors(&self, _: &Path, text: &str) -> Option<usize> {
        let open = text.matches('{').count();
        let close = text.matches('}').count();
        Some(open.abs_diff(close))
    }
}

static BRACES: Braces = Braces;

fn with_braces() -> Options {
    Options {
        symbols: Some(&BRACES),
        ..Options::default()
    }
}

const IMPL: &str = "impl Vfox {\n    fn download(&self) {\n        get(\n            url,\n        );\n    }\n\n    fn other(&self) {}\n}\n";

#[test]
fn a_symbol_edit_replaces_the_definition_and_indents_the_new_text() {
    let f = Fixture::new(&[("a.rs", IMPL)]);
    let spec =
        "=== {a.rs}#download\nfn download(&self, retries: u32) {\n    get(url, retries);\n}\n";
    let (report, _) = f.run_with(spec, Format::Blocks, &with_braces());
    assert!(report.applied || report.files_written == 1, "{report:?}");
    assert_eq!(
        f.read("a.rs"),
        "impl Vfox {\n    fn download(&self, retries: u32) {\n        get(url, retries);\n    }\n\n    fn other(&self) {}\n}\n"
    );
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.symbol.as_deref(), Some("download"));
    assert_eq!(
        edit.signature,
        Some(SignatureChange {
            name: "download".into(),
            line: 2,
            before: "fn download(&self) {".into(),
            after: "fn download(&self, retries: u32) {".into(),
        })
    );
}

#[test]
fn an_unchanged_signature_is_not_reported() {
    let f = Fixture::new(&[("a.rs", IMPL)]);
    let spec = "=== {a.rs}#download\n    fn download(&self) {\n        get(url);\n    }\n";
    let (report, _) = f.run_with(spec, Format::Blocks, &with_braces());
    assert_eq!(report.files_written, 1);
    assert_eq!(report.files[0].edits[0].signature, None);
    assert!(
        f.read("a.rs")
            .contains("    fn download(&self) {\n        get(url);\n    }\n")
    );
}

#[test]
fn a_symbol_edit_that_finds_nothing_lists_the_definitions_and_writes_nothing() {
    let f = Fixture::new(&[("a.rs", IMPL)]);
    let (report, _) = f.run_with(
        "=== {a.rs}#upload\nfn upload() {}\n",
        Format::Blocks,
        &with_braces(),
    );
    assert_eq!(report.files_written, 0);
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::NoMatch);
    assert_eq!(edit.candidates, ["fn download", "fn other"]);
    assert_eq!(f.read("a.rs"), IMPL);
}

#[test]
fn an_ambiguous_symbol_is_refused_with_each_match() {
    let text = "fn a() {\n}\nmod m {\n    fn a() {\n    }\n}\n";
    let f = Fixture::new(&[("a.rs", text)]);
    let (report, _) = f.run_with("=== {a.rs}#a\nfn a() {}\n", Format::Blocks, &with_braces());
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::TooManyMatches);
    assert_eq!(edit.found, 2);
    assert_eq!(f.read("a.rs"), text);
}

#[test]
fn a_symbol_edit_that_breaks_the_syntax_is_refused() {
    let f = Fixture::new(&[("a.rs", IMPL)]);
    let (report, _) = f.run_with(
        "=== {a.rs}#download\nfn download(&self) {\n    get(url);\n",
        Format::Blocks,
        &with_braces(),
    );
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::DoesNotParse);
    assert!(
        edit.message
            .as_deref()
            .unwrap()
            .contains("1 syntax error where it has 0")
    );
    assert_eq!(f.read("a.rs"), IMPL);
}

#[test]
fn symbol_edits_need_a_resolver_and_a_valid_selector() {
    let f = Fixture::new(&[("a.rs", IMPL)]);
    let (report, _) = f.run("=== {a.rs}#download\nfn download() {}\n", Format::Blocks);
    assert_eq!(report.files[0].edits[0].status, EditStatus::Unsupported);
    let (report, _) = f.run_with(
        "=== {a.rs}#fn x y\nfn x() {}\n",
        Format::Blocks,
        &with_braces(),
    );
    let edit = &report.files[0].edits[0];
    assert_eq!(edit.status, EditStatus::Unsupported);
    assert_eq!(edit.message.as_deref(), Some("bad selector `fn x y`"));
}

#[test]
fn symbol_and_search_edits_to_one_file_apply_in_order() {
    let f = Fixture::new(&[("a.rs", IMPL)]);
    let spec = format!(
        "=== {{a.rs}}#other\nfn other(&self) -> u8 {{\n    1\n}}\n{}",
        block(
            "a.rs",
            "        get(\n            url,\n        );\n",
            "        get(url);\n"
        )
    );
    let (report, _) = f.run_with(&spec, Format::Blocks, &with_braces());
    assert_eq!(report.files_written, 1, "{report:?}");
    assert_eq!(
        f.read("a.rs"),
        "impl Vfox {\n    fn download(&self) {\n        get(url);\n    }\n\n    fn other(&self) -> u8 {\n        1\n    }\n}\n"
    );
}
