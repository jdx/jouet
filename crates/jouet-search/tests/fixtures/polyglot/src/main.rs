use polyglot::Downloader;

fn main() {
    let downloader = Downloader::new("https://example.com");
    let bytes = downloader.download("index.html");
    println!("{bytes:?}");
}
