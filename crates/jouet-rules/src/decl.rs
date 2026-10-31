//! The TOML rule format: a file of `[[rule]]` tables, each a matcher, a
//! message and its examples, with a Luau `script` for a rule a matcher
//! cannot express.
//!
//! ```toml
//! [[rule]]
//! name = "git-add-all"
//! why = "`git add -A` stages everything, including other sessions' work."
//! mode = "deny"
//! message = "Stage explicit paths."
//! suggest = "git add <path>..."
//!
//! [[rule.match]]
//! command = "git add"
//! flags = ["-A", "--all"]
//!
//! [rule.examples]
//! fires = ["git add -A"]
//! quiet = ["git add src/main.rs"]
//! ```
//!
//! These types are both the parser and the JSON schema (`jouet rule
//! schema`): what the schema says is exactly what loads. They describe the
//! file; [`crate::matcher`] runs the matchers.

use crate::{Event, Kind, Mode, Scope};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::Deserialize;
use serde::de::{self, Deserializer, IntoDeserializer, Visitor};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

/// Where the published schema lives, for a `#:schema` line at the top of a
/// rule file.
pub const SCHEMA_URL: &str = "https://jouet.jdx.dev/schema/rules.json";

/// A rule file.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    title = "jouet rules",
    description = "A file of jouet rules: `[[rule]]` tables that steer an agent's tool calls. See https://jouet.jdx.dev/rules"
)]
pub(crate) struct RuleFile {
    /// The rules in this file.
    #[serde(default)]
    #[schemars(with = "Vec<RuleDecl>")]
    pub rule: Vec<toml::Spanned<RuleDecl>>,
}

/// One rule: what it matches, what it tells the agent, and the examples
/// that prove it.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Rule")]
pub(crate) struct RuleDecl {
    /// The rule's name: lowercase words joined by dashes, unique among
    /// every rule jouet loads.
    pub name: Name,
    /// What the rule is for, in one line.
    #[schemars(length(min = 1))]
    pub why: String,
    /// `deny` blocks a call it matches; `warn` lets the call run and shows
    /// the message. A rule of yours defaults to `warn`, and only warns until
    /// it passes examples of both kinds.
    pub mode: Option<Mode>,
    /// Where the rule lives, if it says: it must match the directory.
    pub scope: Option<Scope>,
    /// Who wrote it: a person, or the agent session that did.
    pub author: Option<String>,
    /// When, as a date.
    pub created: Option<String>,
    /// When the rule runs. Only a `script` rule can run after a tool
    /// (`post-tool-use`), to record a test, lint or build run.
    pub on: Option<Event>,
    /// The jouet command the rule's advice names, such as `["wait", "ci"]`:
    /// the rule stays off until the installed jouet has it.
    #[serde(default)]
    pub needs: Vec<String>,
    /// What the agent is told when the rule matches. Required unless a
    /// `script` decides.
    pub message: Option<String>,
    /// A command to run instead, shown before the message as "Run `...`
    /// instead."
    pub suggest: Option<String>,
    /// The calls the rule fires on: a table, or a list of tables of which
    /// any may match.
    #[serde(rename = "match", default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<Matcher>")]
    pub matchers: Vec<Matcher>,
    /// A Luau file beside this one, for logic a matcher cannot express. It
    /// returns a table with a `check` function (`jouet rule types` prints
    /// the API). With a `match` too, the script only runs on calls the
    /// matcher matches.
    pub script: Option<ScriptPath>,
    /// Calls the rule must fire on, and near misses it must leave alone.
    pub examples: Option<ExamplesDecl>,
}

