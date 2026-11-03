//! Whether a run's failing tests lie outside what the change affects: then
//! they were not the change's to break, and the stop gate need not ask for
//! the merge base to prove it, which costs a second run of the whole suite.

use super::{Decision, plan};
use std::collections::BTreeSet;
use std::path::Path;

/// For failing tests `names`, each test with the file that defines it,
/// when every one is defined only in test files of packages the change
/// does not reach: none of them is selected by `jouet test --affected`, nor
/// in a changed file's directory. `None` when any is in doubt: a name the
/// code graph does not define as a test, a suite that runs whole for this
/// change, or a test in a project whose adapter selects by something other
/// than files.
pub(crate) fn unaffected(cwd: &Path, names: &[String]) -> Option<Vec<(String, String)>> {
    if names.is_empty() {
        return None;
    }
    let plan = plan(cwd, None, None, &[]).ok()?;
    if plan
        .suites
        .iter()
        .any(|suite| matches!(suite.decision, Decision::Full(_)))
    {
        return None;
    }
    let graphed: Vec<_> = plan
        .suites
        .iter()
        .filter(|suite| suite.adapter.name() == "graph")
        .collect();
    let first = graphed.first()?;
    // A directory is a Go package, and Go runs a selected test's whole
    // package; for the graph's other languages it is only stricter.
    let reached: BTreeSet<&str> = graphed
        .iter()
        .flat_map(|suite| suite.tests.iter().map(|test| test.path.as_str()))
        .chain(plan.changes.iter().map(|change| change.path.as_str()))
        .map(parent)
        .collect();
    let graph = jouet_graph::Graph::open(&first.project.root).ok()?;
    let mut found = Vec::new();
    for name in names {
        let leaf = leaf(name)?;
        let defined: Vec<String> = graph
            .symbols(&jouet_graph::SymbolQuery {
                text: Some(leaf.to_string()),
                ..Default::default()
            })
            .ok()?
            .into_iter()
            .filter(|symbol| {
                symbol.name == leaf
                    && matches!(symbol.kind.as_str(), "function" | "method")
                    && jouet_graph::is_test_path(&symbol.path)
            })
            .map(|symbol| symbol.path)
            .collect();
        let [path, ..] = defined.as_slice() else {
            return None;
        };
        for path in &defined {
            // Only a graph suite's selection is by files; a test another
            // adapter owns, a cargo crate's say, is not judged here.
            let owner = plan
                .suites
                .iter()
                .filter(|suite| suite.project.inside(path).is_some())
                .max_by_key(|suite| suite.project.rel.len())?;
            if owner.adapter.name() != "graph" || reached.contains(parent(path)) {
                return None;
            }
        }
        found.push((name.clone(), path.clone()));
    }
    Some(found)
}

/// The function a failure names: the last part of `module::test` or
/// `file.py::Class::test`, without a parameter id (`test[1]`) or a Go
/// subtest (`TestX/case`). `None` for a name that is not an identifier,
/// such as a build error or a test described in words.
fn leaf(name: &str) -> Option<&str> {
    let last = name.rsplit("::").next()?;
    let last = last.split_once('[').map_or(last, |(head, _)| head);
    let last = last.split_once('/').map_or(last, |(head, _)| head);
    let ok = !last.is_empty()
        && !last.starts_with(|c: char| c.is_ascii_digit())
        && last.chars().all(|c| c.is_alphanumeric() || c == '_');
    ok.then_some(last)
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

#[cfg(test)]
mod tests {
    use super::leaf;

    #[test]
    fn a_failure_names_its_test_function() {
        assert_eq!(
            leaf("TestSearchCmd_CodeFlag"),
            Some("TestSearchCmd_CodeFlag")
        );
        assert_eq!(leaf("TestSearch/empty_query"), Some("TestSearch"));
        assert_eq!(leaf("tests/test_a.py::TestA::test_b[1-2]"), Some("test_b"));
        assert_eq!(leaf("cli::tests::parses"), Some("parses"));
        assert_eq!(leaf("build github.com/x/y"), None);
        assert_eq!(leaf("Foo > does bar"), None);
        assert_eq!(leaf("main.go:12:3"), None);
    }
}
