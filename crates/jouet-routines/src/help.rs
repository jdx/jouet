//! `joue NAME --help`, rendered from the routine's header and usage spec.
//! usage-lib can render help itself, but only through a template engine
//! that would add a megabyte and a half to the binary for one page.

use crate::routine::Routine;
use std::fmt::Write as _;
use usage::Spec;

/// Where descriptions wrap.
const WIDTH: usize = 80;

pub(crate) fn render(routine: &Routine, spec: &Spec) -> String {
    let mut page = routine.about.clone();
    page.push('\n');
    if let Some(help) = &routine.help {
        page.push('\n');
        page.push_str(&wrap(&help.replace('\n', " "), 0, WIDTH));
    }
    let cmd = &spec.cmd;
    let _ = write!(page, "\nUsage: joue {}", routine.name);
    if !cmd.flags.is_empty() {
        page.push_str(" [FLAGS]");
    }
    for arg in &cmd.args {
        let _ = write!(page, " {}", arg.usage);
    }
    page.push('\n');
    let args: Vec<(String, &str)> = cmd
        .args
        .iter()
        .map(|a| (a.usage.clone(), a.help.as_deref().unwrap_or_default()))
        .collect();
    let mut flags: Vec<(String, &str)> = cmd
        .flags
        .iter()
        .filter(|f| !f.hide)
        .map(|f| {
            let shown = if f.short.is_empty() {
                format!("    {}", f.usage)
            } else {
                f.usage.clone()
            };
            (shown, f.help.as_deref().unwrap_or_default())
        })
        .collect();
    flags.push(("-h, --help".to_string(), "Print help"));
    let width = args
        .iter()
        .chain(&flags)
        .map(|(name, _)| name.chars().count())
        .max()
        .unwrap_or(0);
    for (title, rows) in [("Arguments", &args), ("Flags", &flags)] {
        if rows.is_empty() {
            continue;
        }
        let _ = write!(page, "\n{title}:\n");
        for (name, help) in rows {
            let pad = width - name.chars().count();
            let _ = write!(page, "  {name}{}  ", " ".repeat(pad));
            page.push_str(wrap(help, width + 4, WIDTH).trim_start());
        }
    }
    page
}

/// `text` wrapped at `width` columns, every line indented by `indent`, with
/// a newline at the end.
fn wrap(text: &str, indent: usize, width: usize) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && indent + line.len() + 1 + word.len() > width {
            let _ = writeln!(out, "{}{line}", " ".repeat(indent));
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    let _ = writeln!(out, "{}{line}", " ".repeat(indent));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_wrap_under_their_column() {
        assert_eq!(
            wrap("one two three four", 4, 14),
            "    one two\n    three four\n"
        );
        assert_eq!(wrap("", 2, 10), "  \n");
    }
}