/// Which calls a rule matches. Every key given must hold; a list in a key
/// matches if any of its items does (`text` excepted: every pattern must
/// match). Every key but `tool`, `paths`, `unless_paths`, `input`,
/// `unless_input`, `seen`, `min_read_lines`, `fnox` and `secret_files` is
/// about a command, and needs `command`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Matcher {
    /// The harness's tool name, such as `Bash`, `Edit` or `WebFetch`; `*`
    /// and `?` are wildcards. Without it, a matcher with `command` matches
    /// any tool that runs a command.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub tool: Vec<Glob>,
    /// For file tools: the path the tool reads or writes. A pattern without
    /// a `/` matches the file name, one with a `/` the path from the working
    /// directory; `*` stays within a directory and `**` crosses them.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub paths: Vec<Glob>,
    /// The program and its subcommands, such as `"git stash pop"`. Matched
    /// after wrappers such as `rtk`, `sudo` and `env`, and past the options
    /// before the first subcommand (`git -C dir add`). Words may use `*` and
    /// `?`.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub command: Vec<Words>,
    /// The wrapper the command ran through, such as `rtk`, `sudo` or
    /// `mise`.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub via: Vec<String>,
    /// The command has one of these options, such as `-f` or `--force`.
    /// `-f` matches inside a cluster such as `-fu`, and `--force` matches
    /// `--force=...`.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<Flag>")]
    pub flags: Vec<Flag>,
    /// The command has none of these options.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<Flag>")]
    pub unless_flags: Vec<Flag>,
    /// The command's options are exactly these, whatever their values.
    pub exact_flags: Option<Vec<Flag>>,
    /// The command's options that take a value, so that in `git commit -m
    /// -n` the `-n` is a message, not an option, and `-mn` is `-m n`.
    #[serde(default)]
    pub value_flags: Vec<Flag>,
    /// One of the command's arguments (the words after its subcommands
    /// that are not options or their values) matches one of these. `*` and
    /// `?` are wildcards.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub args: Vec<Glob>,
    /// None of the command's arguments matches any of these.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub unless_args: Vec<Glob>,
    /// The command has at most this many arguments.
    pub max_args: Option<usize>,
    /// The command runs in the background: `&` follows it, or the harness
    /// runs the call in the background (Claude Code's `run_in_background`).
    pub background: Option<bool>,
    /// Regular expressions that must each match one of the command's words
    /// after the program, or one of its here-documents. `|` gives
    /// alternatives within one.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub text: Vec<Pattern>,
    /// Another command in the same call must match this too, as in `sleep
    /// 60 && gh pr checks`. It takes the command keys, not `tool`, `paths`
    /// or another `with`.
    pub with: Option<Box<Matcher>>,
    /// No other command in the same call may match this, as in `rg x |
    /// head`. It takes the command keys, like `with`.
    pub unless_with: Option<Box<Matcher>>,
    /// The tool's input has these fields with these values, such as
    /// `{ output_mode = "content" }` for Claude Code's Grep. A value that
    /// is not a string is matched as its JSON text; `*` and `?` are
    /// wildcards.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, String>")]
    pub input: BTreeMap<String, Glob>,
    /// The tool's input has none of these fields, such as `head_limit`.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub unless_input: Vec<String>,
    /// Whether this agent session was already shown everything the call
    /// reads, every line, in files unchanged since. jouet's hook works it
    /// out for Claude Code's Read tool and for shell calls that only read
    /// files with `cat`, `head`, `tail`, `sed -n` or `nl`; any other call is
    /// unseen. In an example, set it with `seen = true`.
    pub seen: Option<bool>,
    /// The call puts at least this many lines of files in the context:
    /// Claude Code's Read (up to 2,000 lines without a `limit`), or a shell
    /// read with `cat`, `head`, `tail`, `sed -n` or `nl`. jouet's hook counts
    /// them; set it in an example with `read_lines`.
    pub min_read_lines: Option<u32>,
    /// For file tools: the path matches none of these patterns, read as in
    /// `paths`.
    #[serde(default, deserialize_with = "one_or_many")]
    #[schemars(with = "OneOrMany<String>")]
    pub unless_paths: Vec<Glob>,
    /// Whether the call runs in a project that uses fnox: a `fnox.toml`,
    /// `.fnox.toml` or `.config/fnox.toml` in the working directory or
    /// above it. In an example, set it with `fnox = {}`.
    pub fnox: Option<bool>,
    /// The call reads a file that a fnox `plain` provider reads secrets
    /// from: a file tool's path, or with `command`, one of the command's
    /// arguments. In an example, list them in `fnox = { files = [...] }`.
    pub secret_files: Option<bool>,
    /// The command expands, outside single quotes, a variable the
    /// project's fnox config defines as a secret: `$NAME` or `${NAME}`. In
    /// an example, list them in `fnox = { secrets = [...] }`.
    pub secret_vars: Option<bool>,
    /// One of the command's arguments is the name of a secret the project's
    /// fnox config defines, as in `printenv NAME`.
    pub secret_args: Option<bool>,
    /// Whether the command's standard output reaches the transcript: it is
    /// not redirected to a file, piped into another command, or captured
    /// with `$(...)`.
    pub prints: Option<bool>,
}

