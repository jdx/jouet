//! A local fake GitHub (or GitLab, or Forgejo) for the subprocess tests, so
//! nothing they run talks to the real API.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Output;
use std::sync::{Arc, Mutex};

/// A request the fake server received: its method, path, token and body
/// (`null` for a GET).
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub token: String,
    pub body: Value,
}

pub(crate) struct Fake {
    pub url: String,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Fake {
    pub(crate) fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }
}

/// A status for `respond` to return: a 200 whose body is cut short.
pub(crate) const CUT_SHORT: u16 = 0;
/// A status for `respond` to return: a 403 saying the primary rate limit is
/// used up for the next hour.
pub(crate) const RATE_LIMITED: u16 = 1;

/// Serves `respond(request, index)` for each request, one per connection.
pub(crate) fn serve(respond: impl Fn(&Request, usize) -> (u16, String) + Send + 'static) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut start = String::new();
            reader.read_line(&mut start).unwrap_or(0);
            let mut words = start.split_whitespace();
            let method = words.next().unwrap_or_default().to_string();
            let path = words.next().unwrap_or_default().to_string();
            let (mut token, mut length) = (String::new(), 0);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let (name, value) = line.split_once(':').unwrap_or_default();
                let value = value.trim();
                match name.to_ascii_lowercase().as_str() {
                    // GitHub's `bearer`, GitLab's `Bearer`, Forgejo's `token`.
                    "authorization" => {
                        token = ["bearer ", "Bearer ", "token "]
                            .iter()
                            .find_map(|p| value.strip_prefix(p))
                            .unwrap_or(value)
                            .into();
                    }
                    "content-length" => length = value.parse().unwrap(),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request = Request {
                method,
                path,
                token,
                body: serde_json::from_slice(&body).unwrap_or(Value::Null),
            };
            let index = {
                let mut seen = seen.lock().unwrap();
                seen.push(request.clone());
                seen.len() - 1
            };
            let (status, text) = respond(&request, index);
            // `CUT_SHORT` promises a longer body than it sends, then hangs up.
            let (status, length, extra) = match status {
                CUT_SHORT => (200, text.len() + 100, String::new()),
                RATE_LIMITED => {
                    let reset = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        + 3600;
                    let headers =
                        format!("X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: {reset}\r\n");
                    (403, text.len(), headers)
                }
                _ => (status, text.len(), String::new()),
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}\
                 Content-Length: {length}\r\nConnection: close\r\n\r\n{text}"
            );
        }
    });
    Fake { url, requests }
}

/// A GraphQL request's query text, or `""` for a REST request.
pub(crate) fn query(request: &Request) -> &str {
    request.body["query"].as_str().unwrap_or_default()
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub(crate) fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(out)))
}
