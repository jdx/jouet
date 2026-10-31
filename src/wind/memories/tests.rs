use super::*;
use crate::wind::Paths;

const NOW: u64 = 1_793_354_400;

fn rules() -> Vec<String> {
    vec!["git-stash".into(), "no-verify".into()]
}

fn reference<'a>(claims: &'a Claims, key: &str) -> &'a Reference {
    claims
        .references
        .iter()
        .find(|r| r.key == key)
        .unwrap_or_else(|| panic!("no {key} in {claims:#?}"))
}

#[test]
fn a_workaround_waiting_on_a_release_is_read_as_one() {
    // The note that started this: merged, but not in a release yet.
    let body = "Run cargo through `mise x -- cargo` in those worktrees.\n\n\
        **When to drop this:** jdx/mr-boxington#886 (merged 2026-10-31) keeps a target \
        that a running process is using. It isn't in a release yet: 1.29.0 (2026-10-29) \
        predates it, and this machine runs 1.28.0.\n";
    let c = claims(body, Some("jdx/mise"), &rules());
    let r = reference(&c, "jdx/mr-boxington#886");
    assert_eq!(r.said, Some(Said::Done));
    assert!(r.awaits_release);
    assert_eq!(c.commands, Vec::<Vec<String>>::new());
}

#[test]
fn what_is_said_beside_a_reference_decides() {
    let c = claims(
        "Fix pending: npm/cli#9876.\n\n\
         Brand (2026-10-30, jdx/jouet#16, open): the tagline.\n\n\
         packslip [#74](https://github.com/jdx/packslip/pull/74) is open and green.\n\n\
         `jouet wait` merged in jdx/jouet#7; `wait ci` (jdx/jouet#9) is still open.\n\n\
         When main merges fixes while a docs branch is open (mise docs stack jdx/mise#14098), merging main keeps the old text.\n\n\
         Fixed in jdx/mise#13193 with an observer that disconnects once every link has a badge.\n",
        None,
        &rules(),
    );
    let said = |key: &str| reference(&c, key).said;
    assert_eq!(said("npm/cli#9876"), Some(Said::Waiting));
    assert_eq!(said("jdx/jouet#16"), Some(Said::Waiting));
    assert_eq!(said("jdx/packslip#74"), Some(Said::Waiting));
    assert_eq!(said("jdx/jouet#7"), Some(Said::Done));
    assert_eq!(said("jdx/jouet#9"), Some(Said::Waiting));
    // "is open" about a branch, not the PR.
    assert_eq!(said("jdx/mise#14098"), None);
    // "once" about something else, beside "fixed in".
    assert_eq!(said("jdx/mise#13193"), Some(Said::Done));
}

#[test]
fn a_sentence_cue_goes_to_the_reference_nearest_it() {
    let c = claims(
        "Stack: jdx/packslip#174 installer, then jdx/packslip#179 image, then jdx/packslip#181 docs, which waits on the first image.\n",
        None,
        &rules(),
    );
    assert_eq!(reference(&c, "jdx/packslip#174").said, None);
    assert_eq!(reference(&c, "jdx/packslip#181").said, Some(Said::Waiting));
}

#[test]
fn done_anywhere_in_a_memory_wins() {
    let c = claims(
        "Opened jdx/mise#13797 while it was pending review.\n\n\
         Later, jdx merged jdx/mise#13797 by accident.\n",
        None,
        &rules(),
    );
    assert_eq!(reference(&c, "jdx/mise#13797").said, Some(Said::Done));
}

#[test]
fn release_cues_mark_a_reference_as_waiting_for_a_release() {
    let c = claims(
        "The root cause is jdx/usage#1566, and once it ships `rejoin` is only a fallback.\n\n\
         jdx/mise#13573 (2026-09-24): `include_bytes!` of the marker, plus unreleased `build/registry_url.rs` and `schema/mise.json` beside it.\n\n\
         jdx/hk#1519 and jdx/hk#1520 are all on main, none in the v2.3.1 release.\n",
        None,
        &rules(),
    );
    assert!(reference(&c, "jdx/usage#1566").awaits_release);
    // "Unreleased" far from the reference is about the files.
    assert!(!reference(&c, "jdx/mise#13573").awaits_release);
    let hk = reference(&c, "jdx/hk#1520");
    assert_eq!((hk.said, hk.awaits_release), (Some(Said::Done), true));
}

