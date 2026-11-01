//! The GitHub writes behind `jouet pr land`: updating a branch with its
//! base, merging, and confirming the merge.
//!
//! Both writes go through the REST endpoints that take the head SHA as a
//! guard, so a push between the check and the write makes GitHub refuse
//! rather than merge something nobody looked at. Neither ever force-pushes:
//! updating merges the base into the branch on GitHub. Merging asks for the
//! same merge a person gets from the button, so branch protection applies
//! and nothing here asks to bypass it.
//!
//! A PR in one of GitHub's native stacks cannot go through the merge
//! endpoint: GitHub only merges it through the asynchronous one, which
//! takes the same head SHA guard, merges in the background, and is polled
//! for its result. It merges every PR below the one named as well, so
//! callers use it only for the bottom PR of a stack.

use super::target::Repo;
use crate::{Client, Failure, Rest};
use serde_json::{Value, json};

/// How the merge is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Squash,
    Merge,
    Rebase,
}

impl Method {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "squash" => Some(Self::Squash),
            "merge" => Some(Self::Merge),
            "rebase" => Some(Self::Rebase),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Squash => "squash",
            Self::Merge => "merge",
            Self::Rebase => "rebase",
        }
    }
}

/// Why a write did not happen.
#[derive(Debug)]
pub enum Refused {
    /// GitHub refused it: branch protection, a head that moved, a method
    /// the repository does not allow, or missing permission. Its message.
    Rejected(String),
    NotFound(String),
    /// The request failed: network, rate limit, or a 5xx.
    Failed(Failure),
}

impl From<Failure> for Refused {
    fn from(f: Failure) -> Self {
        Self::Failed(f)
    }
}

fn pulls(repo: &Repo, number: u64) -> String {
    format!("/repos/{}/{}/pulls/{number}", repo.owner, repo.name)
}

/// Asks GitHub to merge the base into the branch, if the head is still
/// `head_oid`. GitHub does this in the background, so the new head appears
/// a moment after this returns.
pub fn update_branch(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    head_oid: &str,
) -> Result<(), Refused> {
    let path = format!("{}/update-branch", pulls(repo, number));
    let resp = client.put(&path, &json!({ "expected_head_sha": head_oid }))?;
    match resp.status {
        200..=299 => Ok(()),
        _ => Err(refusal(repo, number, &resp)),
    }
}

/// Merges the PR if its head is still `head_oid`, and returns the merge
/// commit GitHub reports.
pub fn merge(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    head_oid: &str,
    method: Method,
) -> Result<Option<String>, Refused> {
    let path = format!("{}/merge", pulls(repo, number));
    let body = json!({ "merge_method": method.as_str(), "sha": head_oid });
    let resp = client.put(&path, &body)?;
    match resp.status {
        200 => Ok(read(&resp)?["sha"].as_str().map(str::to_string)),
        _ => Err(refusal(repo, number, &resp)),
    }
}

/// Asks GitHub to merge a stacked PR in the background through the
/// asynchronous merge endpoint, if its head is still `head_oid`. Like the
/// merge endpoint, it applies branch protection; `direct_merge` keeps it from
/// queueing the PR instead. It merges the PRs below this one too.
pub fn merge_async(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    head_oid: &str,
    method: Method,
) -> Result<Async, Refused> {
    let path = format!("{}/merge-async", pulls(repo, number));
    let body = json!({
        "merge_method": method.as_str(),
        "merge_action": "direct_merge",
        "sha": head_oid,
    });
    let resp = client.put(&path, &body)?;
    match resp.status {
        200 | 202 => Ok(Async::read(&read(&resp)?)),
        // Not mergeable, such as closed or a draft.
        400 => Err(Refused::Rejected(Async::message(&resp))),
        409 => Err(Refused::Rejected(format!(
            "GitHub already has a merge request for it, which may use other options: {}",
            Async::message(&resp)
        ))),
        404 => Err(Refused::Rejected(format!(
            "GitHub's asynchronous merge, which a stacked PR needs, is not available for {repo}"
        ))),
        _ => Err(refusal(repo, number, &resp)),
    }
}

/// Where an asynchronous merge GitHub accepted as `uuid` stands now.
pub fn merge_async_result(
    client: &mut Client,
    repo: &Repo,
    number: u64,
    uuid: &str,
) -> Result<Async, Refused> {
    let path = format!("{}/merge-async/{uuid}", pulls(repo, number));
    let resp = client.get(&path, None)?;
    match resp.status {
        200 => Ok(Async::read(&read(&resp)?)),
        _ => Err(refusal(repo, number, &resp)),
    }
}

