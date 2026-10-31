//! The registries' and OSV's HTTP client: jouet's TLS, a timeout on every
//! request, one retry after a dropped connection, a 5xx or a 429.

use crate::docs::Lang;
use std::time::Duration;

/// The longest one request may take, connecting included.
const TIMEOUT: Duration = Duration::from_secs(15);
/// A response larger than this is refused: a registry document is far
/// smaller.
const MAX_BODY: u64 = 64 << 20;

/// A service `jouet deps` asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Service {
    Registry(Lang),
    Osv,
}

impl Service {
    fn root(self) -> &'static str {
        match self {
            Service::Registry(Lang::Rust) => "https://crates.io",
            Service::Registry(Lang::Ts) => "https://registry.npmjs.org",
            Service::Registry(Lang::Python) => "https://pypi.org",
            Service::Registry(Lang::Go) => "https://proxy.golang.org",
            Service::Osv => "https://api.osv.dev",
        }
    }

    /// Under `JOUET_DEPS_API_URL`, where a test's fake server answers for
    /// every service.
    fn key(self) -> &'static str {
        match self {
            Service::Registry(lang) => super::ecosystem(lang),
            Service::Osv => "osv",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Service::Registry(lang) => super::registry_name(lang),
            Service::Osv => "OSV",
        }
    }
}

/// A response: its status and body.
pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) body: String,
}

#[derive(Clone)]
pub(crate) struct Http {
    agent: ureq::Agent,
    /// `JOUET_DEPS_API_URL`: one root for every service, `<root>/cargo`,
    /// `<root>/osv` and so on.
    fake: Option<String>,
    /// Ignore cached answers (and replace them).
    pub(crate) refresh: bool,
}

impl Http {
    pub(crate) fn new(refresh: bool) -> Self {
        let agent = ureq::Agent::config_builder()
            .tls_config(jouet_github::tls::config())
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            // crates.io asks for a user agent that says who is calling.
            .user_agent(concat!(
                "jouet/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/jdx/jouet)"
            ))
            .build()
            .into();
        let fake = std::env::var("JOUET_DEPS_API_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .map(|u| u.trim_end_matches('/').to_string());
        Self {
            agent,
            fake,
            refresh,
        }
    }

    pub(crate) fn url(&self, service: Service, path: &str) -> String {
        match &self.fake {
            Some(root) => format!("{root}/{}{path}", service.key()),
            None => format!("{}{path}", service.root()),
        }
    }

    pub(crate) fn get(&self, service: Service, path: &str) -> Result<Response, String> {
        self.send(service, path, None)
    }

    pub(crate) fn post(
        &self,
        service: Service,
        path: &str,
        body: &str,
    ) -> Result<Response, String> {
        self.send(service, path, Some(body))
    }

    /// Both methods are safe to repeat: every request here only reads.
    fn send(&self, service: Service, path: &str, body: Option<&str>) -> Result<Response, String> {
        let url = self.url(service, path);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let sent = match body {
                Some(body) => self
                    .agent
                    .post(&url)
                    .content_type("application/json")
                    .send(body),
                None => self
                    .agent
                    .get(&url)
                    .header("Accept", "application/json")
                    .call(),
            }
            .and_then(|mut resp| {
                let text = resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_BODY)
                    .read_to_string()?;
                Ok((resp.status().as_u16(), text))
            });
            let retry = match &sent {
                Ok((status, _)) => *status == 429 || *status >= 500,
                Err(_) => true,
            };
            if retry && attempt < 2 {
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            return match sent {
                Ok((status, _)) if status == 429 || status >= 500 => {
                    Err(format!("{} returned {status} for {url}", service.name()))
                }
                Ok((status, body)) => Ok(Response { status, body }),
                Err(e) => Err(format!("could not reach {}: {e}", service.name())),
            };
        }
    }
}
