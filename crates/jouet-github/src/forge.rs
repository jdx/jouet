//! Which forge a host runs, where its API is, and which tokens it takes.
//!
//! github.com, gitlab.com, codeberg.org and gitea.com are known. A
//! self-hosted GitLab or Forgejo is named in jouet's config, which the
//! binary hands over with [`configure`] and which is read only when a host
//! jouet does not know comes up:
//!
//! ```toml
//! [forges."git.example.com"]
//! kind = "gitlab"                                  # or "forgejo" ("gitea" is the same API)
//! api_url = "https://git.example.com/api/v4"       # optional
//! ```
//!
//! `JOUET_GITLAB_API_URL` and `JOUET_FORGEJO_API_URL` point every host of
//! that kind at one API root, as `JOUET_GITHUB_API_URL` does for GitHub, for
//! a test's fake server.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Forge {
    GitHub,
    GitLab,
    /// Forgejo and Gitea, which share one API.
    Forgejo,
}

impl Forge {
    /// Parses a config `kind`.
    pub fn parse(kind: &str) -> Option<Self> {
        match kind.to_ascii_lowercase().as_str() {
            "github" => Some(Self::GitHub),
            "gitlab" => Some(Self::GitLab),
            "forgejo" | "gitea" => Some(Self::Forgejo),
            _ => None,
        }
    }

    /// The forge's name in messages.
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Forgejo => "Forgejo",
        }
    }

    /// The environment variables a token is read from, in order.
    pub(crate) fn token_vars(self) -> &'static [&'static str] {
        match self {
            Self::GitHub => &["JOUET_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"],
            Self::GitLab => &["JOUET_GITLAB_TOKEN", "GITLAB_TOKEN", "GITLAB_ACCESS_TOKEN"],
            Self::Forgejo => &["JOUET_FORGEJO_TOKEN", "FORGEJO_TOKEN", "GITEA_TOKEN"],
        }
    }

    fn api_url_var(self) -> &'static str {
        match self {
            Self::GitHub => "JOUET_GITHUB_API_URL",
            Self::GitLab => "JOUET_GITLAB_API_URL",
            Self::Forgejo => "JOUET_FORGEJO_API_URL",
        }
    }
}

/// A host named in the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    pub host: String,
    pub forge: Forge,
    pub api_url: Option<String>,
}

static LOADER: OnceLock<fn() -> Vec<Configured>> = OnceLock::new();
static CONFIGURED: OnceLock<Vec<Configured>> = OnceLock::new();

/// Sets what reads the hosts the config names. Called once, before any
/// repository is read; a second call is ignored.
pub fn configure(load: fn() -> Vec<Configured>) {
    let _ = LOADER.set(load);
}

fn configured(host: &str) -> Option<&'static Configured> {
    let hosts = CONFIGURED.get_or_init(|| {
        let load = LOADER.get().map_or_else(Vec::new, |load| load());
        load.into_iter()
            .map(|mut h| {
                h.host = normalize(&h.host);
                h
            })
            .collect()
    });
    hosts.iter().find(|c| c.host == host)
}

/// A host as jouet keys it: lowercase, without `www.`.
pub(crate) fn normalize(host: &str) -> String {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host
        .strip_prefix("www.")
        .map_or(host.clone(), str::to_string);
    // The SSH-over-443 hosts serve the same repositories.
    match host.as_str() {
        "ssh.github.com" => "github.com".into(),
        "altssh.gitlab.com" => "gitlab.com".into(),
        _ => host,
    }
}

/// The forge a host runs, if jouet knows it.
pub fn of_host(host: &str) -> Option<Forge> {
    let host = normalize(host);
    match host.as_str() {
        "github.com" => Some(Forge::GitHub),
        "gitlab.com" => Some(Forge::GitLab),
        "codeberg.org" | "gitea.com" => Some(Forge::Forgejo),
        _ => configured(&host).map(|c| c.forge),
    }
}

/// Why a host cannot be used, for a host jouet does not know.
pub fn unknown(host: &str) -> String {
    format!(
        "jouet does not know which forge {host} runs; name it in jouet's config: \
         [forges.\"{host}\"] kind = \"gitlab\" (or \"forgejo\")"
    )
}

/// The API root of a GitLab or Forgejo host: the forge's environment
/// variable, else the config's `api_url`, else the forge's usual path.
pub(crate) fn api_url(forge: Forge, host: &str) -> String {
    if let Some(url) = std::env::var(forge.api_url_var())
        .ok()
        .filter(|u| !u.is_empty())
    {
        return url.trim_end_matches('/').to_string();
    }
    if let Some(url) = configured(host).and_then(|c| c.api_url.clone()) {
        return url.trim_end_matches('/').to_string();
    }
    match forge {
        Forge::GitHub => "https://api.github.com".into(),
        Forge::GitLab => format!("https://{host}/api/v4"),
        Forge::Forgejo => format!("https://{host}/api/v1"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_need_no_config() {
        assert_eq!(of_host("github.com"), Some(Forge::GitHub));
        assert_eq!(of_host("GitLab.com"), Some(Forge::GitLab));
        assert_eq!(of_host("www.codeberg.org"), Some(Forge::Forgejo));
        assert_eq!(of_host("git.example.com"), None);
    }

    #[test]
    fn kinds_parse_with_gitea_as_forgejo() {
        assert_eq!(Forge::parse("GitLab"), Some(Forge::GitLab));
        assert_eq!(Forge::parse("gitea"), Some(Forge::Forgejo));
        assert_eq!(Forge::parse("bitbucket"), None);
    }
}
