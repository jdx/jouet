//! `pr status`, `pr threads` and `wait ci` against a fake GitLab and a fake
//! Forgejo. Nothing here talks to a real forge.
//!
//! The Forgejo responses are recorded from codeberg.org (forgejo/forgejo#15077),
//! trimmed to the fields jouet reads. Of GitLab's, the MR, its approvals and
//! its commits are recorded from gitlab.com (gitlab-org/cli!4266); gitlab.com
//! answers discussions, commit statuses, versions and job traces only with a
//! token, so those follow the shapes in GitLab's API documentation. The MR is
//! served under a nested group path, `group/sub/project`, so nothing may split
//! a repository at its first `/`.

mod common;

use common::{Fake, Request, json, query, serve, stderr, stdout};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

const GL_MR: &str = include_str!("fixtures/forges/gitlab/mr.json");
const GL_APPROVALS: &str = include_str!("fixtures/forges/gitlab/approvals.json");
const GL_COMMITS: &str = include_str!("fixtures/forges/gitlab/commits.json");
const GL_DISCUSSIONS: &str = include_str!("fixtures/forges/gitlab/discussions.json");
const GL_STATUSES: &str = include_str!("fixtures/forges/gitlab/statuses.json");
const GL_VERSIONS: &str = include_str!("fixtures/forges/gitlab/versions.json");
const GL_TRACE: &str = include_str!("fixtures/forges/gitlab/trace.txt");

const FJ_PR: &str = include_str!("fixtures/forges/forgejo/pr.json");
const FJ_REVIEWS: &str = include_str!("fixtures/forges/forgejo/reviews.json");
const FJ_REVIEW_COMMENTS: &str = include_str!("fixtures/forges/forgejo/review_comments.json");
const FJ_STATUS: &str = include_str!("fixtures/forges/forgejo/status.json");
const FJ_COMMITS: &str = include_str!("fixtures/forges/forgejo/commits.json");
const FJ_COMMENTS: &str = include_str!("fixtures/forges/forgejo/comments.json");

const GOOD: &str = "good-token";
const PROJECT: &str = "/projects/group%2Fsub%2Fproject";
const MR: &str = "gitlab.com/group/sub/project!4266";
const PR: &str = "codeberg.org/forgejo/forgejo#15077";
/// The discussion of the unresolved GitLab thread, whose first note is 1126.
const DISCUSSION: &str = "6a9c1750b37d513a43987b574953fceb50b03ce7";

