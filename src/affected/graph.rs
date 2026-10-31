//! A graph of which file imports which, walked backwards from the changed
//! files to find the tests that depend on them and the import chain that
//! connects each one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Files and the files each one imports, by path from the repository root.
#[derive(Debug, Default)]
pub(crate) struct Graph {
    /// For each file, the files that import it.
    importers: BTreeMap<String, BTreeSet<String>>,
}

impl Graph {
    pub(crate) fn add(&mut self, file: &str, imports: impl IntoIterator<Item = String>) {
        for import in imports {
            if import != file {
                self.importers
                    .entry(import)
                    .or_default()
                    .insert(file.to_string());
            }
        }
    }

    /// Every file that depends on one of `changed`, directly or through
    /// other files, with the shortest chain of imports from a changed file
    /// to it: the changed file first, the dependent last. A changed file is
    /// its own chain.
    pub(crate) fn dependents(&self, changed: &[String]) -> BTreeMap<String, Vec<String>> {
        let mut parent: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut queue = VecDeque::new();
        for file in changed {
            if parent.insert(file.clone(), None).is_none() {
                queue.push_back(file.clone());
            }
        }
        while let Some(file) = queue.pop_front() {
            for importer in self.importers.get(&file).into_iter().flatten() {
                if !parent.contains_key(importer) {
                    parent.insert(importer.clone(), Some(file.clone()));
                    queue.push_back(importer.clone());
                }
            }
        }
        parent
            .keys()
            .map(|file| {
                let mut chain = vec![file.clone()];
                let mut at = file;
                while let Some(Some(up)) = parent.get(at) {
                    chain.push(up.clone());
                    at = up;
                }
                chain.reverse();
                (file.clone(), chain)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn finds_dependents_through_chains_by_the_shortest_path() {
        let mut g = Graph::default();
        g.add("calc.py", s(&["math.py"]));
        g.add("test_calc.py", s(&["calc.py"]));
        g.add("test_math.py", s(&["math.py", "calc.py"]));
        g.add("test_other.py", s(&["other.py"]));
        // A cycle must not loop.
        g.add("math.py", s(&["calc.py"]));
        let found = g.dependents(&s(&["math.py"]));
        assert_eq!(
            found["test_calc.py"],
            s(&["math.py", "calc.py", "test_calc.py"])
        );
        assert_eq!(found["test_math.py"], s(&["math.py", "test_math.py"]));
        assert_eq!(found["math.py"], s(&["math.py"]));
        assert!(!found.contains_key("test_other.py"));
    }
}
