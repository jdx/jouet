/// Builds a [`Client`](super::Client).
pub struct ClientBuilder {
    timeout: u64,
}

impl ClientBuilder {
    /// Sets the timeout, in seconds.
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout = secs;
        self
    }
}