/// Runs jouet in `dir` (no git checkout) with the fake as both forges' API,
/// no token but those given, and a home of its own, so neither the
/// developer's tokens nor their glab and tea configs are read.
fn jouet(fake: &Fake, dir: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_jouet"));
    cmd.args(args)
        .current_dir(dir)
        .env("JOUET_GITLAB_API_URL", &fake.url)
        .env("JOUET_FORGEJO_API_URL", &fake.url)
        .env("JOUET_GITHUB_API_URL", "http://127.0.0.1:9")
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("JOUET_LOG_DIR", dir.join("logs"))
        .env("JOUET_REPLY_FOOTER", "")
        .env("PATH", dir.join("empty"));
    for var in [
        "JOUET_GITHUB_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "JOUET_GITLAB_TOKEN",
        "GITLAB_TOKEN",
        "GITLAB_ACCESS_TOKEN",
        "GLAB_CONFIG_DIR",
        "JOUET_FORGEJO_TOKEN",
        "FORGEJO_TOKEN",
        "GITEA_TOKEN",
        "JOUET_DISPLAY",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    for (name, value) in env {
        cmd.env(name, value);
    }
    cmd.output().expect("jouet should start")
}

fn ok(body: &str) -> (u16, String) {
    (200, body.to_string())
}

fn missing() -> (u16, String) {
    (404, r#"{"message":"404 Not Found"}"#.into())
}

/// The fake GitLab: the recorded MR under `group/sub/project`.
fn gitlab(request: &Request, _: usize) -> (u16, String) {
    if request.token != GOOD {
        return (401, r#"{"message":"401 Unauthorized"}"#.into());
    }
    if request.path == "/user" {
        return ok(r#"{"username":"author"}"#);
    }
    let Some(rest) = request.path.strip_prefix(PROJECT) else {
        return missing();
    };
    let notes = format!("/merge_requests/4266/discussions/{DISCUSSION}/notes");
    let resolve = format!("/merge_requests/4266/discussions/{DISCUSSION}?resolved=true");
    match (request.method.as_str(), rest) {
        ("GET", "/merge_requests/4266?with_merge_status_recheck=true") => ok(GL_MR),
        ("GET", "/merge_requests/4266/approvals") => ok(GL_APPROVALS),
        ("GET", "/merge_requests/4266/discussions?per_page=100") => ok(GL_DISCUSSIONS),
        ("GET", r) if r.starts_with("/merge_requests/4266/versions") => ok(GL_VERSIONS),
        ("GET", r) if r.starts_with("/merge_requests/4266/commits") => ok(GL_COMMITS),
        ("GET", r) if r.starts_with("/repository/commits/") && r.contains("/statuses") => {
            ok(GL_STATUSES)
        }
        ("GET", r)
            if r.starts_with("/merge_requests?source_branch=8698-validate-oauth-client-id") =>
        {
            ok(r#"[{"iid":4266}]"#)
        }
        ("GET", "/jobs/9001/trace") => ok(GL_TRACE),
        ("GET", "/pipelines/3011158147") => ok(&json!({
            "id": 3011158147u64, "status": "failed", "ref": "8698-validate-oauth-client-id",
            "sha": "5b1cfff77c89ad3e77bbac0235d0b51ada2c79cf"
        })
        .to_string()),
        ("GET", "/pipelines/3011158147/jobs?per_page=100") => {
            let statuses: Vec<Value> = serde_json::from_str(GL_STATUSES).unwrap();
            let jobs: Vec<Value> = statuses
                .into_iter()
                .filter(|s| s["pipeline_id"].is_u64())
                .map(|mut s| {
                    s["web_url"] = s["target_url"].take();
                    s["stage"] = "test".into();
                    s
                })
                .collect();
            ok(&Value::Array(jobs).to_string())
        }
        ("POST", r) if r == notes => ok(r#"{"id":1150,"body":"Fixed in the next push"}"#),
        ("PUT", r) if r == resolve => {
            let mut discussions: Vec<Value> = serde_json::from_str(GL_DISCUSSIONS).unwrap();
            let mut d = discussions.remove(0);
            for n in d["notes"].as_array_mut().unwrap() {
                n["resolved"] = true.into();
            }
            ok(&d.to_string())
        }
        _ => missing(),
    }
}

/// What the fake Forgejo says beyond the recordings.
#[derive(Clone, Copy)]
struct Forgejo {
    /// The recorded PR conflicts; tests of its checks need it not to.
    mergeable: bool,
    version: &'static str,
}

fn forgejo(world: Forgejo) -> impl Fn(&Request, usize) -> (u16, String) {
    move |request, _| {
        if request.token != GOOD {
            return (401, r#"{"message":"token is required"}"#.into());
        }
        let base = "/repos/forgejo/forgejo";
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/user") => return ok(r#"{"login":"ApolloFops"}"#),
            ("GET", "/version") => return ok(&json!({ "version": world.version }).to_string()),
            _ => {}
        }
        let Some(rest) = request.path.strip_prefix(base) else {
            return missing();
        };
        match (request.method.as_str(), rest) {
            ("GET", "/pulls/15077") => {
                let mut pr: Value = serde_json::from_str(FJ_PR).unwrap();
                pr["mergeable"] = world.mergeable.into();
                ok(&pr.to_string())
            }
            ("GET", "/pulls/15077/reviews?limit=50") => ok(FJ_REVIEWS),
            ("GET", "/pulls/15077/reviews/2267390/comments") => ok(FJ_REVIEW_COMMENTS),
            ("GET", "/pulls/15077/commits?limit=50") => ok(FJ_COMMITS),
            ("GET", "/issues/15077/comments?limit=50") => ok(FJ_COMMENTS),
            ("GET", r) if r.starts_with("/commits/") && r.contains("/status?") => ok(FJ_STATUS),
            ("POST", "/pulls/15077/reviews") => ok(
                r#"{"id":2270000,"html_url":"https://codeberg.org/forgejo/forgejo/pulls/15077#issuecomment-27000000"}"#,
            ),
            ("POST", "/pulls/comments/26828843/resolve") if world.version.starts_with("1.") => {
                (204, String::new())
            }
            _ => missing(),
        }
    }
}

const FORGEJO_16: Forgejo = Forgejo {
    mergeable: false,
    version: "16.0.0-dev-798-568c49e1+gitea-1.22.0",
};

fn tmp() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("empty")).unwrap();
    dir
}

fn codes(pr: &Value) -> Vec<&str> {
    pr["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["code"].as_str().unwrap())
        .collect()
}

#[test]
fn a_gitlab_mr_in_a_nested_group_reports_like_a_pr() {
    let fake = serve(gitlab);
    let dir = tmp();
    let env = [("GITLAB_TOKEN", GOOD)];
    for arg in [
        MR,
        "https://gitlab.com/group/sub/project/-/merge_requests/4266",
    ] {
        let out = jouet(
            &fake,
            dir.path(),
            &env,
            &["pr", "status", arg, "--settle", "0", "--json"],
        );
        assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
        let pr = &json(&out)["prs"][0];
        assert_eq!(pr["repo"], "group/sub/project");
        assert_eq!(pr["forge"], "gitlab");
        assert_eq!(pr["host"], "gitlab.com");
        assert_eq!(pr["number"], 4266);
        assert_eq!(pr["state"], "open");
        assert_eq!(pr["head"], "8698-validate-oauth-client-id");
        assert_eq!(pr["merge_state"], "blocked");
        assert_eq!(pr["mergeable"], "mergeable");
        assert_eq!(pr["review"], "review_required");
        assert_eq!(pr["unresolved_threads"], 1);
        assert_eq!(pr["pushed_at"], "2026-10-30T11:00:00Z");
        // The job allowed to fail counts as skipped, as GitLab shows it.
        assert_eq!(
            pr["checks"],
            json!({ "pass": 2, "fail": 1, "skipped": 1, "failing": ["test"] })
        );
        assert_eq!(codes(pr), ["checks_failing", "unresolved_threads"]);
    }
    let out = jouet(
        &fake,
        dir.path(),
        &env,
        &[
            "pr",
            "status",
            "4266",
            "--repo",
            "gitlab.com/group/sub/project",
            "--settle",
            "0",
            "--display",
            "plain",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("#4266 open "), "{text}");
    assert!(text.contains("threads 1  review required"), "{text}");
    // REST only, the token sent as a bearer token and never into the output.
    assert!(
        fake.requests()
            .iter()
            .all(|r| r.token == GOOD && query(r).is_empty())
    );
    assert!(!text.contains(GOOD) && !stderr(&out).contains(GOOD));
}

#[test]
fn pr_next_names_the_mr_with_its_host_and_the_forges_own_cli() {
    let fake = serve(gitlab);
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITLAB_TOKEN", GOOD)],
        &["pr", "next", MR, "--settle", "0", "--json"],
    );
    assert_eq!(out.status.code(), Some(10), "{}", stderr(&out));
    let pr = &json(&out)["prs"][0];
    assert_eq!(pr["action"], "checks_failing");
    assert_eq!(
        pr["command"],
        "jouet wait ci gitlab.com/group/sub/project#4266"
    );

    let fake = serve(forgejo(FORGEJO_16));
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &["pr", "next", PR, "--settle", "0", "--json"],
    );
    assert_eq!(out.status.code(), Some(10), "{}", stderr(&out));
    let pr = &json(&out)["prs"][0];
    // A Forgejo draft is a `WIP:` title: no command clears it.
    assert_eq!(pr["action"], "draft");
    assert!(pr.get("command").is_none(), "{pr}");
}

#[test]
fn a_gitlab_branch_finds_its_mr() {
    let fake = serve(gitlab);
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITLAB_TOKEN", GOOD)],
        &[
            "pr",
            "status",
            "8698-validate-oauth-client-id",
            "--repo",
            "gitlab.com/group/sub/project",
            "--settle",
            "0",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["prs"][0]["number"], 4266);
}

#[test]
fn gitlab_discussions_are_threads() {
    let fake = serve(gitlab);
    let dir = tmp();
    let env = [("GITLAB_TOKEN", GOOD)];
    let out = jouet(&fake, dir.path(), &env, &["pr", "threads", MR, "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["repo"], "group/sub/project");
    assert_eq!(
        doc["counts"],
        json!({ "unresolved": 1, "resolved": 1, "outdated": 0 })
    );
    let threads = doc["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 1);
    let t = &threads[0];
    assert_eq!(t["id"], 1126);
    assert_eq!(t["path"], "internal/oauth/flow.go");
    assert_eq!(t["line"], 42);
    assert_eq!(t["comments"][1]["author"], "author");
    assert_eq!(t["comments"][0]["created_at"], "2026-10-30T12:10:00Z");

    let out = jouet(
        &fake,
        dir.path(),
        &env,
        &["pr", "threads", MR, "--all", "--include-reviews", "--json"],
    );
    let doc = json(&out);
    let all = doc["threads"].as_array().unwrap();
    let resolved = all.iter().find(|t| t["id"] == 1130).unwrap();
    // On the base's side, and from an older version of the MR.
    assert_eq!(resolved["side"], "left");
    assert_eq!(resolved["outdated"], true);
    assert_eq!(resolved["original_line"], 10);
    assert!(resolved.get("line").is_none());
    let comment = all.iter().find(|t| t["id"] == 1140).unwrap();
    assert_eq!(comment["kind"], "comment");
    // System notes ("added 1 commit") are not threads.
    assert!(all.iter().all(|t| t["id"] != 1135));
}

#[test]
fn a_gitlab_reply_goes_into_the_discussion_and_resolves_it() {
    let fake = serve(gitlab);
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITLAB_TOKEN", GOOD)],
        &[
            "pr",
            "threads",
            "reply",
            "1126",
            MR,
            "--body",
            "Fixed in the next push",
            "--resolve",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let result = &json(&out)["results"][0];
    assert_eq!(result["replied"], true);
    assert_eq!(result["resolved"], true);
    assert_eq!(
        result["url"],
        "https://gitlab.com/gitlab-org/cli/-/merge_requests/4266#note_1150"
    );
    let writes: Vec<Request> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method != "GET")
        .collect();
    assert_eq!(writes.len(), 2);
    assert_eq!(writes[0].method, "POST");
    assert_eq!(writes[0].body, json!({ "body": "Fixed in the next push" }));
    assert_eq!(writes[1].method, "PUT");
    assert!(
        writes[1].path.ends_with("?resolved=true"),
        "{}",
        writes[1].path
    );
}

#[test]
fn wait_ci_on_gitlab_fails_fast_with_the_jobs_log() {
    let fake = serve(gitlab);
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITLAB_TOKEN", GOOD)],
        &["wait", "ci", MR, "--timeout", "30s", "--json"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["repo"], "group/sub/project");
    assert_eq!(doc["result"], "failure");
    let checks = doc["checks"].as_array().unwrap();
    let test = &checks[0];
    assert_eq!(test["name"], "test");
    assert_eq!(test["state"], "fail");
    assert_eq!(test["required"], true);
    let excerpt = test["log_excerpt"].as_array().unwrap();
    let excerpt: Vec<&str> = excerpt.iter().map(|l| l.as_str().unwrap()).collect();
    assert!(
        excerpt.contains(&"--- FAIL: TestValidateClientID (0.00s)"),
        "{excerpt:?}"
    );
    assert!(
        excerpt
            .iter()
            .all(|l| !l.contains("section_") && !l.is_empty()),
        "{excerpt:?}"
    );
    let lint = checks.iter().find(|c| c["name"] == "lint").unwrap();
    assert_eq!(lint["state"], "skipped");
    // The log is kept under the repository's host-qualified name.
    assert!(
        dir.path()
            .join("logs/gitlab.com-group-sub-project-job-9001.log")
            .exists()
    );
}

#[test]
fn wait_run_follows_a_gitlab_pipeline() {
    let fake = serve(gitlab);
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITLAB_TOKEN", GOOD)],
        &[
            "wait",
            "run",
            "https://gitlab.com/group/sub/project/-/pipelines/3011158147",
            "--no-logs",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["run_id"], 3011158147u64);
    assert_eq!(doc["result"], "failure");
    assert_eq!(doc["counts"]["fail"], 1);
}

#[test]
fn a_rejected_gitlab_token_is_named_by_its_source_never_its_value() {
    let fake = serve(gitlab);
    let dir = tmp();
    let secret = "glpat-wrong-secret";
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITLAB_TOKEN", secret)],
        &["pr", "status", MR, "--settle", "0", "--json"],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stderr(&out));
    let message = json(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains("GITLAB_TOKEN"), "{message}");
    assert!(!stdout(&out).contains(secret) && !stderr(&out).contains(secret));
}

#[test]
fn glabs_config_supplies_the_token_and_a_config_names_a_self_hosted_gitlab() {
    let fake = serve(gitlab);
    let dir = tmp();
    let glab = dir.path().join("config/glab-cli");
    std::fs::create_dir_all(&glab).unwrap();
    std::fs::write(
        glab.join("config.yml"),
        format!("hosts:\n    git.example.com:\n        token: {GOOD}\n"),
    )
    .unwrap();
    let jouet_config = dir.path().join("config/jouet");
    std::fs::create_dir_all(&jouet_config).unwrap();
    std::fs::write(
        jouet_config.join("config.toml"),
        "[forges.\"git.example.com\"]\nkind = \"gitlab\"\n",
    )
    .unwrap();
    let out = jouet(
        &fake,
        dir.path(),
        &[],
        &[
            "pr",
            "status",
            "git.example.com/group/sub/project!4266",
            "--settle",
            "0",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(json(&out)["prs"][0]["number"], 4266);
    assert!(fake.requests().iter().all(|r| r.token == GOOD));
}

#[test]
fn a_token_goes_only_to_the_host_it_belongs_to() {
    let dir = tmp();
    // GITEA_TOKEN is for gitea.com: codeberg.org never sees it.
    let fake = serve(forgejo(FORGEJO_16));
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITEA_TOKEN", GOOD)],
        &["pr", "status", PR, "--settle", "0", "--json"],
    );
    assert_eq!(out.status.code(), Some(5), "{}", stdout(&out));
    assert!(fake.requests().iter().all(|r| r.token.is_empty()));
    let message = json(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(
        message.contains("codeberg.org needs a token: set JOUET_FORGEJO_TOKEN or FORGEJO_TOKEN"),
        "{message}"
    );

    // GITLAB_TOKEN is for gitlab.com; a self-hosted GitLab takes the
    // variable its table names.
    let jouet_config = dir.path().join("config/jouet");
    std::fs::create_dir_all(&jouet_config).unwrap();
    std::fs::write(
        jouet_config.join("config.toml"),
        "[forges.\"git.example.com\"]\nkind = \"gitlab\"\ntoken_env = \"CORP_GITLAB_TOKEN\"\n",
    )
    .unwrap();
    let fake = serve(gitlab);
    let mr = "git.example.com/group/sub/project!4266";
    let args = ["pr", "status", mr, "--settle", "0", "--json"];
    let out = jouet(&fake, dir.path(), &[("GITLAB_TOKEN", GOOD)], &args);
    assert_eq!(out.status.code(), Some(5), "{}", stdout(&out));
    assert!(fake.requests().iter().all(|r| r.token.is_empty()));
    let fake = serve(gitlab);
    let out = jouet(&fake, dir.path(), &[("CORP_GITLAB_TOKEN", GOOD)], &args);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(fake.requests().iter().all(|r| r.token == GOOD));
}

#[test]
fn wait_ci_required_on_forgejo_is_refused() {
    let fake = serve(forgejo(FORGEJO_16));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &["wait", "ci", PR, "--required", "--json"],
    );
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    assert!(fake.requests().is_empty());
}

#[test]
fn a_forgejo_pr_reports_like_a_github_one() {
    let fake = serve(forgejo(FORGEJO_16));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &["pr", "status", PR, "--settle", "0", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let pr = &json(&out)["prs"][0];
    assert_eq!(pr["repo"], "forgejo/forgejo");
    assert_eq!(pr["state"], "draft");
    assert_eq!(pr["mergeable"], "conflicting");
    assert_eq!(pr["unresolved_threads"], 1);
    // The head commit's date, sent as -04:00, in UTC.
    assert_eq!(pr["pushed_at"], "2026-10-29T18:05:03Z");
    assert_eq!(pr["checks"]["fail"], 2);
    assert_eq!(pr["checks"]["skipped"], 12);
    assert_eq!(
        codes(pr),
        ["conflict", "checks_failing", "unresolved_threads", "draft"]
    );
    assert!(
        pr["attention"][0]["message"]
            .as_str()
            .unwrap()
            .ends_with("it cannot merge until it is rebased")
    );
}

#[test]
fn forgejo_review_comments_on_one_line_are_one_thread() {
    let fake = serve(forgejo(FORGEJO_16));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &["pr", "threads", PR, "--all", "--json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(
        doc["counts"],
        json!({ "unresolved": 1, "resolved": 1, "outdated": 0 })
    );
    let threads = doc["threads"].as_array().unwrap();
    let ids: Vec<u64> = threads.iter().map(|t| t["id"].as_u64().unwrap()).collect();
    assert_eq!(ids, [26828831, 26828843]);
    assert_eq!(threads[0]["resolved"], true);
    assert_eq!(threads[0]["comments"].as_array().unwrap().len(), 6);
    assert_eq!(threads[1]["path"], "routers/web/auth/oauth.go");
    assert_eq!(threads[1]["line"], 1672);
    assert_eq!(
        threads[1]["comments"][0]["created_at"],
        "2026-10-30T08:21:42Z"
    );
}

#[test]
fn forgejo_cannot_resolve_so_nothing_is_posted() {
    let fake = serve(forgejo(FORGEJO_16));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &[
            "pr",
            "threads",
            "reply",
            "26828843",
            PR,
            "--body",
            "Done",
            "--resolve",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    let message = json(&out)["error"]["message"].as_str().unwrap().to_string();
    assert!(message.contains("Forgejo 16.0.0"), "{message}");
    assert!(message.ends_with("nothing was posted"), "{message}");
    assert!(fake.requests().iter().all(|r| r.method == "GET"));
}

#[test]
fn gitea_resolves_and_forgejo_replies_on_the_threads_line() {
    let gitea = Forgejo {
        mergeable: true,
        version: "1.26.0",
    };
    let fake = serve(forgejo(gitea));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("GITEA_TOKEN", GOOD)],
        &[
            "pr",
            "threads",
            "reply",
            "26828843",
            "gitea.com/forgejo/forgejo#15077",
            "--body",
            "Done",
            "--resolve",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let result = &json(&out)["results"][0];
    assert_eq!(result["replied"], true);
    assert_eq!(result["resolved"], true);
    let writes: Vec<Request> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST")
        .collect();
    assert_eq!(writes[0].path, "/repos/forgejo/forgejo/pulls/15077/reviews");
    assert_eq!(writes[0].body["event"], "COMMENT");
    assert_eq!(
        writes[0].body["comments"][0],
        json!({ "path": "routers/web/auth/oauth.go", "body": "Done", "new_position": 1672 })
    );
    assert_eq!(
        writes[1].path,
        "/repos/forgejo/forgejo/pulls/comments/26828843/resolve"
    );
}

#[test]
fn wait_ci_on_forgejo_reads_the_commit_statuses() {
    let checks_only = Forgejo {
        mergeable: true,
        version: FORGEJO_16.version,
    };
    let fake = serve(forgejo(checks_only));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &["wait", "ci", PR, "--timeout", "30s", "--json"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["result"], "failure");
    assert_eq!(doc["counts"]["fail"], 2);
    let failed = &doc["checks"][0];
    assert_eq!(failed["state"], "fail");
    // Forgejo Actions links a job by path; jouet gives the whole URL.
    assert!(
        failed["url"]
            .as_str()
            .unwrap()
            .starts_with("https://codeberg.org/forgejo/forgejo/actions/runs/"),
        "{failed}"
    );
}

#[test]
fn wait_run_on_forgejo_is_refused() {
    let fake = serve(forgejo(FORGEJO_16));
    let dir = tmp();
    let out = jouet(
        &fake,
        dir.path(),
        &[("FORGEJO_TOKEN", GOOD)],
        &[
            "wait",
            "run",
            "7",
            "--repo",
            "codeberg.org/forgejo/forgejo",
            "--json",
        ],
    );
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    assert!(fake.requests().is_empty());
}
