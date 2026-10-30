//! Where coding-agent sessions spend their tokens and time, and which jouet
//! commands and rules would cut it. This is the engine behind `jouet audit`.
//!
//! The pieces, in the order data flows through them:
//!
//! - A [`Reader`] turns one harness's transcript file into a [`Transcript`]:
//!   the model requests it made ([`Request`]) and the tool calls each request
//!   issued ([`Call`]). [`ClaudeCode`] and [`Codex`] are the readers so far;
//!   [`scan`] finds their files and reads them in parallel.
//! - [`categorize`] puts a call in a [`Category`], [`detect`] finds the
//!   recurring [`Pattern`]s in a command, and [`shape()`] reduces a command to
//!   its program, subcommands and flags, with every value replaced, which is
//!   the only form of a command a report shows. [`redact`] removes
//!   token-like strings from anything else that is shown.
//! - An [`Analyzer`] accumulates transcripts, merges with other analyzers
//!   (one per thread, or one per run of an incremental audit), and produces a
//!   [`Report`].
//!
//! Nothing here touches the network, and a report never holds file contents
//! or whole commands, only shapes, unless examples are asked for, and those
//! are redacted.
//!
//! An incremental audit over another source of calls, such as jouet's own
//! hook ledger, builds [`Transcript`]s itself (one [`Request`] per call when
//! it has no token counts) and feeds them to an [`Analyzer`].
//! [`categorize`] and [`detect`] also accept a command that is already a
//! shape, so a ledger that stores only shapes loses only the patterns that
//! look inside a script's body.

mod analyze;
mod category;
mod event;
mod pattern;
mod read;
mod redact;
mod shape;
mod time;

pub use analyze::{
    Analyzer, CategoryReport, HarnessReport, PatternReport, Report, Saving, ShapeCount, Suggestion,
    Totals,
};
pub use category::{Category, categorize};
pub use event::{Call, Request, Tool, Transcript};
pub use pattern::{Pattern, Patterns, detect};
pub use read::{
    ClaudeCode, Codex, Filter, Reader, Scan, ScanStats, Source, Unreadable, open, scan,
};
pub use redact::{example, redact};
pub use shape::shape;
pub use time::parse_timestamp;
