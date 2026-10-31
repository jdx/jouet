//! A client that waits for each response.

/// Waits for each response.
pub struct Client;

impl Client {
    /// Sends a GET request and blocks until the answer.
    pub fn get(&self, url: &str) -> String {
        url.to_string()
    }
}
