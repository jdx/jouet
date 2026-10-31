mod builder;

pub use builder::ClientBuilder;

/// Sends requests and keeps connections open between them.
#[derive(Clone, Debug, Default)]
pub struct Client {
    timeout: u64,
}

impl Client {
    /// A client with the default settings.
    pub fn new() -> Self {
        Client::default()
    }

    /// Sends a GET request to `url`.
    ///
    /// # Errors
    ///
    /// Fails when the server answers with an error status.
    pub fn get(&self, url: &str) -> Result<String, crate::Error> {
        let _ = self.timeout;
        Ok(url.to_string())
    }

    fn private_helper(&self) {}
}

#[cfg(test)]
impl Client {
    pub fn test_only(&self) {}
}
