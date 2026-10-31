//! A downloader.

pub struct Downloader {
    base: String,
}

impl Downloader {
    pub fn new(base: &str) -> Self {
        Self { base: base.to_string() }
    }

    pub fn download(&self, path: &str) -> Result<Vec<u8>, String> {
        let url = join_url(&self.base, path);
        fetch_bytes(&url)
    }
}

fn join_url(base: &str, path: &str) -> String {
    format!("{base}/{path}")
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    Err(format!("offline: {url}"))
}
