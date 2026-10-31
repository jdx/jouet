//! A small reader of Python imports, for the static import graph behind the
//! pytest adapter when pytest-testmon is not installed.
//!
//! It reads `import a.b` and `from a import b` statements, relative ones
//! included, wherever they are (inside functions and `if TYPE_CHECKING:`
//! too), and skips strings and comments. It cannot see dynamic imports
//! (`importlib.import_module`, `__import__`) or what a plugin loads.

/// An import statement: the module named, and for `from X import a, b` the
/// names imported from it, which may be submodules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Import {
    /// Leading dots of a relative import: 1 for `from . import x`.
    pub level: usize,
    /// The module after the dots; empty in `from . import x`.
    pub module: String,
    /// The names after `import` in a `from` statement.
    pub names: Vec<String>,
}

/// The code of a Python file with strings replaced by `""` and comments
/// removed, as logical lines: a line continued by a backslash or an open
/// bracket is joined to the next.
fn logical_lines(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '"' | '\'' => {
                let triple = i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c;
                let width = if triple { 3 } else { 1 };
                i += width;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if !triple && chars[i] == '\n' {
                        break;
                    }
                    if chars[i] == c
                        && (!triple
                            || (i + 2 < chars.len() && chars[i + 1] == c && chars[i + 2] == c))
                    {
                        i += width;
                        break;
                    }
                    i += 1;
                }
                line.push_str("\"\"");
                continue;
            }
            '\\' if chars.get(i + 1) == Some(&'\n') => {
                line.push(' ');
                i += 2;
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '\n' if depth == 0 => {
                lines.push(std::mem::take(&mut line));
                i += 1;
                continue;
            }
            '\n' => {
                line.push(' ');
                i += 1;
                continue;
            }
            _ => {}
        }
        line.push(c);
        i += 1;
    }
    lines.push(line);
    lines
}

/// Every import statement in a Python file.
pub(crate) fn imports(text: &str) -> Vec<Import> {
    let mut out = Vec::new();
    for line in logical_lines(text) {
        for statement in line.split(';') {
            let statement = statement.trim();
            if let Some(rest) = statement.strip_prefix("import ") {
                for part in rest.split(',') {
                    let module = part.split_whitespace().next().unwrap_or("");
                    if is_dotted(module) {
                        out.push(Import {
                            level: 0,
                            module: module.to_string(),
                            names: Vec::new(),
                        });
                    }
                }
            } else if let Some(rest) = statement.strip_prefix("from ") {
                let Some((from, names)) = rest.split_once(" import ") else {
                    continue;
                };
                let from = from.trim();
                let level = from.chars().take_while(|c| *c == '.').count();
                let module = &from[level..];
                if !module.is_empty() && !is_dotted(module) {
                    continue;
                }
                let names = names
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .split(',')
                    .filter_map(|n| n.split_whitespace().next())
                    .filter(|n| *n != "*" && is_dotted(n))
                    .map(str::to_string)
                    .collect();
                out.push(Import {
                    level,
                    module: module.to_string(),
                    names,
                });
            }
        }
    }
    out
}

fn is_dotted(name: &str) -> bool {
    !name.is_empty()
        && name
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn import(level: usize, module: &str, names: &[&str]) -> Import {
        Import {
            level,
            module: module.into(),
            names: names.iter().map(|n| (*n).to_string()).collect(),
        }
    }

    #[test]
    fn reads_imports_and_skips_strings_and_comments() {
        let text = r#"
import os, app.core.math as m
from app.calc import twice
from . import sibling
from ..pkg.mod import (
    a,
    b as c,  # a comment: import nothing
)
from .x import *
def f():
    import lazy.mod
if TYPE_CHECKING:
    from typing_only import T
x = "import fake"
'''
import also_fake
'''
y = 1; import after_semicolon
from long.line \
    import continued
"#;
        assert_eq!(
            imports(text),
            [
                import(0, "os", &[]),
                import(0, "app.core.math", &[]),
                import(0, "app.calc", &["twice"]),
                import(1, "", &["sibling"]),
                import(2, "pkg.mod", &["a", "b"]),
                import(1, "x", &[]),
                import(0, "lazy.mod", &[]),
                import(0, "typing_only", &["T"]),
                import(0, "after_semicolon", &[]),
                import(0, "long.line", &["continued"]),
            ]
        );
    }
}
