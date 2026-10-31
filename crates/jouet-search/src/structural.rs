//! Structural search and rewrites with ast-grep's matcher (`ast-grep-core`,
//! MIT): a pattern is code in the target language, with `$NAME` matching one
//! node and `$$$NAME` any number of them. It shares the grammars symbols use.

/// A match of a pattern, and its rewrite when one was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// The byte range of the matched node.
    pub start: usize,
    pub end: usize,
    /// The 1-based line it starts on.
    pub line: u32,
    /// The text of its first line.
    pub text: String,
    /// What it becomes under `--rewrite`.
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The language name is not one jouet has.
    UnknownLanguage(String),
    /// The pattern or rewrite is not valid code in the language.
    Pattern(String),
    /// This jouet was built without grammars.
    NoGrammars,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownLanguage(name) => write!(
                f,
                "unknown language `{name}`; --lang takes one of {}",
                LANGUAGES.join(", ")
            ),
            Self::Pattern(message) => f.write_str(message),
            Self::NoGrammars => f.write_str(
                "this jouet was built without grammars, so it cannot match code by structure",
            ),
        }
    }
}

/// The names `--lang` takes.
pub const LANGUAGES: &[&str] = &[
    "rust",
    "python",
    "javascript",
    "typescript",
    "tsx",
    "go",
    "ruby",
    "java",
    "c",
    "cpp",
];

pub use imp::Structural;

/// A file the pattern matched: its contents when read, and the matches.
#[derive(Debug)]
pub struct FileMatches {
    pub path: std::path::PathBuf,
    pub source: String,
    pub matches: Vec<Match>,
}

/// Narrows a pattern's matches file by file, as the code graph's scoping
/// does in the binary. It sees every file of the language, matched or not.
pub trait Filter: Sync + std::fmt::Debug {
    /// The matches in `path` to keep, of all those the pattern found there.
    fn keep(
        &self,
        structural: &Structural,
        path: &std::path::Path,
        source: &str,
        matches: Vec<Match>,
    ) -> Vec<Match>;
}

/// Whether `name` occurs in `text` as a whole identifier.
pub fn mentions(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    !name.is_empty()
        && text.match_indices(name).any(|(at, _)| {
            !text[..at].chars().next_back().is_some_and(ident)
                && !text[at + name.len()..].chars().next().is_some_and(ident)
        })
}

impl Structural {
    /// The files under `paths` in the pattern's language that it matches,
    /// in path order, as `.gitignore` and hidden files allow, and how many
    /// files of the language were read. A `filter` narrows each file's
    /// matches.
    pub fn search_files(
        &self,
        paths: &[std::path::PathBuf],
        filter: Option<&dyn Filter>,
    ) -> (Vec<FileMatches>, usize) {
        let mut found = Vec::new();
        let mut read = 0;
        let Some((first, rest)) = paths.split_first() else {
            return (found, read);
        };
        let mut walk = ignore::WalkBuilder::new(first);
        for path in rest {
            walk.add(path);
        }
        for entry in walk.build().flatten() {
            let path = entry.path();
            if !entry.file_type().is_some_and(|t| t.is_file()) || !self.applies_to(path) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            read += 1;
            let mut matches = self.find(&source);
            if let Some(filter) = filter {
                matches = filter.keep(self, path, &source, matches);
            }
            if !matches.is_empty() {
                found.push(FileMatches {
                    path: path.strip_prefix(".").unwrap_or(path).to_path_buf(),
                    source,
                    matches,
                });
            }
        }
        found.sort_by(|a, b| a.path.cmp(&b.path));
        (found, read)
    }
}

#[cfg(feature = "grammars")]
mod imp {
    use super::{Error, Match};
    use crate::symbols::{Lang, language, source_language};
    use ast_grep_core::matcher::PatternBuilder;
    use ast_grep_core::replacer::TemplateFix;
    use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
    use ast_grep_core::{Language, Pattern, PatternError};
    use std::borrow::Cow;
    use std::path::Path;

    /// A grammar as ast-grep sees it. Where `$` cannot start an identifier,
    /// a pattern's metavariables are rewritten to another character before
    /// parsing, as ast-grep's own language crate does.
    #[derive(Clone)]
    struct Grammar {
        lang: Lang,
        ts: TSLanguage,
    }