impl Matcher {
    /// Whether it says anything about a command, beyond its tool and path.
    pub(crate) fn about_a_command(&self) -> bool {
        let Self {
            tool: _,
            paths: _,
            input: _,
            unless_input: _,
            seen: _,
            min_read_lines: _,
            unless_paths: _,
            fnox: _,
            secret_files: _,
            command,
            via,
            flags,
            unless_flags,
            exact_flags,
            value_flags,
            args,
            unless_args,
            max_args,
            background,
            text,
            with,
            unless_with,
            secret_vars,
            secret_args,
            prints,
        } = self;
        !command.is_empty()
            || !via.is_empty()
            || !flags.is_empty()
            || !unless_flags.is_empty()
            || exact_flags.is_some()
            || !value_flags.is_empty()
            || !args.is_empty()
            || !unless_args.is_empty()
            || max_args.is_some()
            || background.is_some()
            || !text.is_empty()
            || with.is_some()
            || unless_with.is_some()
            || secret_vars.is_some()
            || secret_args.is_some()
            || prints.is_some()
    }

    /// What is wrong with it, beyond what its types check.
    pub(crate) fn problem(&self, nested: bool) -> Option<String> {
        let not_command = !self.tool.is_empty()
            || !self.paths.is_empty()
            || !self.input.is_empty()
            || !self.unless_input.is_empty()
            || self.seen.is_some()
            || self.min_read_lines.is_some()
            || !self.unless_paths.is_empty()
            || self.fnox.is_some()
            || self.secret_files.is_some()
            || self.with.is_some()
            || self.unless_with.is_some();
        if nested && not_command {
            return Some("`with` and `unless_with` match another command: give them command keys, not `tool`, `paths`, `unless_paths`, `input`, `unless_input`, `seen`, `min_read_lines`, `fnox`, `secret_files`, `with` or `unless_with`".into());
        }
        if self.command.is_empty() && self.about_a_command() {
            return Some("it says how a command looks, but not which: add `command`".into());
        }
        if self.command.is_empty() && self.tool.is_empty() && self.paths.is_empty() {
            return Some(
                "it would match every call: give it a `command`, `tool` or `paths`".into(),
            );
        }
        self.with
            .iter()
            .chain(&self.unless_with)
            .find_map(|w| w.problem(true))
    }
}

/// A rule's examples. Each is a shell command, or a table for a call that
/// needs more than that.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Examples")]
pub(crate) struct ExamplesDecl {
    /// Calls the rule must fire on: deny, warn or record a check.
    #[serde(default)]
    pub fires: Vec<CaseDecl>,
    /// Near misses it must leave alone.
    #[serde(default)]
    pub quiet: Vec<CaseDecl>,
}

/// An example: a shell command, or a table.
#[derive(Debug)]
pub(crate) enum CaseDecl {
    Command(String),
    Call(CaseTable),
}

/// An example call that needs more than a shell command.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ExampleCall")]
pub(crate) struct CaseTable {
    /// The shell command. Required for a `Bash` call.
    pub command: Option<String>,
    /// The harness's tool name; `Bash` if omitted.
    pub tool: Option<String>,
    /// For a file tool: the path it reads or writes.
    pub path: Option<String>,
    /// `claude`, `codex` or another harness; `test` if omitted.
    pub harness: Option<String>,
    /// For a post-tool-use rule; `0` if omitted.
    pub exit_code: Option<i32>,
    /// For a `fires` example: `deny` or `warn`, when it matters which.
    pub action: Option<ExpectAction>,
    /// For a `fires` example of a post-tool-use rule: the kind it must
    /// record.
    pub kind: Option<Kind>,
    /// For a `fires` example with a `kind`: the scope the record must have,
    /// `full`, or `partial` and its selector, such as `partial -p jouet`.
    pub scope: Option<String>,
    /// The tool's input, for a tool other than `Bash`, such as `{ pattern =
    /// "todo", output_mode = "content" }`.
    pub input: Option<BTreeMap<String, serde_json::Value>>,
    /// Whether the session was already shown everything the call reads,
    /// as jouet's hook would work it out; `false` if omitted.
    pub seen: Option<bool>,
    /// How many lines of files the call reads, as jouet's hook would count
    /// them.
    pub read_lines: Option<u32>,
    /// The call runs in a project that uses fnox, whose config defines
    /// these secrets and files; omitted, it does not.
    pub fnox: Option<FnoxCase>,
    /// What `jouet.git.state()` reports for the work tree; omitted, it has
    /// no answer, as for any probe in an example.
    pub git: Option<GitCase>,
}

