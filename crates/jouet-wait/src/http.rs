//! HTTP requests for `jouet http` and `wait http`: jouet's TLS, a timeout on
//! every request, a cap on the body read, and hurl-style assertions on the
//! response ([`Assertion`]).

mod expect;

pub use expect::{Assertion, Checked, Facts, select};
pub use serde_json_path::JsonPath;

use std::time::{Duration, Instant};

/// One request.
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
    /// The longest the request may take, connecting and reading the body
    /// included.
    pub timeout: Duration,
    /// The most of the body read; a longer body is an error. 0 reads none.
    pub body_limit: u64,
}

/// What came back.
pub struct Response {
    pub status: u16,
    /// Names in lowercase, in the order the server sent them.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub elapsed: Duration,
}

impl Response {
    /// The status code's reason phrase, such as `Not Found`.
    pub fn reason(&self) -> Option<&'static str> {
        ureq::http::StatusCode::from_u16(self.status)
            .ok()
            .and_then(|s| s.canonical_reason())
    }

    /// The first value of a header, by its lowercase name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// What assertions are checked against, with the body as `json` when it
    /// parsed.
    pub fn facts<'a>(&'a self, json: Option<&'a serde_json::Value>) -> Facts<'a> {
        Facts {
            status: self.status,
            headers: &self.headers,
            body: &self.body,
            json,
            elapsed: self.elapsed,
        }
    }
}

/// Why no response came back.
#[derive(Debug)]
pub enum Failure {
    /// The request ran past its timeout.
    Timeout(String),
    /// The server could not be reached, TLS failed, or the response could not
    /// be read.
    Unreachable(String),
    /// The request itself is wrong: a bad method, URL or header.
    Invalid(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Self::Timeout(m) | Self::Unreachable(m) | Self::Invalid(m) => m,
        }
    }
}

/// Refuses anything but an `http://` or `https://` URL.
pub fn check_url(url: &str) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("`{url}` is not an http:// or https:// URL"))
    }
}

/// An agent with jouet's TLS that never turns a status into an error.
/// Redirects are followed (up to 10, without the `Authorization` header)
/// unless `follow_redirects` is false, when a 3xx is the answer.
pub fn agent(follow_redirects: bool) -> ureq::Agent {
    ureq::Agent::config_builder()
        .tls_config(jouet_github::tls::config())
        .http_status_as_error(false)
        .max_redirects(if follow_redirects { 10 } else { 0 })
        .user_agent(concat!("jouet/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// Sends the request and reads the response, the body up to its limit.
pub fn send(agent: &ureq::Agent, req: &Request<'_>) -> Result<Response, Failure> {
    let method = ureq::http::Method::from_bytes(req.method.to_ascii_uppercase().as_bytes())
        .map_err(|_| Failure::Invalid(format!("`{}` is not an HTTP method", req.method)))?;
    let mut builder = ureq::http::Request::builder().method(method).uri(req.url);
    for (name, value) in req.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    let body = req.body.unwrap_or_default().to_vec();
    let request = builder
        .body(body)
        .map_err(|e| Failure::Invalid(format!("the request is not valid: {e}")))?;
    let request = agent
        .configure_request(request)
        .timeout_global(Some(req.timeout))
        .build();
    let started = Instant::now();
    let mut resp = agent.run(request).map_err(failure)?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(n, v)| {
            (
                n.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = if req.body_limit == 0 {
        Vec::new()
    } else {
        resp.body_mut()
            .with_config()
            .limit(req.body_limit)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(limit) => Failure::Unreachable(format!(
                    "the body is larger than {limit} bytes, the most jouet reads"
                )),
                e => failure(e),
            })?
    };
    Ok(Response {
        status,
        headers,
        body,
        elapsed: started.elapsed(),
    })
}

fn failure(e: ureq::Error) -> Failure {
    match e {
        ureq::Error::Timeout(_) => Failure::Timeout(format!("no response in time: {e}")),
        ureq::Error::BadUri(_) | ureq::Error::Http(_) => Failure::Invalid(e.to_string()),
        e => Failure::Unreachable(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// A server that answers one request with `response`, and hands back
    /// the request it read.
    fn serve(response: String) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut head = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            head.push_str(&String::from_utf8_lossy(&body));
            stream.write_all(response.as_bytes()).unwrap();
            head
        });
        (url, handle)
    }

    fn request<'a>(method: &'a str, url: &'a str, headers: &'a [(String, String)]) -> Request<'a> {
        Request {
            method,
            url,
            headers,
            body: Some(b"{\"a\":1}"),
            timeout: Duration::from_secs(5),
            body_limit: 1024,
        }
    }

    #[test]
    fn sends_the_method_headers_and_body_and_reads_the_response() {
        let (url, server) = serve(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nX-Two: a\r\nX-Two: b\r\nContent-Length: 11\r\n\r\n{\"ok\":true}".into(),
        );
        let headers = vec![("x-test".to_string(), "yes".to_string())];
        let resp = send(&agent(true), &request("patch", &url, &headers)).unwrap();
        let sent = server.join().unwrap();
        assert!(sent.starts_with("PATCH /x HTTP/1.1\r\n"), "{sent}");
        assert!(sent.contains("x-test: yes\r\n"), "{sent}");
        assert!(sent.ends_with("{\"a\":1}"), "{sent}");
        assert_eq!(resp.status, 201);
        assert_eq!(resp.reason(), Some("Created"));
        assert_eq!(resp.header("content-type"), Some("application/json"));
        let twos: Vec<_> = resp.headers.iter().filter(|(n, _)| n == "x-two").collect();
        assert_eq!(twos.len(), 2);
        assert_eq!(resp.body, b"{\"ok\":true}");
    }

    #[test]
    fn a_body_over_the_limit_is_refused_and_a_bad_method_is_invalid() {
        let (url, server) = serve(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 2000\r\n\r\n{}",
            "x".repeat(2000)
        ));
        let req = Request {
            body_limit: 10,
            ..request("GET", &url, &[])
        };
        let err = send(&agent(true), &req).err().unwrap();
        assert!(matches!(err, Failure::Unreachable(_)), "{err:?}");
        assert!(err.message().contains("larger than 10 bytes"), "{err:?}");
        drop(server);
        let err = send(&agent(true), &request("GE T", &url, &[]))
            .err()
            .unwrap();
        assert!(matches!(err, Failure::Invalid(_)), "{err:?}");
    }

    #[test]
    fn only_http_urls_are_taken() {
        assert!(check_url("https://example.com").is_ok());
        assert!(check_url("localhost:80").is_err());
        assert!(check_url("file:///etc/passwd").is_err());
    }
}
