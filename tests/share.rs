//! `jouet share` against a local fake GitHub: nothing here talks to the real
//! API, and the fake answers the post instead of GitHub.

mod common;

use common::{CUT_SHORT, Fake, RATE_LIMITED, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

const TOKEN: &str = "good-token";
const URL: &str = "https://github.com/jdx/jouet/discussions/99";

/// A rule of the user's, with private details in its why and message.
const RULE: &str = r#"--- why: tags are cut by the release job, see /home/alice/acme/RELEASING.md
--- mode: deny
return {
	check = function(call)
		for _, c in call.commands do
			if c.argv[1] == "git" and c.argv[2] == "tag" then
				return jouet.deny("Leave tags to deploy.acme.internal.")
			end
		end
		return nil
	end,
	examples = { fires = { "git tag v1" }, quiet = { "git status" } },
}
"#;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A home whose rule `no-tags` has denied for nine days and fired twelve
/// times, once overridden.
fn home(rule: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join("config/jouet/rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(rules.join("no-tags.luau"), rule).unwrap();
    let state = dir.path().join("state/jouet");
    std::fs::create_dir_all(state.join("ledger")).unwrap();
    let t = now() - 3600;
    let mut ledger = String::new();
    for i in 0..12 {
        ledger.push_str(&format!(
            "{{\"ts\":{t},\"harness\":\"claude\",\"session\":\"s{i}\",\"event\":\"pre-tool-use\",\"tool\":\"Bash\",\"shape\":\"git tag <arg>\",\"decisions\":[{{\"rule\":\"no-tags\",\"action\":\"deny\"}}]}}\n\
             {{\"ts\":{t},\"harness\":\"claude\",\"session\":\"s{i}\",\"event\":\"pre-tool-use\",\"tool\":\"Bash\",\"shape\":\"git status\"}}\n"
        ));
    }
    ledger.push_str(&format!(
        "{{\"ts\":{t},\"harness\":\"claude\",\"session\":\"s0\",\"event\":\"override\",\"tool\":\"Bash\",\"shape\":\"git tag <arg>\",\"decisions\":[{{\"rule\":\"no-tags\",\"action\":\"override\"}}]}}\n"
    ));
    std::fs::write(state.join("ledger/ledger.jsonl"), ledger).unwrap();
    let promoted = now() - 9 * 86_400;
    std::fs::write(
        state.join("rule-log.jsonl"),
        format!("{{\"ts\":{promoted},\"rule\":\"no-tags\",\"change\":\"promoted\",\"from\":\"warn\",\"to\":\"deny\",\"reason\":\"x\",\"by\":\"user\"}}\n"),
    )
    .unwrap();
    dir
}

fn jouet(fake: &Fake, home: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("JOUET_GITHUB_API_URL", &fake.url)
        .env("JOUET_GITHUB_TOKEN", TOKEN)
        .env("JOUET_REPLY_FOOTER", "*AI-assisted — Tool: test.*")
        .env("USER", "alice");
    for var in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "JOUET_DISPLAY",
        "HOSTNAME",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    cmd.output().unwrap()
}

/// What the fake answers: the lookup of the viewer, the repository, its
/// categories and any existing proposal, then the post.
#[derive(Clone, Copy)]
struct Script {
    category: bool,
    duplicate: bool,
}

const FINE: Script = Script {
    category: true,
    duplicate: false,
};

fn github(script: Script) -> Fake {
    serve(move |request, _| respond(script, request))
}

fn respond(script: Script, request: &Request) -> (u16, String) {
    let q = query(request);
    let body = if q.starts_with("mutation") {
        json!({"data": {"createDiscussion": {"discussion": {"url": URL}}}})
    } else {
        let mut categories = vec![json!({"id": "C1", "name": "General"})];
        if script.category {
            categories.push(json!({"id": "C2", "name": "Rules & ideas"}));
        }
        let title = request.body["variables"]["q"]
            .as_str()
            .and_then(|q| q.split('"').nth(1))
            .unwrap_or_default();
        let found: Vec<Value> = if script.duplicate {
            vec![json!({"title": title, "url": "https://github.com/jdx/jouet/discussions/7"})]
        } else {
            vec![json!({"title": "Rule: something else", "url": "x"})]
        };
        json!({"data": {
            "viewer": {"login": "alice-gh"},
            "repository": {"id": "R1", "discussionCategories": {"nodes": categories}},
            "search": {"nodes": found},
        }})
    };
    (200, body.to_string())
}

fn mutations(fake: &Fake) -> Vec<Request> {
    fake.requests()
        .into_iter()
        .filter(|r| query(r).starts_with("mutation"))
        .collect()
}

#[test]
fn without_yes_it_shows_the_proposal_and_posts_nothing() {
    let fake = github(FINE);
    let home = home(RULE);
    let out = jouet(&fake, home.path(), &["share", "no-tags"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("title: Rule: no-tags\n"), "{text}");
    assert!(
        text.contains("Fired 12 times in 12 sessions; overridden 1 times (8%)"),
        "{text}"
    );
    assert!(text.contains("Nothing was posted."), "{text}");
    // The diff shows what anonymizing changed.
    assert!(
        text.contains("-Leave tags") || text.contains("deploy.acme.internal"),
        "{text}"
    );
    assert!(
        text.contains("+--- why: tags are cut by the release job, see <path>"),
        "{text}"
    );
    assert!(fake.requests().is_empty());
}

#[test]
fn yes_posts_the_anonymized_proposal_once() {
    // The first read is cut short and retried; the post is sent once.
    let fake = serve(|request, index| {
        let (status, body) = respond(FINE, request);
        if index == 0 {
            (CUT_SHORT, body)
        } else {
            (status, body)
        }
    });
    let home = home(RULE);
    let out = jouet(&fake, home.path(), &["share", "no-tags", "--yes", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["posted"], true);
    assert_eq!(doc["url"], URL);

    let posts = mutations(&fake);
    assert_eq!(posts.len(), 1);
    let vars = &posts[0].body["variables"];
    assert_eq!(vars["cat"], "C2");
    assert_eq!(vars["title"], "Rule: no-tags");
    let body = vars["body"].as_str().unwrap();
    assert!(body.ends_with("*AI-assisted — Tool: test.*\n"), "{body}");
    for private in ["/home/alice", "acme.internal", "alice"] {
        assert!(!body.contains(private), "{private} leaked:\n{body}");
    }
    assert_eq!(posts[0].token, TOKEN);
    assert_eq!(
        (posts[0].method.as_str(), posts[0].path.as_str()),
        ("POST", "/graphql")
    );

    let log = std::fs::read_to_string(home.path().join("state/jouet/rule-log.jsonl")).unwrap();
    assert!(
        log.contains(r#""change":"shared""#) && log.contains("alice-gh"),
        "{log}"
    );
}

#[test]
fn a_missing_category_or_a_duplicate_posts_nothing() {
    let home = home(RULE);
    let fake = github(Script {
        category: false,
        duplicate: false,
    });
    let out = jouet(&fake, home.path(), &["share", "no-tags", "--yes"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("has no \"Rules & ideas\" Discussions category yet"),
        "{}",
        stderr(&out)
    );
    assert!(mutations(&fake).is_empty());

    let fake = github(Script {
        category: true,
        duplicate: true,
    });
    let out = jouet(&fake, home.path(), &["share", "no-tags", "--yes"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains(
            "already exists, so nothing was posted: https://github.com/jdx/jouet/discussions/7"
        ),
        "{}",
        stderr(&out)
    );
    assert!(mutations(&fake).is_empty());
}

#[test]
fn three_posts_a_week_at_most() {
    let home = home(RULE);
    let t = now() - 60;
    let line = format!("{{\"ts\":{t},\"kind\":\"rule\",\"title\":\"x\",\"url\":\"u\"}}\n");
    std::fs::write(home.path().join("state/jouet/shared.jsonl"), line.repeat(3)).unwrap();
    let fake = github(FINE);
    let out = jouet(&fake, home.path(), &["share", "no-tags", "--yes"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("3 proposals were posted in the last week"),
        "{}",
        stderr(&out)
    );
    assert!(fake.requests().is_empty());
}

#[test]
fn a_secret_or_an_unproven_rule_is_refused_before_any_request() {
    let fake = github(FINE);
    let leaky = RULE.replace(
        "deploy.acme.internal",
        "ghp_abcdefghijklmnopqrstuvwxyz0123456789",
    );
    let out = jouet(&fake, home(&leaky).path(), &["share", "no-tags", "--yes"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("looks like a secret"),
        "{}",
        stderr(&out)
    );
    assert!(!stderr(&out).contains("ghp_abc"), "{}", stderr(&out));

    let warn = RULE.replace("--- mode: deny\n", "");
    let out = jouet(&fake, home(&warn).path(), &["share", "no-tags", "--yes"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("is not ready to share:\n- it is in warn mode"),
        "{}",
        stderr(&out)
    );

    let out = jouet(&fake, home(RULE).path(), &["share", "nope"]);
    assert_eq!(out.status.code(), Some(3));
    assert!(fake.requests().is_empty());
}

#[test]
fn an_idea_from_recorded_gaps() {
    let home = home(RULE);
    let fake = github(FINE);
    let out = jouet(&fake, home.path(), &["share", "--idea"]);
    assert_eq!(out.status.code(), Some(4));
    assert!(
        stderr(&out).contains("no gaps are recorded"),
        "{}",
        stderr(&out)
    );

    let out = jouet(
        &fake,
        home.path(),
        &[
            "rule",
            "gap",
            "I kept checking CI before pushing; there is no primitive for CI state",
            "--rule",
            "no-tags",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let out = jouet(&fake, home.path(), &["share", "--idea", "--yes"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let posts = mutations(&fake);
    assert_eq!(posts.len(), 1);
    let title = posts[0].body["variables"]["title"].as_str().unwrap();
    assert!(title.starts_with("Idea: I kept checking CI"), "{title}");

    // A shared gap is not proposed again.
    let out = jouet(&fake, home.path(), &["share", "--idea"]);
    assert_eq!(out.status.code(), Some(4));
}

#[test]
fn a_github_failure_posts_nothing_and_exits_5() {
    let fake = serve(|_, _| {
        (
            RATE_LIMITED,
            r#"{"message":"API rate limit exceeded"}"#.into(),
        )
    });
    let out = jouet(&fake, home(RULE).path(), &["share", "no-tags", "--yes"]);
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    assert!(mutations(&fake).is_empty());
}