#[test]
fn a_bare_number_belongs_to_the_repository_named_before_it() {
    // A memory naming no repository: its project's.
    let c = claims("Waiting on #12 (still open).\n", Some("jdx/mise"), &rules());
    assert_eq!(c.references[0].key, "jdx/mise#12");
    // Naming one: that one.
    let c = claims(
        "Fix pending: npm/cli#9876 + PR #9877.\n",
        Some("aubepkg/aube"),
        &rules(),
    );
    assert!(c.references.iter().any(|r| r.key == "npm/cli#9877"));
    // Naming several: the one named last before it in the paragraph.
    let c = claims(
        "Brand: jdx/jouet#16. Stack: #9 is open.\n\nSee jdx/mise#15201 too.\n\nAlso #5.\n",
        Some("jdx/mise"),
        &rules(),
    );
    assert!(c.references.iter().any(|r| r.key == "jdx/jouet#9"));
    // With nothing before it, a bare number is left out.
    assert!(!c.references.iter().any(|r| r.key.ends_with("#5")));
    // A Markdown link names the same one twice.
    let c = claims(
        "[#59](https://github.com/jdx/packslip/pull/59) is open.\n",
        Some("jdx/mise"),
        &rules(),
    );
    assert_eq!(c.references.len(), 1);
    // `name#12` takes the owner the memory names elsewhere.
    let c = claims(
        "See jdx/mise#1. Then mise#13713 is pending.\n",
        None,
        &rules(),
    );
    assert_eq!(reference(&c, "jdx/mise#13713").said, Some(Said::Waiting));
}

#[test]
fn jouet_commands_and_rule_overrides_are_read_from_code_spans() {
    let c = claims(
        "Use `jouet read --outline/--symbol` and `jouet wind show <ID>`.\n\n\
         Roadmap: no MCP, jdx skipped `jouet mcp` for now.\n\n\
         The `git-stash` rule denies wrongly here, so override it with `git stash push -m x`.\n\n\
         Never use `no-verify` tricks; `git-stash` is fine.\n\n\
         ```\njouet frobnicate\n```\n",
        None,
        &rules(),
    );
    // Placeholders are left out, and so is code in a fence.
    assert_eq!(
        c.commands,
        vec![vec![
            "read".to_string(),
            "--outline".into(),
            "--symbol".into()
        ]]
    );
    assert_eq!(c.overrides.len(), 1);
    assert_eq!(c.overrides[0].0, "git-stash");
}

#[test]
fn folders_are_named_as_claude_code_names_them() {
    assert_eq!(
        folder_of(Path::new("/home/me/src/mise-versions")),
        "-home-me-src-mise-versions"
    );
    assert_eq!(folder_of(Path::new("/tmp/a.b_c")), "-tmp-a-b-c");
}

#[test]
fn a_folder_is_found_on_disk_and_a_worktree_belongs_to_its_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().canonicalize().unwrap();
    // `a-b` and `a/b` both exist: the walk tries the longer name first, and
    // gives the directory whose name encodes to the folder's.
    let main = base.join("a-b");
    std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    std::fs::create_dir_all(base.join("a/b")).unwrap();
    std::fs::write(
        main.join(".git/config"),
        "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = git@github.com:jdx/mise.git\n",
    )
    .unwrap();
    let folder = folder_of(&main);
    assert!(folder.starts_with('-'));
    assert_eq!(project_dir(&folder), Some(main.clone()));
    assert_eq!(origin_repo(&main).as_deref(), Some("jdx/mise"));

    // A linked worktree elsewhere reads the memories of the checkout it
    // was added from.
    let wt = base.join("elsewhere/wt");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
    )
    .unwrap();
    std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
    assert!(in_project(&folder, &wt.join("src")));
    assert!(in_project(&folder, &main));
    // Claude Code names `a/b` the same, so it shares the memories too.
    assert!(in_project(&folder, &base.join("a/b")));
    assert!(!in_project(&folder, &base.join("elsewhere")));
}

