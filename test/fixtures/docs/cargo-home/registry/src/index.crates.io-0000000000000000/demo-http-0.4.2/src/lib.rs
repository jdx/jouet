//! A small HTTP client, for jouet's tests.
//!
//! It has a blocking client too.

#[doc(hidden)]
pub mod __private;
pub mod blocking;
mod client;
mod util;

pub use self::client::{Client, ClientBuilder};
pub use url::Url;

/// Makes a GET request with a default client.
pub fn get(url: &str) -> Result<String, Error> {
    Client::new().get(url)
}

/// Why a request failed.
#[derive(Debug)]
pub enum Error {
    /// The server answered with this status.
    Status(u16),
    /// The connection failed.
    Connect,
}

pub(crate) fn internal() {}

#[cfg(test)]
mod tests {
    pub fn helper() {}
}