/// Where an asynchronous merge stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Async {
    /// Running in the background; poll it by this id.
    Pending(String),
    /// Merged, with the merge commit GitHub reports.
    Merged(Option<String>),
    /// Added to the base branch's merge queue instead.
    Enqueued,
    /// Not merged (a conflict, an unmet branch rule), and why. The merge is
    /// atomic, so nothing in the stack was merged.
    Failed(String),
}

impl Async {
    fn read(v: &Value) -> Self {
        let details = &v["details"];
        let text = |key: &str| details[key].as_str().map(str::to_string);
        match v["status"].as_str() {
            Some("pending") => Self::Pending(text("uuid").unwrap_or_default()),
            Some("merged") => Self::Merged(text("sha")),
            Some("enqueued") => Self::Enqueued,
            _ => Self::Failed(text("message").unwrap_or_else(|| v.to_string())),
        }
    }

    /// The message of a refused request, which comes either as the result
    /// object or as GitHub's usual error.
    fn message(resp: &Rest) -> String {
        let v = serde_json::from_str::<Value>(&resp.body).unwrap_or_default();
        v["details"]["message"]
            .as_str()
            .or(v["message"].as_str())
            .map_or_else(|| resp.body.trim().to_string(), str::to_string)
    }
}

/// The PR as the REST API reads it: whether it is merged, its merge commit,
/// its base, and the stack it is in.
pub fn pull(client: &mut Client, repo: &Repo, number: u64) -> Result<Pull, Refused> {
    let resp = client.get(&pulls(repo, number), None)?;
    if resp.status != 200 {
        return Err(refusal(repo, number, &resp));
    }
    let pr = read(&resp)?;
    let stack = &pr["stack"];
    Ok(Pull {
        merged: pr["merged"].as_bool().unwrap_or(false),
        merge_commit: pr["merge_commit_sha"].as_str().map(str::to_string),
        base: pr["base"]["ref"].as_str().unwrap_or_default().to_string(),
        stack: stack["number"].as_u64().map(|number| Stack {
            number,
            base: stack["base"]["ref"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    pub merged: bool,
    pub merge_commit: Option<String>,
    /// The branch the PR targets.
    pub base: String,
    pub stack: Option<Stack>,
}

/// A GitHub native stack a PR is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub number: u64,
    /// The branch the whole stack targets. Only the bottom open PR targets
    /// it directly; the others target the branch of the PR below them.
    pub base: String,
}

fn read(resp: &Rest) -> Result<Value, Failure> {
    serde_json::from_str(&resp.body)
        .map_err(|e| Failure::Other(format!("GitHub sent a response jouet could not read: {e}")))
}

/// A response other than success, as GitHub explained it. A 5xx is a
/// failure, not a refusal, since the write may still have happened.
fn refusal(repo: &Repo, number: u64, resp: &Rest) -> Refused {
    let message = serde_json::from_str::<Value>(&resp.body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| resp.body.trim().to_string());
    match resp.status {
        404 => Refused::NotFound(format!("{repo}#{number} does not exist")),
        403 | 405 | 409 | 422 => Refused::Rejected(message),
        status => Refused::Failed(Failure::Other(format!(
            "GitHub returned {status}: {message}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_are_the_three_github_allows() {
        for m in ["squash", "merge", "rebase"] {
            assert_eq!(Method::parse(m).map(Method::as_str), Some(m));
        }
        assert_eq!(Method::parse("fast-forward"), None);
    }

    #[test]
    fn rejections_carry_githubs_message() {
        let repo = Repo::parse("jdx/demo").unwrap();
        let resp = |status, body: &str| Rest {
            status,
            etag: None,
            link: None,
            body: body.into(),
        };
        let protected = resp(
            405,
            r#"{"message":"At least 1 approving review is required by reviewers with write access."}"#,
        );
        assert!(matches!(
            refusal(&repo, 7, &protected),
            Refused::Rejected(m) if m.starts_with("At least 1 approving review")
        ));
        assert!(matches!(
            refusal(&repo, 7, &resp(404, r#"{"message":"Not Found"}"#)),
            Refused::NotFound(m) if m == "jdx/demo#7 does not exist"
        ));
        assert!(matches!(
            refusal(&repo, 7, &resp(502, "bad gateway")),
            Refused::Failed(Failure::Other(m)) if m == "GitHub returned 502: bad gateway"
        ));
    }
}
