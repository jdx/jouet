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
    let json = format!(
        r#"[{{"path":"{}","old":"foo","new":"baz"}}]"#,
        f.path("b.rs")
    );
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
