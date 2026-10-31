//! `jouet.pitchfork.daemons()`: the daemons a project declares in its
//! pitchfork config, read from the files themselves, so no pitchfork runs
//! in a hook.
//!
//! pitchfork reads `pitchfork.toml`, `pitchfork.local.toml`,
//! `.config/pitchfork.toml` and `.config/pitchfork.local.toml` in every
//! directory from the filesystem root down to the current one, a nearer or
//! later file overriding an earlier one's daemon of the same name.

use std::collections::BTreeMap;
use std::path::Path;

/// The config files in one directory, in the order pitchfork reads them.
const FILES: &[&str] = &[
    ".config/pitchfork.toml",
    ".config/pitchfork.local.toml",
    "pitchfork.toml",
    "pitchfork.local.toml",
];

/// The daemons declared for `dir`: `{name, run, ports, file}`
/// each, sorted by name; `None` when no config declares any.
pub(crate) fn daemons(dir: &Path) -> Result<Option<serde_json::Value>, String> {
    let mut found: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let dirs: Vec<&Path> = dir.ancestors().collect();
    for dir in dirs.into_iter().rev() {
        for name in FILES {
            let path = dir.join(name);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let doc: toml::Table = toml::from_str(&text)
                .map_err(|e| format!("reading {}: {}", path.display(), e.message()))?;
            let Some(daemons) = doc.get("daemons").and_then(toml::Value::as_table) else {
                continue;
            };
            for (id, daemon) in daemons {
                let Some(run) = daemon.get("run").and_then(run_text) else {
                    continue;
                };
                let mut ports = ports(daemon);
                for port in run_ports(&run) {
                    if !ports.contains(&port) {
                        ports.push(port);
                    }
                }
                found.insert(
                    id.clone(),
                    serde_json::json!({
                        "name": id,
                        "run": run,
                        "ports": ports,
                        "file": path.display().to_string(),
                    }),
                );
            }
        }
    }
    if found.is_empty() {
        return Ok(None);
    }
    Ok(Some(serde_json::Value::Array(
        found.into_values().collect(),
    )))
}

/// `run`, a command line or an argv array, as one line.
fn run_text(run: &toml::Value) -> Option<String> {
    match run {
        toml::Value::String(s) => Some(s.trim().to_string()),
        toml::Value::Array(words) => {
            let words: Option<Vec<&str>> = words.iter().map(toml::Value::as_str).collect();
            Some(words?.join(" "))
        }
        _ => None,
    }
}

/// The ports its `run` command gives with `--port N`, `--port=N` or `-p N`.
fn run_ports(run: &str) -> Vec<u16> {
    let words: Vec<&str> = run.split_whitespace().collect();
    words
        .iter()
        .enumerate()
        .filter_map(|(i, w)| match w.strip_prefix("--port=") {
            Some(value) => Some(value),
            None if *w == "--port" || *w == "-p" => words.get(i + 1).copied(),
            None => None,
        })
        .filter_map(|v| v.parse::<u16>().ok().filter(|p| *p > 0))
        .collect()
}

/// The ports its config says a daemon listens on: `port` (a number, a list or
/// `{ expect = [...] }`), `expected_port` and `ready_port` (a number or
/// `{ port = N }`). Templates are left out.
fn ports(daemon: &toml::Value) -> Vec<u16> {
    let mut out = Vec::new();
    let mut add = |v: &toml::Value| {
        if let Some(p) = v.as_integer().and_then(|p| u16::try_from(p).ok())
            && p > 0
            && !out.contains(&p)
        {
            out.push(p);
        }
    };
    let list = |v: &toml::Value| -> Vec<toml::Value> {
        match v {
            toml::Value::Array(items) => items.clone(),
            other => vec![other.clone()],
        }
    };
    if let Some(port) = daemon.get("port") {
        match port.get("expect") {
            Some(expect) => list(expect).iter().for_each(&mut add),
            None => list(port).iter().for_each(&mut add),
        }
    }
    if let Some(expected) = daemon.get("expected_port") {
        list(expected).iter().for_each(&mut add);
    }
    if let Some(ready) = daemon.get("ready_port") {
        match ready.get("port") {
            Some(port) => add(port),
            None => add(ready),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_config_down_to_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("shop");
        let web = project.join("web");
        std::fs::create_dir_all(web.join(".config")).unwrap();
        std::fs::write(
            project.join("pitchfork.toml"),
            "[daemons.api]\nrun = \"npm run api\"\nport = { expect = [4000], bump = 3 }\nready_port = 4000\n\n[daemons.db]\nrun = [\"postgres\", \"-D\", \"data\"]\nexpected_port = [5432]\n",
        )
        .unwrap();
        std::fs::write(
            web.join(".config/pitchfork.toml"),
            "namespace = \"front\"\n[daemons.web]\nrun = \"  npm run dev  \"\nport = [5173, 24678]\nready_port = { port = 5173, timeout = \"30s\" }\n[daemons.api]\nrun = \"npm run api -- --mock --port=4100\"\n",
        )
        .unwrap();
        let found = daemons(&web).unwrap().unwrap();
        let names: Vec<&str> = found
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["api", "db", "web"]);
        assert_eq!(found[0]["run"], "npm run api -- --mock --port=4100");
        assert_eq!(found[0]["ports"], serde_json::json!([4100]));
        assert_eq!(found[1]["run"], "postgres -D data");
        assert_eq!(found[1]["ports"], serde_json::json!([5432]));
        assert_eq!(found[2]["run"], "npm run dev");
        assert_eq!(found[2]["ports"], serde_json::json!([5173, 24678]));
        assert_eq!(daemons(tmp.path()).unwrap(), None);
    }

    #[test]
    fn a_broken_config_says_where() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pitchfork.toml"), "[daemons\n").unwrap();
        let e = daemons(tmp.path()).unwrap_err();
        assert!(e.contains("pitchfork.toml"), "{e}");
    }
}
