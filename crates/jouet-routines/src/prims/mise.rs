//! `mise`: the project's resolved config and tasks, through the `mise` on
//! PATH in the work tree, and running its tasks.
//!
//! - `mise.tasks()`, `mise.config()`, `mise.tools()` and `mise.env()`:
//!   `mise tasks ls`, `mise config ls`, `mise ls --current` and `mise env`,
//!   as JSON (`mise.read`).
//! - `mise.run(task, {args})`: `mise run TASK -- ARGS...` (`mise.run`).
//! - `mise.test()`: the project's test command, by the first of these it
//!   has: a mise `test` task, a package.json `test` script, Cargo.toml, a
//!   justfile or Makefile `test` target, go.mod (`mise.run`).
//!
//! A task's output goes to a log; the result is `{ ok, exit_code, command,
//! duration_ms, log, tail }`, where `tail` is its last lines.

use super::{primitive, strings};
use crate::Fail;
use crate::capability::Capability::{MiseRead, MiseRun};
use crate::runtime::{Ctx, to_lua};
use jouet_output::exit::Exit;
use mlua::{Lua, Table};
use serde_json::{Value, json};
use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Instant;

/// Lines of a task's output kept in its result.
const TAIL_LINES: usize = 30;

fn failed(message: String) -> mlua::Error {
    Fail::new(Exit::Failed, message).raise()
}

/// A test's recorded answer under `key` (and `sub`, for `run`), when the
/// run is a test's.
fn recorded(ctx: &Ctx, key: &str, sub: Option<&str>) -> mlua::Result<Option<Value>> {
    let world = ctx.world.borrow();
    let Some(recorded) = &world.recorded else {
        return Ok(None);
    };
    let found = recorded.mise.get(key).and_then(|v| match sub {
        Some(sub) => v.get(sub).cloned(),
        None => Some(v.clone()),
    });
    let name = sub.map_or_else(|| key.to_string(), |s| format!("{key}.{s}"));
    found
        .map(Some)
        .ok_or_else(|| failed(format!("the fixture records no answer for mise.{name}")))
}

