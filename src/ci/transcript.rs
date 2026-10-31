//! Reading the transcript `jactionlint run` prints: `[job]` blocks with
//! their `[step N]` lines and output, then a `Summary` with one line per
//! job. The summary is the record of what ran; the blocks say which step
//! failed and what it printed.

use serde::Serialize;

const RESULTS: [&str; 4] = ["success", "failure", "cancelled", "skipped"];

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Job {
    pub(crate) name: String,
    /// `success`, `failure`, `cancelled` or `skipped`.
    pub(crate) result: String,
    /// Why it was skipped or failed without running, such as `runs-on
    /// macos-latest needs macOS, this machine is Linux`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) failed_steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Step {
    /// `step 2`, `step 3.1` for a step of the composite action of step 3,
    /// or `post 3`.
    pub(crate) label: String,
    pub(crate) name: String,
    /// What ended it, such as `exit status 1` or `timed out after 5
    /// minutes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    /// What jactionlint suggests, ending with the `--explain` pointer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) hint: Option<String>,
    /// The end of its output, and earlier lines that look like errors.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) excerpt: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) excerpt_truncated: bool,
    /// All of its output, as the transcript has it.
    #[serde(skip)]
    pub(crate) output: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Transcript {
    /// In the order of the summary, or of the blocks when there is none.
    pub(crate) jobs: Vec<Job>,
    /// `warning:` lines of the run itself, such as a secret not given.
    pub(crate) warnings: Vec<String>,
    /// The IDs of every `(jactionlint run --explain ID)` pointer.
    pub(crate) explain: Vec<String>,
}

/// One `[job]` block while it is read.
struct Block {
    name: String,
    /// Steps that have started and not ended, outermost first.
    open: Vec<Step>,
    failed: Vec<Step>,
}

pub(crate) fn parse(text: &str) -> Transcript {
    let mut t = Transcript::default();
    let mut blocks: Vec<Job> = Vec::new();
    let mut block: Option<Block> = None;
    let mut summary: Option<Vec<Job>> = None;
    for raw in text.lines() {
        let line = strip_ansi(raw);
        let line = line.trim_end();
        note_explain(&mut t.explain, line);
        if let Some(jobs) = &mut summary {
            if let Some(job) = summary_line(line) {
                jobs.push(job);
            }
            continue;
        }
        let indent = (line.len() - line.trim_start_matches(' ').len()) / 2;
        let body = line.trim_start_matches(' ');
        if indent == 0 {
            if body == "Summary" {
                summary = Some(Vec::new());
            } else if let Some(rest) = body.strip_prefix("[job] ") {
                if let Some((name, result, note)) = ended(rest) {
                    let failed = match block.take() {
                        Some(b) if b.name == name => b.failed,
                        other => {
                            block = other;
                            Vec::new()
                        }
                    };
                    blocks.push(Job {
                        name: name.to_string(),
                        result,
                        note,
                        failed_steps: failed,
                    });
                } else {
                    block = Some(Block {
                        name: rest.to_string(),
                        open: Vec::new(),
                        failed: Vec::new(),
                    });
                }
            } else if let Some(w) = body.strip_prefix("warning: ") {
                t.warnings.push(w.to_string());
            }
            continue;
        }
        let Some(b) = &mut block else {
            continue;
        };
        if let Some((label, rest)) = step_header(body)
            && indent == label_indent(label)
        {
            step_line(b, label, rest);
            continue;
        }
        // Output belongs to the innermost open step it is indented under;
        // a hint follows the end of the step it is about.
        if let Some(hint) = body.strip_prefix("hint: ")
            && let Some(last) = b.failed.last_mut()
            && indent == label_indent(&last.label) + 1
            && last.hint.is_none()
        {
            last.hint = Some(hint.to_string());
            continue;
        }
        if let Some(step) = b.open.last_mut()
            && indent > label_indent(&step.label)
        {
            let cut = 2 * (label_indent(&step.label) + 1);
            step.output
                .push(line.get(cut..).unwrap_or(body).to_string());
        }
    }
    t.jobs = match summary {
        Some(mut jobs) => {
            // The summary has the results; the blocks have the failed steps.
            for job in &mut jobs {
                if let Some(i) = blocks.iter().position(|b| b.name == job.name) {
                    job.failed_steps = blocks.remove(i).failed_steps;
                }
            }
            jobs
        }
        None => blocks,
    };
    t
}

fn step_line(b: &mut Block, label: &str, rest: &str) {
    if let Some((result, note)) = step_end(rest) {
        let Some(i) = b.open.iter().rposition(|s| s.label == label) else {
            return;
        };
        let mut step = b.open.remove(i);
        // Steps of a composite action end before it does.
        b.open.truncate(i);
        let tolerated = note
            .as_deref()
            .is_some_and(|n| n.contains("continue-on-error"));
        if matches!(result.as_str(), "failure" | "cancelled") && !tolerated {
            step.note = note;
            // A composite action fails because one of its steps did, which
            // is already recorded; keep the step that says why.
            let inner = format!("{label}.");
            if !b.failed.iter().any(|f| f.label.starts_with(&inner)) {
                b.failed.push(step);
            }
        }
        return;
    }
    if rest.ends_with(')') && rest.contains(": skipped (") {
        return;
    }
    b.open.push(Step {
        label: label.to_string(),
        name: rest.to_string(),
        note: None,
        hint: None,
        excerpt: Vec::new(),
        excerpt_truncated: false,
        output: Vec::new(),
    });
}

/// `[step 2] Run make` as (`step 2`, `Run make`).
fn step_header(body: &str) -> Option<(&str, &str)> {
    let rest = body.strip_prefix('[')?;
    let (label, rest) = rest.split_once("] ")?;
    let (kind, number) = label.split_once(' ')?;
    let numbered = !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    (matches!(kind, "step" | "post") && numbered).then_some((label, rest))
}

/// The indent of a step's header: one level inside its job, and one more
/// for each composite action it is nested in.
fn label_indent(label: &str) -> usize {
    1 + label.matches('.').count()
}

/// `failure (exit status 1) [2.0s]` as (`failure`, `exit status 1`).
fn step_end(rest: &str) -> Option<(String, Option<String>)> {
    let rest = strip_timing(rest);
    let (result, note) = match rest.split_once(' ') {
        Some((r, n)) => (r, Some(n.strip_prefix('(')?.strip_suffix(')')?.to_string())),
        None => (rest, None),
    };
    RESULTS
        .contains(&result)
        .then(|| (result.to_string(), note))
}

/// `build: failure (bad if) [1.0s]` as (`build`, `failure`, `bad if`).
fn ended(rest: &str) -> Option<(&str, String, Option<String>)> {
    let rest = strip_timing(rest);
    // Names and notes can both hold ": ", so the name ends at the first
    // one that a well-formed end follows.
    rest.match_indices(": ").find_map(|(i, _)| {
        step_end(&rest[i + 2..]).map(|(result, note)| (&rest[..i], result, note))
    })
}

fn strip_timing(s: &str) -> &str {
    match s.rsplit_once(" [") {
        Some((head, t)) if t.ends_with("s]") && t[..t.len() - 2].parse::<f64>().is_ok() => head,
        _ => s,
    }
}

/// `  failure   first  (a job it needs did not succeed)`.
fn summary_line(line: &str) -> Option<Job> {
    let body = line.strip_prefix("  ")?;
    let (result, rest) = body.split_once(' ')?;
    if !RESULTS.contains(&result) {
        return None;
    }
    let rest = rest.trim_start();
    let (name, note) = match rest.strip_suffix(')').and_then(|r| r.rsplit_once("  (")) {
        Some((name, note)) => (name, Some(note.to_string())),
        None => (rest, None),
    };
    Some(Job {
        name: name.to_string(),
        result: result.to_string(),
        note,
        failed_steps: Vec::new(),
    })
}

fn note_explain(ids: &mut Vec<String>, line: &str) {
    const POINTER: &str = "(jactionlint run --explain ";
    for (i, _) in line.match_indices(POINTER) {
        let rest = &line[i + POINTER.len()..];
        if let Some((id, _)) = rest.split_once(')')
            && !id.is_empty()
            && !ids.iter().any(|known| known == id)
        {
            ids.push(id.to_string());
        }
    }
}

/// Drops colour escapes, for a jactionlint that ignores `NO_COLOR`.
fn strip_ansi(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\x1b') {
        return s.into();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job<'a>(t: &'a Transcript, name: &str) -> &'a Job {
        t.jobs
            .iter()
            .find(|j| j.name == name)
            .unwrap_or_else(|| panic!("no job {name} in {t:?}"))
    }

    #[test]
    fn a_failed_step_keeps_its_output_and_tolerated_failures_do_not_count() {
        let t = parse(include_str!("fixtures/failure.txt"));
        let names: Vec<_> = t
            .jobs
            .iter()
            .map(|j| (j.name.as_str(), j.result.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("first", "failure"),
                ("tolerated", "success"),
                ("after-success", "success"),
                ("after-failure", "skipped"),
                ("only-on-failure", "success"),
                ("cleanup", "success"),
            ]
        );
        let first = job(&t, "first");
        assert_eq!(first.failed_steps.len(), 1, "{first:?}");
        let step = &first.failed_steps[0];
        assert_eq!(step.label, "step 2");
        assert_eq!(step.name, "Run echo \"about to fail\" >&2");
        assert_eq!(step.note.as_deref(), Some("exit status 7"));
        assert_eq!(step.output, ["about to fail"]);
        assert!(job(&t, "tolerated").failed_steps.is_empty());
        assert_eq!(
            job(&t, "after-failure").note.as_deref(),
            Some("a job it needs did not succeed")
        );
    }

    #[test]
    fn hints_attach_to_their_step_and_explain_ids_are_collected() {
        let t = parse(include_str!("fixtures/sandbox_linux.txt"));
        let deny = job(&t, "deny");
        assert_eq!(deny.failed_steps.len(), 2);
        let first = &deny.failed_steps[0];
        assert_eq!(first.name, "writing outside the workspace is denied");
        assert_eq!(
            first.output,
            ["<RUN>/temp/step-1.sh: line 1: <RUN>/work/../outside: Permission denied"]
        );
        assert!(
            first
                .hint
                .as_deref()
                .is_some_and(|h| h.ends_with("(jactionlint run --explain sandbox-denied)")),
            "{first:?}"
        );
        let timeout = &job(&t, "timeout").failed_steps[0];
        assert_eq!(
            timeout.note.as_deref(),
            Some("timed out after 0.01 minutes")
        );
        assert!(timeout.output.is_empty());
        assert_eq!(t.explain, ["sandbox-denied"]);
    }

    #[test]
    fn passing_runs_have_no_failed_steps() {
        for text in [
            include_str!("fixtures/basic.txt"),
            include_str!("fixtures/commands.txt"),
        ] {
            let t = parse(text);
            assert!(!t.jobs.is_empty());
            assert!(
                t.jobs
                    .iter()
                    .all(|j| j.result == "success" && j.failed_steps.is_empty())
            );
        }
        let t = parse(include_str!("fixtures/basic.txt"));
        assert_eq!(job(&t, "test (my-runner, 1)").result, "success");
        // Output of a step that wrote "warning:" is not a warning of the run.
        assert!(
            parse(include_str!("fixtures/commands.txt"))
                .warnings
                .is_empty()
        );
    }

    #[test]
    fn composite_steps_skips_timings_colour_and_run_warnings() {
        let text = "\
warning: secrets.TOKEN is referenced but was not given (jactionlint run --explain secret-unset)
[job] build
  [step 1] Run make [0.1s]
    ok
  [step 1] success [0.1s]
  [step 2] ./.github/actions/setup
    [step 2.1] Run ./install.sh
      fetching
      \x1b[31merror: no such file\x1b[0m
    [step 2.1] \x1b[31mfailure\x1b[0m (exit status 2) [1.5s]
  [step 2] failure
  [step 3] Run echo never: skipped (an earlier step failed)
[job] build: failure [2.0s]
[job] test (macos-latest): skipped (runs-on macos-latest needs macOS, this machine is Linux)

Summary
  failure   build
  skipped   test (macos-latest)  (runs-on macos-latest needs macOS, this machine is Linux)
";
        let t = parse(text);
        assert_eq!(
            t.warnings,
            [
                "secrets.TOKEN is referenced but was not given (jactionlint run --explain secret-unset)"
            ]
        );
        let build = job(&t, "build");
        assert_eq!(build.failed_steps.len(), 1, "{build:?}");
        let step = &build.failed_steps[0];
        assert_eq!(step.label, "step 2.1");
        assert_eq!(step.output, ["fetching", "error: no such file"]);
        assert_eq!(step.note.as_deref(), Some("exit status 2"));
        let mac = job(&t, "test (macos-latest)");
        assert_eq!(mac.result, "skipped");
        assert_eq!(
            mac.note.as_deref(),
            Some("runs-on macos-latest needs macOS, this machine is Linux")
        );
        assert_eq!(t.explain, ["secret-unset"]);
    }

    #[test]
    fn without_a_summary_the_blocks_are_the_record() {
        let t = parse(
            "[job] a\n  [step 1] Run false\n  [step 1] failure (exit status 1)\n[job] a: failure\n",
        );
        assert_eq!(t.jobs.len(), 1);
        assert_eq!(t.jobs[0].failed_steps[0].name, "Run false");
    }

    #[test]
    fn a_job_name_with_a_colon_still_ends() {
        assert_eq!(
            ended("deploy: prod: skipped (if is false)"),
            Some(("deploy: prod", "skipped".into(), Some("if is false".into())))
        );
        assert_eq!(
            ended("lint: failure (bad if: x)"),
            Some(("lint", "failure".into(), Some("bad if: x".into())))
        );
        assert_eq!(ended("build"), None);
    }
}