#[test]
fn origin_urls_are_read_in_their_forms() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    for (url, want) in [
        ("https://github.com/jdx/jouet", Some("jdx/jouet")),
        ("https://github.com/jdx/jouet.git", Some("jdx/jouet")),
        ("ssh://git@github.com/jdx/hk.git", Some("jdx/hk")),
        ("https://gitlab.com/jdx/jouet.git", None),
    ] {
        std::fs::write(
            dir.path().join(".git/config"),
            format!("[remote \"upstream\"]\n\turl = x\n[remote \"origin\"]\n\turl = {url}\n"),
        )
        .unwrap();
        assert_eq!(origin_repo(dir.path()).as_deref(), want, "{url}");
    }
}

fn harness_dirs() -> (tempfile::TempDir, Roots) {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    let memory = claude.join("projects/-work-app/memory");
    std::fs::create_dir_all(&memory).unwrap();
    std::fs::write(memory.join("MEMORY.md"), "- [A](a.md) — the index\n").unwrap();
    std::fs::write(
        memory.join("a.md"),
        "---\nname: mbx-gc\ndescription: \"cargo through mise x\"\nmetadata:\n  type: feedback\n---\n\nUntil jdx/mr-boxington#886 is merged, run `mise x -- cargo`.\n",
    )
    .unwrap();
    std::fs::write(memory.join("notes.txt"), "not a memory").unwrap();
    std::fs::write(
        claude.join("CLAUDE.md"),
        "# Global\n\nNever capitalize mise.\n",
    )
    .unwrap();
    let codex = dir.path().join("codex");
    std::fs::create_dir_all(codex.join("memories")).unwrap();
    std::fs::write(codex.join("AGENTS.md"), "# Agents\n").unwrap();
    std::fs::write(codex.join("memories/MEMORY.md"), "# Task families\n").unwrap();
    std::fs::write(codex.join("memories_1.sqlite"), "").unwrap();
    let roots = Roots {
        claude: Some(claude),
        codex: Some(codex),
    };
    (dir, roots)
}

#[test]
fn memories_are_found_in_both_harnesses_but_not_their_indexes() {
    let (_dir, roots) = harness_dirs();
    assert!(roots.any());
    let found = discover(&roots);
    let names: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|m| (m.description.as_str(), m.project.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("cargo through mise x", Some("-work-app")),
            ("Global", None),
            ("Agents", None),
            ("Task families", None),
        ]
    );
    assert_eq!(found[0].name, "mbx-gc");
    assert!(found[0].body.starts_with("\nUntil"));
    assert_eq!(found[0].origin, Origin::Claude);
    assert!(found[1].name.ends_with("CLAUDE.md"));
    assert!(
        !Roots {
            claude: None,
            codex: None
        }
        .any()
    );
}

fn lookup(state: &str, released: Option<(&str, &str)>) -> Lookup {
    Lookup {
        checked: NOW,
        pr: true,
        state: state.into(),
        reason: None,
        at: Some("2026-10-31".into()),
        commit: Some("abc".into()),
        released: released.map(|(t, d)| (t.to_string(), d.to_string())),
        compared: None,
    }
}

fn memory(name: &str, project: Option<&str>, body: &str) -> Memory {
    Memory {
        origin: Origin::Claude,
        path: PathBuf::from(format!("/m/{name}.md")),
        name: name.into(),
        description: String::new(),
        project: project.map(str::to_string),
        body: body.into(),
    }
}

