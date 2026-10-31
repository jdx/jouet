//! `jouet graph`, and the hook and `jouet edit` keeping the graph fresh.

use serde_json::{Value, json};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

struct Env {
    repo: tempfile::TempDir,
    home: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir(repo.path().join(".git")).unwrap();
        Self {
            repo,
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.repo.path()
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.root().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        // Older than now, so a rewrite in the same tick still differs.
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(5 + text.len() as u64))
            .unwrap();
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
        cmd.args(args)
            .current_dir(self.root())
            .env("HOME", self.home.path())
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("XDG_STATE_HOME", self.home.path().join("state"))
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("JOUET_DISPLAY", "plain");
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let out = self.run(&all);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn hook(&self, payload: &Value) {
        let mut child = self
            .command(&["hook", "claude", "post-tool-use"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }

    fn db_files(&self) -> Vec<String> {
        let dir = self.home.path().join("cache/jouet/graph");
        std::fs::read_dir(dir)
            .map(|d| {
                d.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| n.ends_with(".db"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[test]
fn index_builds_the_graph_in_the_cache_and_nothing_in_the_repo() {
    let env = Env::new();
    env.write("src/lib.rs", "pub fn alpha() {}\n");
    let report = env.json(&["graph", "index"]);
    assert_eq!(report["indexed"], 1);
    assert_eq!(report["files"], 1);
    let db = report["db"].as_str().unwrap();
    assert!(
        db.starts_with(env.home.path().join("cache/jouet/graph").to_str().unwrap()),
        "{db}"
    );
    let in_repo: Vec<_> = std::fs::read_dir(env.root())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(in_repo.len(), 2, "{in_repo:?}");

    let again = env.json(&["graph", "index"]);
    assert_eq!(again.get("indexed"), Some(&json!(0)));

    let full = env.json(&["graph", "index", "--full"]);
    assert_eq!(full["indexed"], 1);

    let text = env.run(&["graph", "status"]);
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("1 files"), "{text}");
    assert!(text.contains("outside the repository"), "{text}");
}

#[test]
fn outside_a_work_tree_is_a_precondition_failure() {
    let env = Env::new();
    std::fs::remove_dir(env.root().join(".git")).unwrap();
    let out = env.run(&["graph", "status"]);
    assert_eq!(out.status.code(), Some(4));
}

fn edit_payload(env: &Env, tool: &str, input: Value) -> Value {
    json!({
        "hook_event_name": "PostToolUse",
        "session_id": "s1",
        "cwd": env.root(),
        "tool_name": tool,
        "tool_input": input,
    })
}

/// The function names in the graph, read straight from its database, so
/// these tests do not depend on a query command refreshing it first.
fn outline(env: &Env) -> String {
    let status = env.json(&["graph", "status"]);
    let conn = rusqlite::Connection::open(status["db"].as_str().unwrap()).unwrap();
    let mut stmt = conn
        .prepare("SELECT name FROM nodes WHERE kind = 'function' ORDER BY name")
        .unwrap();
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names.join(",")
}

#[test]
fn the_hook_reindexes_what_a_file_tool_wrote_only_when_a_graph_exists() {
    let env = Env::new();
    env.write("a.py", "def one():\n    pass\n");
    env.hook(&edit_payload(
        &env,
        "Write",
        json!({"file_path": env.root().join("a.py")}),
    ));
    assert!(env.db_files().is_empty(), "a hook must not build a graph");

    env.json(&["graph", "index"]);
    env.write("a.py", "def two():\n    pass\n");
    env.hook(&edit_payload(
        &env,
        "Edit",
        json!({"file_path": env.root().join("a.py")}),
    ));
    assert_eq!(outline(&env), "two");
    let status = env.json(&["graph", "status"]);
    assert_eq!(status["files"], 1);
}

#[test]
fn the_hook_reads_the_files_a_codex_patch_names() {
    let env = Env::new();
    env.write("x.go", "package x\n\nfunc A() {}\n");
    env.json(&["graph", "index"]);
    env.write("x.go", "package x\n\nfunc B() {}\n");
    env.write("y.go", "package x\n\nfunc C() {}\n");
    let patch = "*** Begin Patch\n*** Update File: x.go\n@@\n-func A() {}\n+func B() {}\n*** Add File: y.go\n+package x\n*** End Patch\n";
    env.hook(&edit_payload(
        &env,
        "apply_patch",
        json!({"command": patch}),
    ));
    let status = env.json(&["graph", "status"]);
    assert_eq!(status["files"], 2);
    assert_eq!(outline(&env), "B,C");
}

#[test]
fn jouet_edit_reindexes_what_it_wrote() {
    let env = Env::new();
    env.write("m.rb", "def old_name\nend\n");
    env.json(&["graph", "index"]);
    let mut child = env
        .command(&["edit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            b"=== m.rb\n<<<<<<< SEARCH\ndef old_name\n=======\ndef new_name\n>>>>>>> REPLACE\n",
        )
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(outline(&env), "new_name");
}

fn many_callers(env: &Env) {
    env.write("lib.py", "def target():\n    return 1\n");
    let mut caller = String::from("from lib import target\n\n");
    for i in 0..40 {
        caller.push_str(&format!(
            "\ndef caller_number_{i}():\n    return target()\n"
        ));
    }
    env.write("use.py", &caller);
}

#[test]
fn queries_refresh_first_and_page_within_their_budget() {
    let env = Env::new();
    many_callers(&env);
    // No index was ever built: the first query builds it.
    let out = env.run(&["graph", "callers", "target", "--budget", "400"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.starts_with("callers of function target (lib.py:1): 40\n"),
        "{text}"
    );
    assert!(text.len() < 700, "{text}");
    let marker = text.lines().last().unwrap();
    assert!(marker.starts_with("... "), "{marker}");
    assert!(marker.contains("--page 2)"), "{marker}");

    // Page 2 continues where page 1 stopped, and every caller is on a page.
    let mut seen = Vec::new();
    for page in 1..=20 {
        let doc = env.json(&[
            "graph",
            "callers",
            "target",
            "--budget",
            "400",
            "--page",
            &page.to_string(),
        ]);
        let s = &doc["symbols"][0];
        for item in s["items"].as_array().unwrap() {
            seen.push(item["name"].as_str().unwrap().to_string());
        }
        if s["page"] == s["pages"] {
            break;
        }
    }
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 40);

    // A file changed since: the next query sees it.
    env.write(
        "use.py",
        "from lib import target\n\n\ndef only():\n    return target()\n",
    );
    let doc = env.json(&["graph", "callers", "target"]);
    assert_eq!(doc["symbols"][0]["total"], 1);
}

#[test]
fn symbols_explore_and_impact_answer_and_say_what_they_left_out() {
    let env = Env::new();
    many_callers(&env);
    let symbols = env.json(&["graph", "symbols", "caller number", "--budget", "300"]);
    assert_eq!(symbols["total"], 40);
    assert!(symbols["pages"].as_u64().unwrap() > 1);

    let out = env.run(&["graph", "explore", "target", "--budget", "2000"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("lib.py:1-2 function target"), "{text}");
    assert!(text.contains("called by: caller_number_0"), "{text}");
    assert!(text.contains("... 35 more"), "{text}");

    // The 40 callers, and the file that imports it.
    let impact = env.json(&["graph", "impact", "lib.py#target"]);
    assert_eq!(impact["symbols"][0]["total"], 41);

    let missing = env.run(&["graph", "callers", "no_such_thing"]);
    assert_eq!(missing.status.code(), Some(3));
}

#[test]
fn affected_lists_tests_and_fails_when_a_file_is_unknown() {
    let env = Env::new();
    env.write("src/util.py", "def f():\n    return 1\n");
    env.write(
        "tests/test_util.py",
        "from src.util import f\n\n\ndef test_f():\n    assert f() == 1\n",
    );
    let ok = env.run(&["graph", "affected", "src/util.py"]);
    assert!(ok.status.success());
    let text = String::from_utf8(ok.stdout).unwrap();
    assert_eq!(text, "tests/test_util.py  (via src/util.py)\n");
    let unknown = env.run(&["graph", "affected", "src/util.py", "notes.txt"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8(unknown.stdout)
            .unwrap()
            .contains("unknown: notes.txt")
    );
}
