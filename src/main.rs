//! jouet: a toolkit CLI for coding agents.
//!
//! Early development: there are no commands yet, only `--help` and `--version`.

use usage_rs::Cli;

#[derive(Cli)]
#[usage(
    name = "jouet",
    bin = "jouet",
    version,
    about = "A toolkit CLI for coding agents",
    unknown_flags = "error"
)]
struct Jouet {}

fn main() {
    Jouet::parse();
}