#[test]
fn findings_compare_what_a_memory_says_with_what_is_true() {
    let memories = vec![
        memory(
            "waits",
            Some("-work-app"),
            "Until jdx/a#1 merges, pin it.\n",
        ),
        memory("released", None, "jdx/a#2 (merged) is not released yet.\n"),
        memory(
            "unreleased",
            None,
            "jdx/a#3 (merged) is not released yet.\n",
        ),
        memory("history", None, "Fixed in jdx/a#4.\n"),
        memory("open", None, "jdx/a#5 is still open.\n"),
        memory("gone", None, "Run `jouet frob` and `jouet wind show n1`.\n"),
        memory(
            "jouet",
            None,
            "Override `git-stash` when it denies wrongly.\n",
        ),
    ];
    let checked = extract(&memories, &rules());
    let mut lookups = BTreeMap::new();
    lookups.insert("jdx/a#1".to_string(), lookup("MERGED", None));
    lookups.insert(
        "jdx/a#2".to_string(),
        lookup("MERGED", Some(("v1.30.0", "2026-11-02"))),
    );
    lookups.insert("jdx/a#3".to_string(), lookup("MERGED", None));
    lookups.insert("jdx/a#4".to_string(), lookup("MERGED", None));
    lookups.insert("jdx/a#5".to_string(), lookup("OPEN", None));
    // Only what a memory waits on and could still change is asked about.
    let wanted = to_look_up(&checked, &BTreeMap::new(), NOW);
    assert_eq!(
        wanted.into_iter().collect::<Vec<_>>(),
        [
            ("jdx/a#1".to_string(), false),
            ("jdx/a#2".to_string(), true),
            ("jdx/a#3".to_string(), true),
            ("jdx/a#5".to_string(), false),
        ]
    );
    // Settled answers are not asked again; a release never compared is.
    let again = to_look_up(&checked, &lookups, NOW + 60);
    assert_eq!(again.into_keys().collect::<Vec<_>>(), ["jdx/a#3"]);
    let unknown = |words: &[&str]| (words[0] == "frob").then(|| "jouet frob".to_string());
    let found = findings(&checked, &lookups, &unknown);
    let got: Vec<(&str, Kind, &str)> = found
        .iter()
        .map(|f| (f.memory.as_str(), f.kind, f.found.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            (
                "waits",
                Kind::Resolved,
                "waits on PR jdx/a#1, which was merged on 2026-10-31"
            ),
            (
                "released",
                Kind::Released,
                "waits for a release with jdx/a#2, and v1.30.0 (2026-11-02) has it"
            ),
            (
                "gone",
                Kind::Gone,
                "names `jouet frob`, which this jouet does not have"
            ),
            (
                "jouet",
                Kind::Jouet,
                "seems to work around jouet's built-in rule `git-stash`"
            ),
        ]
    );
    assert_eq!(found[0].project.as_deref(), Some("-work-app"));
}

#[test]
fn issues_closed_for_any_reason_are_findings() {
    let memories = vec![memory("m", None, "Blocked on jdx/a#7 for now.\n")];
    let checked = extract(&memories, &rules());
    let mut l = lookup("CLOSED", None);
    l.pr = false;
    l.reason = Some("NOT_PLANNED".into());
    let lookups = BTreeMap::from([("jdx/a#7".to_string(), l)]);
    let found = findings(&checked, &lookups, &|_| None);
    assert_eq!(
        found[0].found,
        "waits on issue jdx/a#7, which was closed as not planned on 2026-10-31"
    );
}

fn paths() -> (tempfile::TempDir, Paths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths {
        ledger: dir.path().join("ledger"),
        state: dir.path().join("wind.json"),
        rules: dir.path().join("rules"),
        log: dir.path().join("rule-log.jsonl"),
    };
    (dir, paths)
}

fn finding(memory: &str, project: Option<&str>) -> Finding {
    Finding {
        memory: memory.into(),
        path: format!("/m/{memory}.md"),
        project: project.map(str::to_string),
        kind: Kind::Resolved,
        about: "jdx/a#1".into(),
        found: "waits on PR jdx/a#1, which was merged on 2026-10-31".into(),
    }
}

