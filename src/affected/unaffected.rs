//! Whether a run's failing tests lie outside what the change affects: then
//! they were not the change's to break, and the stop gate need not ask for
//! the merge base to prove it, which costs a second run of the whole suite.

use super::{Decision, plan};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// A failing test, by the name its runner gave it.
pub(crate) struct Failing {
    pub name: String,
    /// The import paths of the packages `go test` failed it in, for a Go
    /// failure: only a definition in one of them is this test.
    pub go_packages: Option<BTreeSet<String>>,
}

/// For failing tests `failing`, each test with the file that defines it,
/// when every one is defined only in test files of packages the change
/// does not reach: none of them is selected by `jouet test --affected`, nor
/// in a changed file's directory. `None` when any is in doubt: a name the
/// code graph does not define as a test, a suite that runs whole for this
/// change, a name defined more often than the graph lists at once, or a
/// test in a project whose adapter selects by something other than files.
pub(crate) fn unaffected(cwd: &Path, failing: &[Failing]) -> Option<Vec<(String, String)>> {
    if failing.is_empty() {
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
    let mut modules: HashMap<&Path, Option<String>> = HashMap::new();
    let mut found = Vec::new();
    for test in failing {
        let leaf = leaf(&test.name)?;
        let symbols = graph
            .symbols(&jouet_graph::SymbolQuery {
                text: Some(leaf.to_string()),
                ..Default::default()
            })
            .ok()?;
        // A full page may leave out the definition that is this test.
        if symbols.len() >= jouet_graph::MAX_RESULTS {
            return None;
        }
        let mut defined = Vec::new();
        for symbol in symbols {
            if symbol.name != leaf
                || !matches!(symbol.kind.as_str(), "function" | "method")
                || !jouet_graph::is_test_path(&symbol.path)
            {
                continue;
            }
            // Only a graph suite's selection is by files; a test another
            // adapter owns, a cargo crate's say, is not judged here.
            let owner = plan
                .suites
                .iter()
                .filter(|suite| suite.project.inside(&symbol.path).is_some())
                .max_by_key(|suite| suite.project.rel.len())?;
            if owner.adapter.name() != "graph" {
                return None;
            }
            if let Some(packages) = &test.go_packages {
                let module = modules
                    .entry(owner.project.dir.as_path())
                    .or_insert_with(|| go_module(&owner.project.dir))
                    .as_deref()?;
                let dir = parent(owner.project.inside(&symbol.path)?);
                let package = if dir.is_empty() {
                    module.to_string()
                } else {
                    format!("{module}/{dir}")
                };
                if !packages.contains(&package) {
                    continue;
                }
            }
            defined.push(symbol.path);
        }
        let [path, ..] = defined.as_slice() else {
            return None;
        };
        if defined.iter().any(|path| reached.contains(parent(path))) {
            return None;
        }
        found.push((test.name.clone(), path.clone()));
    }
    Some(found)
}

/// The module path a `go.mod` in `dir` declares.
fn go_module(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("go.mod")).ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("module "))
        .map(|module| module.trim().trim_matches('"').to_string())
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