    impl Grammar {
        fn expando(&self) -> char {
            match self.lang {
                Lang::C | Lang::Cpp => '𐀀',
                Lang::Go | Lang::Python | Lang::Ruby | Lang::Rust => 'µ',
                Lang::Java | Lang::JavaScript | Lang::TypeScript | Lang::Tsx => '$',
            }
        }
    }

    impl Language for Grammar {
        fn kind_to_id(&self, kind: &str) -> u16 {
            self.ts.id_for_node_kind(kind, true)
        }

        fn field_to_id(&self, field: &str) -> Option<u16> {
            self.ts.field_id_for_name(field).map(|f| f.get())
        }

        fn expando_char(&self) -> char {
            self.expando()
        }

        fn pre_process_pattern<'q>(&self, query: &'q str) -> Cow<'q, str> {
            if self.expando() == '$' {
                return Cow::Borrowed(query);
            }
            Cow::Owned(expand(self.expando(), query))
        }

        fn build_pattern(&self, builder: &PatternBuilder) -> Result<Pattern, PatternError> {
            builder.build(|src| StrDoc::try_new(src, self.clone()))
        }
    }

    impl LanguageExt for Grammar {
        fn get_ts_language(&self) -> TSLanguage {
            self.ts.clone()
        }
    }

    /// `$A`, `$$A` and `$$$` with `$` swapped for `expando`, leaving a lone
    /// `$` or `$a` alone. After ast-grep-language's `pre_process_pattern`
    /// (MIT).
    fn expand(expando: char, query: &str) -> String {
        let mut out = String::with_capacity(query.len());
        let mut dollars = 0;
        for c in query.chars() {
            if c == '$' {
                dollars += 1;
                continue;
            }
            let metavariable = c.is_ascii_uppercase() || c == '_' || dollars == 3;
            let sigil = if metavariable { expando } else { '$' };
            out.extend(std::iter::repeat_n(sigil, dollars));
            dollars = 0;
            out.push(c);
        }
        let sigil = if dollars == 3 { expando } else { '$' };
        out.extend(std::iter::repeat_n(sigil, dollars));
        out
    }

    /// A pattern, and a rewrite, compiled for one language.
    pub struct Structural {
        grammar: Grammar,
        pattern: Pattern,
        rewrite: Option<TemplateFix>,
    }

    impl Structural {
        pub fn new(pattern: &str, lang: &str, rewrite: Option<&str>) -> Result<Self, Error> {
            let lang = match lang {
                "rust" | "rs" => Lang::Rust,
                "python" | "py" => Lang::Python,
                "javascript" | "js" | "jsx" => Lang::JavaScript,
                "typescript" | "ts" => Lang::TypeScript,
                "tsx" => Lang::Tsx,
                "go" | "golang" => Lang::Go,
                "ruby" | "rb" => Lang::Ruby,
                "java" => Lang::Java,
                "c" => Lang::C,
                "cpp" | "c++" | "cc" | "cxx" => Lang::Cpp,
                other => return Err(Error::UnknownLanguage(other.to_string())),
            };
            let grammar = Grammar {
                lang,
                ts: source_language(lang),
            };
            let name = super::LANGUAGES[lang_index(lang)];
            // tree-sitter recovers from errors, so a pattern cut short would
            // still compile and match nothing useful. Parse errors cannot
            // tell: an expression alone is an error at the top of a Rust or
            // Go file, but a fine pattern. Unbalanced brackets can.
            if !balanced(pattern) {
                return Err(Error::Pattern(format!(
                    "the pattern `{pattern}` does not parse as {name}: write a complete expression, statement or definition, with `$X` for one node and `$$$X` for any number"
                )));
            }
            let pattern = Pattern::try_new(pattern, grammar.clone()).map_err(|e| {
                Error::Pattern(format!(
                    "the pattern is not valid {name}: {e}; a pattern must be a complete piece of code, such as an expression or a statement"
                ))
            })?;
            let rewrite = rewrite
                .map(|r| TemplateFix::try_new(r, &grammar))
                .transpose()
                .map_err(|e| Error::Pattern(format!("the rewrite is not valid: {e:?}")))?;
            Ok(Self {
                grammar,
                pattern,
                rewrite,
            })
        }

        /// Whether the file is in the pattern's language, judged by its name.
        pub fn applies_to(&self, path: &Path) -> bool {
            language(path) == Some(self.grammar.lang)
        }

        /// Every match in `source`, in order, outermost first where matches
        /// nest.
        pub fn find(&self, source: &str) -> Vec<Match> {
            let root = self.grammar.ast_grep(source);
            root.root()
                .find_all(&self.pattern)
                .map(|m| {
                    let range = m.range();
                    let text = m.text();
                    Match {
                        start: range.start,
                        end: range.end,
                        line: u32::try_from(m.start_pos().line() + 1).unwrap_or(u32::MAX),
                        text: text.lines().next().unwrap_or("").trim().to_string(),
                        replacement: self.rewrite.as_ref().map(|fix| {
                            let edit = m.replace_by(fix);
                            String::from_utf8_lossy(&edit.inserted_text).into_owned()
                        }),
                    }
                })
                .collect()
        }

        /// Where `name` is written inside a macro, which neither a pattern
        /// nor the code graph parses as code: a Rust macro's token tree, or
        /// the body of a C or C++ `#define`.
        pub fn in_macros(&self, source: &str, name: &str) -> Vec<Match> {
            let root = self.grammar.ast_grep(source);
            let mut out = Vec::new();
            for node in root.root().dfs() {
                let hit = match node.kind().as_ref() {
                    "identifier" => {
                        node.text() == name
                            && node.parent().is_some_and(|p| p.kind() == "token_tree")
                    }
                    "preproc_arg" => super::mentions(&node.text(), name),
                    _ => false,
                };
                if !hit {
                    continue;
                }
                let range = node.range();
                let line_start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
                let line_end = source[range.start..]
                    .find('\n')
                    .map_or(source.len(), |i| range.start + i);
                out.push(Match {
                    start: range.start,
                    end: range.end,
                    line: u32::try_from(node.start_pos().line() + 1).unwrap_or(u32::MAX),
                    text: source[line_start..line_end].trim().to_string(),
                    replacement: None,
                });
            }
            out
        }
    }

    /// Whether `()`, `[]` and `{}` pair up outside quoted strings.
    fn balanced(code: &str) -> bool {
        let mut open = Vec::new();
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for c in code.chars() {
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
                continue;
            }
            match c {
                '"' | '\'' | '`' => quote = Some(c),
                '(' | '[' | '{' => open.push(c),
                ')' | ']' | '}' => {
                    let want = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    if open.pop() != Some(want) {
                        return false;
                    }
                }
                _ => {}
            }
        }
        open.is_empty() && quote.is_none()
    }

    fn lang_index(lang: Lang) -> usize {
        match lang {
            Lang::Rust => 0,
            Lang::Python => 1,
            Lang::JavaScript => 2,
            Lang::TypeScript => 3,
            Lang::Tsx => 4,
            Lang::Go => 5,
            Lang::Ruby => 6,
            Lang::Java => 7,
            Lang::C => 8,
            Lang::Cpp => 9,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::expand;

        #[test]
        fn brackets_must_pair_outside_strings() {
            assert!(super::balanced("f($A, [1, 2], { x })"));
            assert!(super::balanced("f(\")(\")"));
            assert!(!super::balanced("fetch($A"));
            assert!(!super::balanced("f(]"));
        }

        #[test]
        fn metavariables_are_expanded_and_other_dollars_kept() {
            assert_eq!(expand('µ', "$A.get($$$ARGS)"), "µA.get(µµµARGS)");
            assert_eq!(expand('µ', "f($$$)"), "f(µµµ)");
            assert_eq!(expand('µ', "\"$5 $a\""), "\"$5 $a\"");
        }
    }
}

#[cfg(not(feature = "grammars"))]
mod imp {
    use super::{Error, Match};
    use std::path::Path;

    /// Without grammars there is nothing to match against.
    pub struct Structural;

    impl Structural {
        pub fn new(_: &str, _: &str, _: Option<&str>) -> Result<Self, Error> {
            Err(Error::NoGrammars)
        }

        pub fn applies_to(&self, _: &Path) -> bool {
            false
        }

        pub fn find(&self, _: &str) -> Vec<Match> {
            Vec::new()
        }

        pub fn in_macros(&self, _: &str, _: &str) -> Vec<Match> {
            Vec::new()
        }
    }
}
