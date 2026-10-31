use usage_rs::{Args, Subcommands};

/// Notice costly patterns in the hook ledger and whole sessions, and propose fixes
///
/// Reads the ledger jouet's hooks keep from where it last stopped. When a
/// command keeps failing across sessions, or agents keep sleeping instead of
/// waiting, it drafts a rule in warn mode and keeps a note of it, which the
/// next session start shows (two at most). The hooks run it on their own;
/// running it by hand reads the ledger now. Off with JOUET_WIND=0.
///
/// The ledger holds only command shapes. `jouet wind digest` and `jouet wind
/// review` read whole sessions: the user's corrections, abandoned
/// approaches, waits and repeated exploration. A review's proposals (mise
/// tasks, rules, routines, AGENTS.md or memory notes, or changes to the
/// memories agents already keep) are recorded with `jouet wind propose` and
/// shown like the ledger's notes. `jouet wind memories` notices memories
/// whose workaround looks fixed.
#[derive(Args)]
pub(crate) struct WindArgs {
    #[usage(subcommand)]
    pub(crate) cmd: Option<WindCmd>,
}

#[derive(Subcommands)]
pub(crate) enum WindCmd {
    /// Show a note or a proposal: what was noticed, and what was drafted
    Show(WindNoteArgs),
    /// Drop a note, a proposal or a suggested review, so it is never shown again
    Dismiss(WindNoteArgs),
    /// Mark a review's proposal or a memory note as dealt with
    Adopt(WindNoteArgs),
    /// Digest whole sessions: corrections, failures, waits, re-reads
    Digest(WindDigestArgs),
    /// Print a prompt and session digests for an agent to review
    Review(WindDigestArgs),
    /// Record what a review proposed, to show at the next session start
    Propose(WindProposeArgs),
    /// Check your agents' memories for workarounds whose bug looks fixed
    Memories(WindMemoriesArgs),
}

/// Show or drop a winding note
#[derive(Args)]
pub(crate) struct WindNoteArgs {
    /// The id of a note (n3), a proposal (p2), a suggested review (r1), a
    /// mise task (t1) or a memory note (m1)
    pub(crate) id: String,
}

/// Digest whole sessions: corrections, failures, waits, re-reads
///
/// Reads a Claude Code or Codex transcript and reduces it to a few thousand
/// tokens: the user's corrections and interruptions, failures and their
/// retries, long calls, where tokens and time went (as `jouet audit` counts
/// them), files read repeatedly, commands typed out again, and a timeline
/// with a phase per prompt. Every item names its lines in the transcript.
/// Secrets are redacted (with gitleaks' rules too, when a config is found)
/// and the home directory is written as ~. Nothing leaves the machine.
/// `jouet wind review` prints the same with a prompt for a reviewing agent.
#[derive(Args)]
pub(crate) struct WindDigestArgs {
    /// A session ID, a prefix of one, or a transcript's path
    pub(crate) session: Option<String>,
    /// The most recently written session (inside a session, that session)
    #[usage(long)]
    pub(crate) last: bool,
    /// Every session written since TIME (7d, 12h, a date or RFC 3339), the
    /// 8 most recent
    #[usage(long, value_name = "TIME")]
    pub(crate) since: Option<String>,
    /// Which transcripts to read: claude, codex or all (default all)
    #[usage(long, value_name = "HARNESS")]
    pub(crate) harness: Option<String>,
    /// Only sessions that ran in PATH or a directory below it
    #[usage(long, value_name = "PATH")]
    pub(crate) project: Option<String>,
    /// Tokens per session's digest, about (default 4000)
    #[usage(long, value_name = "N")]
    pub(crate) budget: Option<usize>,
}

/// Check your agents' memories for workarounds whose bug looks fixed
///
/// Reads Claude Code's memories (~/.claude/projects/*/memory/*.md and
/// ~/.claude/CLAUDE.md), ~/.codex/AGENTS.md and Codex's consolidated
/// memories, and checks what each one depends on: a PR or issue it says it
/// waits on (merged or closed now?), a merge it says is not released yet
/// (in a release now?), a `jouet` command or flag it names (still there?),
/// and a built-in rule it says to get past (a jouet bug, not a memory).
/// It lists what it finds, and asks GitHub, with the token `jouet pr`
/// uses. It never changes a memory. The stop hook runs it with --record
/// once a day in the background; the next session starts in the memory's
/// project then show each note in one line. Off with JOUET_WIND_MEMORIES=0.
#[derive(Args)]
pub(crate) struct WindMemoriesArgs {
    /// Also record what it finds as notes, two a day at most, to show at
    /// session starts
    #[usage(long)]
    pub(crate) record: bool,
}

/// Record what a review proposed, to show at the next session start
///
/// Reads a review's JSON reply on stdin (or from FILE): `{"session": "...",
/// "proposals": [{"kind", "title", "why", "evidence", "scope", "draft"}]}`,
/// as `jouet wind review` asks for. Each proposal is checked and recorded
/// as pending: three per review and three a day at most, never one that
/// looks like it holds a secret, never one already proposed. Recording
/// applies nothing: the next session starts show each proposal in one line,
/// and the agent there asks the user before applying it.
#[derive(Args)]
pub(crate) struct WindProposeArgs {
    /// Read the reply from FILE instead of stdin
    pub(crate) file: Option<String>,
}

/// Propose retiring rules that are noisy or always overridden, and unused tasks
///
/// Lists your rules and a trusted repository's that the hook ledger shows
/// are nearly always overridden, fire on a large share of calls, or no
/// longer load. Built-in rules are never proposed. --apply moves your own
/// rules to ~/.config/jouet/rules/retired/ and logs it; a repository's rules
/// change through a PR. It also lists the mise tasks `jouet task apply` added
/// that no agent session has run in the 21 days since, and still run what
/// jouet wrote; --apply removes them with `mise tasks rm`.
#[derive(Args)]
pub(crate) struct UnwindArgs {
    /// Retire the proposed rules of yours, and remove the proposed tasks
    #[usage(long)]
    pub(crate) apply: bool,
}