/// An example's work tree, as `jouet.git.state()` would report it.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ExampleGit")]
pub(crate) struct GitCase {
    /// Whether anything is changed, untracked or conflicted.
    pub dirty: bool,
}

/// An example's fnox project, as jouet's hook would read it from the
/// project's config.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ExampleFnox")]
pub(crate) struct FnoxCase {
    /// The secret names the config defines.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// The files its `plain` providers read secrets from, from the working
    /// directory.
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExpectAction {
    Deny,
    Warn,
}

impl<'de> Deserialize<'de> for CaseDecl {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = CaseDecl;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a shell command, or a table with `command`, `tool`, `path`, `harness`, `exit_code`, `action`, `kind`, `scope`, `input`, `seen`, `read_lines`, `fnox` or `git`")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<CaseDecl, E> {
                Ok(CaseDecl::Command(v.to_string()))
            }
            fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<CaseDecl, A::Error> {
                CaseTable::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(CaseDecl::Call)
            }
        }
        d.deserialize_any(V)
    }
}

impl JsonSchema for CaseDecl {
    fn schema_name() -> Cow<'static, str> {
        "Example".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let table = generator.subschema_for::<CaseTable>();
        json_schema!({
            "anyOf": [{ "type": "string", "description": "A shell command." }, table]
        })
    }
}

/// A value, or a list of them.
pub(crate) struct OneOrMany<T>(PhantomData<T>);

impl<T: JsonSchema> JsonSchema for OneOrMany<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("OneOrMany_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let item = generator.subschema_for::<T>();
        json_schema!({ "anyOf": [item, { "type": "array", "items": item }] })
    }
}

/// Reads a value or a list of values as a list, keeping the item's own
/// errors (an untagged enum would only say no variant matched).
fn one_or_many<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct V<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a value or a list of values")
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<Vec<T>, E> {
            T::deserialize(v.into_deserializer()).map(|t| vec![t])
        }
        fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Vec<T>, A::Error> {
            T::deserialize(de::value::MapAccessDeserializer::new(map)).map(|t| vec![t])
        }
        fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Vec<T>, A::Error> {
            Vec::<T>::deserialize(de::value::SeqAccessDeserializer::new(seq))
        }
    }
    d.deserialize_any(V(PhantomData))
}

/// Defines a string newtype that checks its value as it is read, with the
/// pattern the schema gives it.
macro_rules! checked_string {
    ($(#[$meta:meta])* $name:ident, $pattern:expr, $check:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub(crate) struct $name(pub String);

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = String::deserialize(d)?;
                let check: fn(&str) -> Result<(), String> = $check;
                check(&text).map_err(de::Error::custom)?;
                Ok(Self(text))
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                let pattern: Option<&str> = $pattern;
                match pattern {
                    Some(pattern) => json_schema!({ "type": "string", "pattern": pattern }),
                    None => json_schema!({ "type": "string" }),
                }
            }

            fn inline_schema() -> bool {
                true
            }
        }
    };
}

checked_string!(
    /// A wildcard pattern.
    Glob,
    None,
    |_| Ok(())
);

checked_string!(
    /// A path to a Luau file in the same directory, or below it.
    ScriptPath,
    Some(r"^[^/\\].*\.luau$"),
    |path| {
        let p = std::path::Path::new(path);
        if p.is_absolute()
            || p.components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(format!(
                "`script` is a path in this rule's directory, such as \"my-rule.luau\", not `{path}`"
            ));
        }
        if p.extension().is_none_or(|e| e != "luau") {
            return Err(format!("`script` names a .luau file, not `{path}`"));
        }
        Ok(())
    }
);

