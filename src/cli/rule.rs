use usage_rs::{Args, Subcommands};

/// List and test the rules jouet's hooks run
///
/// Rules are `[[rule]]` tables in TOML files: built in, in /etc/jouet/rules
/// (enforced: your settings cannot turn them off), in
/// ~/.config/jouet/rules, in a repository's .jouet/rules once it is
/// trusted (`jouet trust`), and in plugins. `jouet rule schema` prints
/// their JSON schema; a rule with logic a matcher cannot express names a
/// Luau `script`, written against the API `jouet rule types` prints.
#[derive(Args)]
pub(crate) struct RuleArgs {
    #[usage(subcommand)]
    pub(crate) cmd: RuleCmd,
}

#[derive(Subcommands)]
pub(crate) enum RuleCmd {
    /// List every rule, where it comes from and whether it runs
    List(RuleListArgs),
    /// Run a rule against a command or hook payload, or against its examples
    Test(RuleTestArgs),
    /// Print the JSON schema of rule files
    Schema(RuleSchemaArgs),
    /// Print the Luau type definitions of the jouet module, for rule scripts
    Types(RuleTypesArgs),
    /// Write a new rule of yours, in warn mode, from a template or a winding note
    New(RuleNewArgs),
    /// Switch a rule of yours from warn to deny
    Promote(RulePromoteArgs),
    /// Show every change made to your rules, and why
    Log(RuleLogArgs),
    /// Record something a rule could not express, for `jouet share --idea`
    Gap(RuleGapArgs),
}

/// List every rule, where it comes from and whether it runs
#[derive(Args)]
pub(crate) struct RuleListArgs {}

/// Run a rule against a command or hook payload, or against its examples
///
/// With neither --command nor --payload, runs the rule's `examples`: every
/// `fires` example must make it deny, warn or record, and every `quiet`
/// one must leave it silent. It fails, showing what was expected and what
/// the rule did, when one does not, or when the rule lacks either kind.
/// --all does this for every rule. --replay runs the rule over every call
/// in the hook ledger instead, and fails if it fires on none of them.
#[derive(Args)]
pub(crate) struct RuleTestArgs {
    /// A rule's name, or the path to a .toml (or .luau) rule file
    pub(crate) rule: Option<String>,
    /// Run the examples of every rule: built in, yours and this repository's
    #[usage(long)]
    pub(crate) all: bool,
    /// A shell command to test the rule against
    #[usage(long, value_name = "CMD")]
    pub(crate) command: Option<String>,
    /// A Claude Code or Codex hook payload (JSON) to test against; `-` is stdin
    #[usage(long, value_name = "FILE")]
    pub(crate) payload: Option<String>,
    /// The command's exit code, for a post-tool-use rule (default 0)
    #[usage(long, value_name = "N")]
    pub(crate) exit_code: Option<i32>,
    /// Run the rule over the hook ledger's calls: what it fires on, and the near misses
    #[usage(long)]
    pub(crate) replay: bool,
}

/// Print the JSON schema of rule files
///
/// Every key a `[[rule]]` table takes, with what it means. The rule files
/// jouet writes start with a `#:schema` line, so editors that read it
/// complete and check them.
#[derive(Args)]
pub(crate) struct RuleSchemaArgs {}

/// Print the Luau type definitions of the jouet module, for rule scripts
#[derive(Args)]
pub(crate) struct RuleTypesArgs {}

/// Write a new rule of yours, in warn mode, from a template or a winding note
///
/// Writes ~/.config/jouet/rules/NAME.toml, a rule saying why it exists, who
/// wrote it and when, with a matcher and examples to fill in. It starts in
/// warn mode: `jouet rule promote` makes its denials real. --script adds
/// NAME.luau for logic a matcher cannot express. With --from-note, the rule
/// is the one `jouet wind` drafted.
#[derive(Args)]
pub(crate) struct RuleNewArgs {
    /// The rule's name: lowercase words joined by dashes
    pub(crate) name: String,
    /// Start from the rule a winding note drafted, such as n3
    #[usage(long, value_name = "ID")]
    pub(crate) from_note: Option<String>,
    /// What the rule is for, in one line (the template's `why`)
    #[usage(long, value_name = "TEXT")]
    pub(crate) why: Option<String>,
    /// Who is writing it: your session's id (default: $JOUET_SESSION_ID, or the harness's)
    #[usage(long, value_name = "WHO")]
    pub(crate) author: Option<String>,
    /// Also write NAME.luau, a script for logic a matcher cannot express
    #[usage(long)]
    pub(crate) script: bool,
}

/// Switch a rule of yours from warn to deny
///
/// Only rules in ~/.config/jouet/rules: built-in rules are part of jouet,
/// and a repository's change through a reviewed PR. Logged in `jouet rule
/// log`.
#[derive(Args)]
pub(crate) struct RulePromoteArgs {
    /// The rule's name
    pub(crate) name: String,
    /// Why, for the log
    #[usage(long, value_name = "TEXT")]
    pub(crate) reason: Option<String>,
}

/// Show every change made to your rules, and why
#[derive(Args)]
pub(crate) struct RuleLogArgs {
    /// Only this rule's changes
    pub(crate) name: Option<String>,
}

/// Record something a rule could not express, for `jouet share --idea`
///
/// jouet records on its own when a rule reads a `jouet` primitive that does
/// not exist. This records a wish in your words: "I kept doing X; there is
/// no primitive for Y".
#[derive(Args)]
pub(crate) struct RuleGapArgs {
    /// What you wanted a rule to do, and what was missing
    pub(crate) text: String,
    /// The rule you were writing, if any
    #[usage(long, value_name = "NAME")]
    pub(crate) rule: Option<String>,
}