/// `mise ARGS` with JSON output, in the work tree.
fn read_json(ctx: &Ctx, key: &str, args: &[&str]) -> mlua::Result<Value> {
    if let Some(value) = recorded(ctx, key, None)? {
        return Ok(value);
    }
    let root = ctx.world.borrow().root.clone();
    let out = Command::new("mise")
        .args(args)
        .current_dir(&root)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| failed(format!("running mise: {e}")))?;
    if !out.status.success() {
        return Err(failed(format!(
            "mise {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    serde_json::from_slice(&out.stdout)
        .map_err(|e| failed(format!("mise {} printed no JSON: {e}", args.join(" "))))
}

/// Runs a command in the work tree with its output in a log, and reports
/// how it ended with the end of the log.
fn run_logged(ctx: &Ctx, command: &[String]) -> mlua::Result<Value> {
    let (root, log_dir) = {
        let world = ctx.world.borrow();
        (world.root.clone(), world.log_dir.clone())
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let log = log_dir.join(format!("routine-task-{stamp}-{}.log", std::process::id()));
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| failed(format!("creating {}: {e}", log_dir.display())))?;
    let file = std::fs::File::create(&log)
        .map_err(|e| failed(format!("creating {}: {e}", log.display())))?;
    let err = file.try_clone().map_err(|e| failed(e.to_string()))?;
    eprintln!("joue: running {}", command.join(" "));
    let started = Instant::now();
    let status = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stdout(file)
        .stderr(err)
        .status()
        .map_err(|e| failed(format!("running {}: {e}", command[0])))?;
    let exit_code = status.code().unwrap_or(1);
    Ok(json!({
        "ok": exit_code == 0,
        "command": command,
        "exit_code": exit_code,
        "duration_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "log": log.display().to_string(),
        "tail": tail(&log, TAIL_LINES),
    }))
}

/// The last `n` lines of a file.
fn tail(path: &Path, n: usize) -> Vec<String> {
    let mut text = String::new();
    let _ = std::fs::File::open(path).and_then(|mut f| f.read_to_string(&mut text));
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| (*l).to_string())
        .collect()
}

pub(super) fn install(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let mise = lua.create_table()?;
    let reads: [(&str, &[&str]); 4] = [
        ("tasks", &["tasks", "ls", "--json"]),
        ("config", &["config", "ls", "--json"]),
        ("tools", &["ls", "--current", "--json"]),
        ("env", &["env", "--json"]),
    ];
    for (key, args) in reads {
        mise.set(
            key,
            primitive(
                lua,
                ctx,
                Some(MiseRead),
                format!("mise.{key}"),
                move |lua, ctx, ()| to_lua(lua, &read_json(ctx, key, args)?),
            )?,
        )?;
    }
    mise.set(
        "run",
        primitive(
            lua,
            ctx,
            Some(MiseRun),
            "mise.run",
            |lua, ctx, (task, opts): (String, Option<Table>)| {
                if task.is_empty() || task.starts_with('-') {
                    return Err(failed(format!("mise.run: `{task}` is not a task name")));
                }
                if let Some(value) = recorded(ctx, "run", Some(&task))? {
                    return to_lua(lua, &value);
                }
                let mut command = vec!["mise".to_string(), "run".into(), task];
                let args = strings(opts.as_ref(), "args", "mise.run")?;
                if !args.is_empty() {
                    command.push("--".into());
                    command.extend(args);
                }
                to_lua(lua, &run_logged(ctx, &command)?)
            },
        )?,
    )?;
    mise.set(
        "test",
        primitive(lua, ctx, Some(MiseRun), "mise.test", |lua, ctx, ()| {
            if let Some(value) = recorded(ctx, "test", None)? {
                return to_lua(lua, &value);
            }
            let root = ctx.world.borrow().root.clone();
            let command = test_task(&root).ok_or_else(|| {
                Fail::new(
                    Exit::NotFound,
                    format!(
                        "mise.test: no test command found in {}: looked for a mise `test` task, a package.json `test` script, Cargo.toml, a justfile or Makefile `test` target, and go.mod",
                        root.display()
                    ),
                )
                .raise()
            })?;
            to_lua(lua, &run_logged(ctx, &command)?)
        })?,
    )?;
    globals.set("mise", mise)?;
    Ok(())
}

/// The repository's test command, by the first of these it has: a mise
/// `test` task, a package.json `test` script (run by the package manager
/// its lockfile names), Cargo.toml, a justfile or Makefile `test` target,
/// go.mod.
pub fn test_task(root: &Path) -> Option<Vec<String>> {
    let has = |name: &str| root.join(name).is_file();
    let read = |name: &str| std::fs::read_to_string(root.join(name)).ok();
    let words = |w: &[&str]| Some(w.iter().map(|s| (*s).to_string()).collect());
    let mise_files = [
        "mise.toml",
        ".mise.toml",
        "mise/config.toml",
        ".config/mise.toml",
        ".mise/config.toml",
    ];
    let configs: Vec<toml::Table> = mise_files
        .iter()
        .filter_map(|f| read(f))
        .filter_map(|text| toml::from_str::<toml::Table>(&text).ok())
        .collect();
    let declared = configs.iter().any(|config| {
        config
            .get("tasks")
            .and_then(toml::Value::as_table)
            .is_some_and(|tasks| tasks.contains_key("test"))
    });
    // `[task_config] includes`: TOML files of tasks keyed by name, as
    // mise's own `tasks.toml`, or directories of file tasks.
    let included = configs
        .iter()
        .filter_map(|config| config.get("task_config")?.get("includes")?.as_array())
        .flatten()
        .filter_map(toml::Value::as_str)
        .any(|include| {
            let path = root.join(include);
            if path.is_dir() {
                path.join("test").is_file()
            } else {
                std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
                    .is_some_and(|tasks| tasks.contains_key("test"))
            }
        });
    let mise_task = declared
        || included
        || [
            "mise-tasks/test",
            ".mise-tasks/test",
            ".mise/tasks/test",
            "mise/tasks/test",
        ]
        .iter()
        .any(|f| has(f));
    if mise_task {
        return words(&["mise", "run", "test"]);
    }
    // Not `npm init`'s placeholder, which only fails: `echo "Error: no
    // test specified" && exit 1`.
    let script = read("package.json")
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|pkg| {
            pkg["scripts"]["test"]
                .as_str()
                .is_some_and(|s| !s.contains("no test specified"))
        });
    if script {
        return if has("pnpm-lock.yaml") {
            words(&["pnpm", "test"])
        } else if has("yarn.lock") {
            words(&["yarn", "test"])
        } else if has("bun.lock") || has("bun.lockb") {
            words(&["bun", "run", "test"])
        } else {
            words(&["npm", "test"])
        };
    }
    if has("Cargo.toml") {
        return words(&["cargo", "test"]);
    }
    let target = |names: &[&str]| {
        names.iter().filter_map(|f| read(f)).any(|text| {
            text.lines()
                .any(|l| l.starts_with("test:") || l.starts_with("test "))
        })
    };
    if target(&["justfile", "Justfile", ".justfile"]) {
        return words(&["just", "test"]);
    }
    if target(&["Makefile", "makefile", "GNUmakefile"]) {
        return words(&["make", "test"]);
    }
    if has("go.mod") {
        return words(&["go", "test", "./..."]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_task_is_found_by_what_the_repository_has() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(test_task(root), None);
        std::fs::write(root.join("go.mod"), "module x\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["go", "test", "./..."]);
        std::fs::write(
            root.join("Makefile"),
            "build:\n\ttrue\ntest: build\n\ttrue\n",
        )
        .unwrap();
        assert_eq!(test_task(root).unwrap(), ["make", "test"]);
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["cargo", "test"]);
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"test":"vitest"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(test_task(root).unwrap(), ["pnpm", "test"]);
        std::fs::write(root.join("mise.toml"), "[tasks.lint]\nrun = 'x'\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["pnpm", "test"]);
        std::fs::write(root.join("mise.toml"), "[tasks.test]\nrun = 'x'\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["mise", "run", "test"]);
    }

    #[test]
    fn a_test_task_in_an_included_file_or_directory_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // mise's own layout: npm's placeholder script, and the test task
        // in an included `tasks.toml`.
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["cargo", "test"]);
        std::fs::write(
            root.join("mise.toml"),
            "[task_config]\nincludes = [\"tasks.toml\", \"xtasks\"]\n",
        )
        .unwrap();
        std::fs::write(root.join("tasks.toml"), "[lint]\nrun = 'x'\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["cargo", "test"]);
        std::fs::write(root.join("tasks.toml"), "[test]\nrun = 'x'\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["mise", "run", "test"]);
        // Or a file task in an included directory.
        std::fs::write(root.join("tasks.toml"), "").unwrap();
        std::fs::create_dir(root.join("xtasks")).unwrap();
        std::fs::write(root.join("xtasks/test"), "#!/bin/sh\n").unwrap();
        assert_eq!(test_task(root).unwrap(), ["mise", "run", "test"]);
    }
}