/// A rule's name: lowercase words joined by dashes.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.ends_with('-')
}

checked_string!(
    /// A rule name.
    Name,
    Some("^[a-z][a-z0-9-]{0,62}[a-z0-9]$|^[a-z]$"),
    |name| {
        if valid_name(name) {
            Ok(())
        } else {
            Err(format!(
                "`{name}` is not a rule name: use lowercase words joined by dashes, such as no-tags"
            ))
        }
    }
);

checked_string!(
    /// An option: `-x` or `--long`.
    Flag,
    Some("^(-[^-]|--[^=]+)$"),
    |flag| {
        let ok = match flag.strip_prefix("--") {
            Some(long) => !long.is_empty() && !long.contains('='),
            None => flag.len() == 2 && flag.starts_with('-'),
        };
        if ok {
            return Ok(());
        }
        if flag.starts_with('-') && !flag.starts_with("--") && flag.len() > 2 {
            let each: Vec<String> = flag[1..].chars().map(|c| format!("`-{c}`")).collect();
            return Err(format!(
                "`{flag}` is a cluster of options: list each one ({})",
                each.join(", ")
            ));
        }
        Err(format!(
            "`{flag}` is not an option: write `-x` or `--long`, without a value"
        ))
    }
);

/// A program and its subcommands, as words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Words(pub Vec<String>);

impl<'de> Deserialize<'de> for Words {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Err(de::Error::custom(
                "`command` names a program, such as \"git push\"",
            ));
        }
        Ok(Self(words))
    }
}

/// A regular expression.
#[derive(Debug, Clone)]
pub(crate) struct Pattern(pub regex::Regex);

impl<'de> Deserialize<'de> for Pattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        regex::Regex::new(&text)
            .map(Self)
            .map_err(|e| de::Error::custom(format!("`{text}` is not a regular expression: {e}")))
    }
}

/// The JSON schema of a rule file, as `jouet rule schema` prints it.
pub fn schema() -> String {
    let settings = schemars::generate::SchemaSettings::draft2020_12()
        .with_transform(drop_null as fn(&mut Schema));
    let mut schema = settings.into_generator().into_root_schema_for::<RuleFile>();
    schema.insert("$id".into(), SCHEMA_URL.into());
    let mut text = serde_json::to_string_pretty(&schema).unwrap_or_default();
    text.push('\n');
    text
}

/// Takes `null` out of a schema and its subschemas: TOML has no null, so
/// an optional key is one that is left out.
fn drop_null(schema: &mut Schema) {
    if let Some(object) = schema.as_object_mut() {
        if let Some(serde_json::Value::Array(types)) = object.get_mut("type") {
            types.retain(|t| t != "null");
            if let [only] = types.as_slice() {
                let only = only.clone();
                object.insert("type".into(), only);
            }
        }
        if let Some(serde_json::Value::Array(any)) = object.get_mut("anyOf") {
            any.retain(|s| s.get("type").is_none_or(|t| t != "null"));
            if let [serde_json::Value::Object(only)] = any.as_slice() {
                let only = only.clone();
                object.remove("anyOf");
                for (key, value) in only {
                    object.entry(key).or_insert(value);
                }
            }
        }
    }
    schemars::transform::transform_subschemas(&mut drop_null, schema);
}

/// A parsed rule file: each rule with the line its table starts on, or the
/// file's one error, as `LINE: message`.
pub(crate) fn parse(code: &str) -> Result<Vec<(usize, RuleDecl)>, (usize, String)> {
    let file: RuleFile = toml::from_str(code).map_err(|e| {
        let line = e.span().map_or(1, |s| line_of(code, s.start));
        (line, e.message().trim().to_string())
    })?;
    Ok(file
        .rule
        .into_iter()
        .map(|spanned| {
            let line = line_of(code, spanned.span().start);
            (line, spanned.into_inner())
        })
        .collect())
}

/// The 1-based line of a byte offset.
fn line_of(code: &str, offset: usize) -> usize {
    code.get(..offset)
        .unwrap_or(code)
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

#[cfg(test)]
mod tests;