#[test]
fn notes_are_recorded_twice_a_day_once_each_and_shown_in_their_project() {
    let (_dir, paths) = paths();
    let found = [
        finding("one", None),
        finding("two", Some("-work-app")),
        finding("three", None),
    ];
    let record_now = |now: u64| {
        crate::wind::update(&paths.state, |s| record(s, &found, &BTreeMap::new(), now)).unwrap()
    };
    assert_eq!(record_now(NOW), ["m1", "m2"]);
    // The day's two are taken; tomorrow the third, and never one again.
    assert!(record_now(NOW + 60).is_empty());
    assert_eq!(record_now(NOW + 86_400), ["m3"]);
    assert!(record_now(NOW + 2 * 86_400).is_empty());

    // A session elsewhere sees the global ones only.
    let lines = lines_for_session(&paths, "s1", Some(Path::new("/elsewhere")), 2);
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0],
        "jouet wind: memory `one` waits on PR jdx/a#1, which was merged on 2026-10-31: the bug it works around may be fixed. At a natural pause, check whether it still happens, then ask the user before updating or deleting the memory. `jouet wind show m1` for its path; `jouet wind adopt m1` once done, `jouet wind dismiss m1` to drop it."
    );
    assert!(lines[1].contains("`three`"));
    let lines = lines_for_session(&paths, "s2", Some(Path::new("/work/app")), 2);
    assert!(lines[1].contains("`two`"));
    assert!(lines_for_session(&paths, "s3", None, 0).is_empty());

    // Dismissed, it is never shown again, nor recorded again.
    assert!(crate::wind::set_status(&paths, "m1", Status::Dismissed));
    let lines = lines_for_session(&paths, "s4", None, 2);
    assert!(lines.iter().all(|l| !l.contains("`one`")));
    let state = State::load(&paths.state);
    assert_eq!(state.memories.note("m1").unwrap().path, "/m/one.md");
}

#[test]
fn a_memory_that_works_around_jouet_points_at_its_idea() {
    let note = MemoryNote {
        id: "m4".into(),
        created: NOW,
        memory: "stash".into(),
        path: "~/m.md".into(),
        project: None,
        kind: Kind::Jouet,
        about: "git-stash".into(),
        found: "seems to work around jouet's built-in rule `git-stash`".into(),
        idea: Some("note-a-saved-memory-works-around".into()),
        status: Status::Pending,
        shown: Vec::new(),
    };
    assert_eq!(
        line(&note),
        "jouet wind: memory `stash` seems to work around jouet's built-in rule `git-stash`: a jouet bug belongs with jouet's maintainers, not in a memory. Tell the user, and ask before deleting the memory. `jouet share --idea note-a-saved-memory-works-around` drafts a report; post it with `--yes` only if they agree. `jouet wind show m4` for its path; `jouet wind adopt m4` once done, `jouet wind dismiss m4` to drop it."
    );
}

#[test]
fn the_check_runs_once_a_day() {
    let (_dir, paths) = paths();
    assert!(due(&paths, NOW));
    assert!(!due(&paths, NOW + 3600));
    assert!(due(&paths, NOW + EVERY));
}

#[test]
fn a_review_sees_read_and_noted_memories_whole_and_the_rest_as_an_index() {
    let a = memory(
        "a",
        Some("-work-app"),
        "token ghp_abcdefghijklmnopqrstuvwxyz0123456789 here",
    );
    let b = memory("b", None, "B's text");
    let c = memory("c", Some("-other"), "C's text");
    let all = [a, b, c];
    let ours = for_projects(&all, &[PathBuf::from("/work/app")]);
    assert_eq!(ours.len(), 2);
    let read = BTreeSet::from(["/m/b.md".to_string()]);
    let noted = BTreeSet::new();
    let shown = for_review(&ours, &read, &noted, &|t| jouet_audit::redact(t));
    assert_eq!(shown[0].path, "/m/b.md");
    assert_eq!(shown[0].text.as_deref(), Some("B's text"));
    assert_eq!(shown[1].text, None);
    let noted = BTreeSet::from(["/m/a.md".to_string()]);
    let shown = for_review(&ours, &BTreeSet::new(), &noted, &|t| jouet_audit::redact(t));
    let text = shown[0].text.as_deref().unwrap();
    assert!(!text.contains("ghp_"), "{text}");
    let rendered = render(&shown);
    assert!(rendered.starts_with("\n## Memories\n"));
    assert!(rendered.contains("- /m/b.md — b: \n"));
    assert!(rendered.contains("### /m/a.md\n"));
    assert_eq!(render(&[]), "");
}
