//! Running a routine: a sandboxed Luau VM with the primitives, the `joue`
//! module and `print`, and what the run came to.

use crate::capability::Capability;
use crate::json;
use crate::routine::{Args, Catalog, Routine, State};
use crate::world::World;
use crate::{Fail, prims};
use jouet_github::Client;
use jouet_output::exit::Exit;
use mlua::{Function, Lua, MultiValue, Table, Value, Variadic, VmState};
use std::cell::{Cell, RefCell, RefMut};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How deep `joue.call` may nest.
const MAX_DEPTH: usize = 8;

/// Limits on the Luau between two primitive calls.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub time: Duration,
    /// Luau has no instruction counter; this counts its interrupts, which it
    /// raises at every function call and loop iteration.
    pub interrupts: u32,
    /// Bytes for the whole VM.
    pub memory: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            time: Duration::from_secs(10),
            interrupts: 50_000_000,
            memory: 256 * 1024 * 1024,
        }
    }
}

/// How a run behaves, from `joue`'s flags.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// `--json`: the routine's result is printed as JSON, and `joue.json`
    /// tells it so.
    pub json: bool,
    /// `--yes`: writes are made.
    pub yes: bool,
    /// `--dry-run`: writes are only shown, and that is not an error.
    pub dry_run: bool,
    /// Under `joue test`: a routine that writes runs without examples of its
    /// own (they are what is running), and file writes are always previewed.
    pub testing: bool,
}

/// What a run came to.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The routine's result, as JSON: its first return value.
    pub value: Option<serde_json::Value>,
    /// What it printed.
    pub text: String,
    pub exit_code: i32,
    /// What stopped it, if anything did.
    pub error: Option<Fail>,
    /// Writes it asked for that were not made, without `--yes`.
    pub planned: Vec<String>,
    /// It was refused (exit 4) because it would have written without
    /// `--yes`.
    pub needs_yes: bool,
}

/// What the VM's callbacks share.
pub(crate) struct Ctx {
    pub(crate) world: RefCell<World>,
    pub(crate) options: Options,
    /// The capabilities of the routine running now.
    caps: RefCell<Vec<Capability>>,
    current: RefCell<String>,
    planned: RefCell<Vec<String>>,
    text: RefCell<String>,
    deadline: Cell<Option<Instant>>,
    interrupts: Cell<u32>,
    budget: Budget,
    client: RefCell<Option<Client>>,
    catalog: Catalog,
    depth: Cell<usize>,
}

const SHAPES: &str = "joue.shapes";

impl Ctx {
    /// Refuses a primitive outside the running routine's capabilities.
    pub(crate) fn need(&self, capability: Capability, what: &str) -> mlua::Result<()> {
        if self.caps.borrow().contains(&capability) {
            return Ok(());
        }
        Err(Fail::new(
            Exit::Precondition,
            format!(
                "{what} needs the `{}` capability, which {} does not declare",
                capability.name(),
                self.current.borrow()
            ),
        )
        .hint(format!(
            "add it to the header: `--- capabilities: {}`",
            capability.name()
        ))
        .raise())
    }

    /// Whether a write is made: with `--yes` it is; otherwise it is noted
    /// in the plan and the primitive previews it.
    pub(crate) fn write(&self, description: String) -> bool {
        if self.options.yes {
            return true;
        }
        self.planned.borrow_mut().push(description);
        false
    }

    /// Notes a write that is only ever shown, whatever the flags.
    pub(crate) fn note(&self, description: String) {
        self.planned.borrow_mut().push(description);
    }

    /// Runs a primitive outside the budget, then starts it afresh.
    pub(crate) fn outside<T>(&self, f: impl FnOnce() -> T) -> T {
        self.deadline.set(None);
        let value = f();
        self.restart_budget();
        value
    }

    fn restart_budget(&self) {
        self.interrupts.set(0);
        self.deadline.set(Some(Instant::now() + self.budget.time));
    }

    /// The GitHub client, made on first use.
    pub(crate) fn client(&self) -> RefMut<'_, Client> {
        RefMut::map(self.client.borrow_mut(), |client| {
            client.get_or_insert_with(|| match &self.world.borrow().github {
                crate::world::Github::Env => Client::from_env(),
                crate::world::Github::Fake { url, token } => {
                    Client::from_env().with_api_url(url).with_token(token)
                }
            })
        })
    }
}

/// A JSON value as Luau sees it, with its lists marked as arrays.
pub(crate) fn to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
    let shapes: Table = lua.named_registry_value(SHAPES)?;
    json::to_lua(lua, &shapes, value)
}

/// A Luau value as JSON.
pub(crate) fn from_lua(lua: &Lua, value: &Value) -> mlua::Result<serde_json::Value> {
    let shapes: Table = lua.named_registry_value(SHAPES)?;
    json::from_lua(&shapes, value).map_err(mlua::Error::runtime)
}

/// Runs `routine` with its parsed arguments. `catalog` holds the routines
/// `joue.call` may run.
pub fn run(
    catalog: &Catalog,
    routine: &Routine,
    args: Args,
    world: World,
    options: Options,
) -> Outcome {
    run_with(catalog, routine, args, world, options, Budget::default())
}

pub(crate) fn run_with(
    catalog: &Catalog,
    routine: &Routine,
    args: Args,
    world: World,
    options: Options,
    budget: Budget,
) -> Outcome {
    let mut outcome = Outcome {
        exit_code: 1,
        ..Outcome::default()
    };
    if let Err(fail) = ready(routine) {
        outcome.exit_code = fail.exit.code().into();
        outcome.error = Some(fail);
        return outcome;
    }
    let ctx = new_ctx(catalog, routine, world, options, budget);
    let result = vm(&ctx).and_then(|lua| {
        let table = load(&lua, &ctx, routine)?;
        if routine.writes() && !options.testing && !has_examples(&table)? {
            return Err(Fail::new(
                Exit::Precondition,
                format!(
                    "{} writes, so it needs examples that `joue test` can run",
                    routine.name
                ),
            )
            .hint("add an `examples` list with recorded responses to the table it returns")
            .raise());
        }
        let args = to_lua(&lua, &serde_json::Value::Object(args.values))?;
        let returned = call_run(&lua, &ctx, &table, args)?;
        Ok((returned.0, returned.1))
    });
    ctx.deadline.set(None);
    outcome.text = ctx.text.take();
    outcome.planned = ctx.planned.take();
    match result {
        Ok((value, code)) => {
            outcome.value = value;
            outcome.exit_code = code.unwrap_or(0);
        }
        Err(e) => {
            let fail = fail_of(&e).unwrap_or_else(|| {
                Fail::new(
                    Exit::Failed,
                    format!("{}: {}", routine.name, first_line(&e.to_string())),
                )
            });
            outcome.exit_code = fail.exit.code().into();
            outcome.error = Some(fail);
            return outcome;
        }
    }
    if !outcome.planned.is_empty() && !options.yes && !options.dry_run {
        outcome.needs_yes = true;
        outcome.exit_code = Exit::Precondition.code().into();
    }
    outcome
}

fn new_ctx(
    catalog: &Catalog,
    routine: &Routine,
    world: World,
    options: Options,
    budget: Budget,
) -> Rc<Ctx> {
    Rc::new(Ctx {
        world: RefCell::new(world),
        options,
        caps: RefCell::new(routine.capabilities.clone()),
        current: RefCell::new(routine.name.clone()),
        planned: RefCell::new(Vec::new()),
        text: RefCell::new(String::new()),
        deadline: Cell::new(None),
        interrupts: Cell::new(0),
        budget,
        client: RefCell::new(None),
        catalog: catalog.clone(),
        depth: Cell::new(0),
    })
}

/// A routine's `examples`, as JSON, read by loading its file without
/// running it.
pub(crate) fn examples(
    catalog: &Catalog,
    routine: &Routine,
) -> Result<Vec<serde_json::Value>, Fail> {
    ready(routine)?;
    let world = World {
        root: std::env::current_dir().unwrap_or_default(),
        host: Box::new(Nowhere),
        github: crate::world::Github::Fake {
            url: "http://127.0.0.1:9".into(),
            token: "none".into(),
        },
        ledger: None,
        repo_key: None,
        log_dir: std::env::temp_dir(),
        recorded: Some(crate::world::Recorded::default()),
    };
    let options = Options {
        testing: true,
        ..Options::default()
    };
    let ctx = new_ctx(catalog, routine, world, options, Budget::default());
    let read = vm(&ctx).and_then(|lua| {
        let table = load(&lua, &ctx, routine)?;
        let examples: Value = table.raw_get("examples")?;
        from_lua(&lua, &examples)
    });
    match read {
        Ok(serde_json::Value::Array(examples)) => Ok(examples),
        Ok(serde_json::Value::Null) => Ok(Vec::new()),
        Ok(serde_json::Value::Object(o)) if o.is_empty() => Ok(Vec::new()),
        Ok(_) => Err(Fail::new(
            Exit::Failed,
            format!("{}: `examples` must be a list", routine.name),
        )),
        Err(e) => Err(fail_of(&e).unwrap_or_else(|| {
            Fail::new(
                Exit::Failed,
                format!("{}: {}", routine.name, first_line(&e.to_string())),
            )
        })),
    }
}

/// A host for reading a routine's file, where nothing runs.
struct Nowhere;

impl crate::world::Host for Nowhere {
    fn jouet(&mut self, _: &[String]) -> Result<crate::world::StepOutput, String> {
        Err("nothing runs while a routine's examples are read".into())
    }
}

/// Why a routine cannot run, if it cannot.
fn ready(routine: &Routine) -> Result<(), Fail> {
    match &routine.state {
        State::Ready => Ok(()),
        State::Untrusted => Err(Fail::new(
            Exit::Precondition,
            format!("{} is in a repository that is not trusted", routine.name),
        )
        .hint("review .jouet/routines, then run `joue --trust`")),
        State::Broken(e) => Err(Fail::new(Exit::Failed, format!("{}: {e}", routine.name))),
    }
}

/// Starts the VM: the primitives, `joue` and `print`, then the sandbox.
fn vm(ctx: &Rc<Ctx>) -> mlua::Result<Lua> {
    let lua = Lua::new();
    lua.set_memory_limit(ctx.budget.memory)?;
    lua.set_named_registry_value(SHAPES, json::shapes(&lua)?)?;
    let globals = lua.globals();
    prims::install(&lua, &globals, ctx)?;
    install_joue(&lua, &globals, ctx)?;
    let tostring: Function = globals.get("tostring")?;
    let c = ctx.clone();
    globals.set(
        "print",
        lua.create_function(move |_, values: Variadic<Value>| {
            let words = values
                .iter()
                .map(|v| tostring.call::<String>(v.clone()))
                .collect::<mlua::Result<Vec<_>>>()?;
            let mut text = c.text.borrow_mut();
            text.push_str(&words.join("\t"));
            text.push('\n');
            Ok(())
        })?,
    )?;
    // Routines read the outside world only through the primitives.
    globals.set("require", Value::Nil)?;
    let guard = ctx.clone();
    lua.set_interrupt(move |_| {
        let Some(deadline) = guard.deadline.get() else {
            return Ok(VmState::Continue);
        };
        let count = guard.interrupts.get() + 1;
        guard.interrupts.set(count);
        if count > guard.budget.interrupts {
            return Err(mlua::Error::runtime("exceeded its instruction budget"));
        }
        if Instant::now() > deadline {
            return Err(mlua::Error::runtime("exceeded its time budget"));
        }
        Ok(VmState::Continue)
    });
    lua.sandbox(true)?;
    Ok(lua)
}

/// Compiles a routine and runs its file, in an environment of its own,
/// returning the table it returns.
pub(crate) fn load(lua: &Lua, ctx: &Ctx, routine: &Routine) -> mlua::Result<Table> {
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__index", lua.globals())?;
    env.set_metatable(Some(meta))?;
    ctx.restart_budget();
    let value: Value = lua
        .load(&routine.code)
        .set_name(format!("={}", routine.name))
        .set_environment(env)
        .eval()?;
    ctx.deadline.set(None);
    let table = match value {
        Value::Table(table) => table,
        other => {
            return Err(Fail::new(
                Exit::Failed,
                format!(
                    "{}: the file must return a table, not {}",
                    routine.name,
                    other.type_name()
                ),
            )
            .raise());
        }
    };
    let run: Value = table.raw_get("run")?;
    if !run.is_function() {
        return Err(Fail::new(
            Exit::Failed,
            format!(
                "{}: the table it returns needs a `run` function",
                routine.name
            ),
        )
        .raise());
    }
    Ok(table)
}

fn has_examples(table: &Table) -> mlua::Result<bool> {
    Ok(table
        .raw_get::<Option<Table>>("examples")?
        .is_some_and(|t| t.raw_len() > 0))
}

/// Calls `run(args)` under the budget: its result as JSON (a string is
/// printed instead) and its exit code.
fn call_run(
    lua: &Lua,
    ctx: &Ctx,
    table: &Table,
    args: Value,
) -> mlua::Result<(Option<serde_json::Value>, Option<i32>)> {
    let run: Function = table.raw_get("run")?;
    ctx.restart_budget();
    let returned: MultiValue = run.call(args)?;
    let mut values = returned.into_iter();
    let first = values.next().unwrap_or(Value::Nil);
    let code = match values.next() {
        None | Some(Value::Nil) => None,
        Some(Value::Integer(i)) if (0..=255).contains(&i) => Some(i as i32),
        Some(other) => {
            return Err(Fail::new(
                Exit::Failed,
                format!(
                    "run's second return value is the exit code, 0 to 255, not {}",
                    describe(&other)
                ),
            )
            .raise());
        }
    };
    let value = match &first {
        Value::Nil => None,
        Value::String(s) => {
            let mut text = ctx.text.borrow_mut();
            text.push_str(&s.to_string_lossy());
            if !text.ends_with('\n') {
                text.push('\n');
            }
            None
        }
        other => Some(from_lua(lua, other)?),
    };
    ctx.deadline.set(None);
    Ok((value, code))
}

fn describe(value: &Value) -> String {
    match value {
        Value::Integer(i) => i.to_string(),
        Value::Number(n) => n.to_string(),
        other => format!("a {}", other.type_name()),
    }
}

/// The `joue` module: what the run is, the exit codes, and helpers.
fn install_joue(lua: &Lua, globals: &Table, ctx: &Rc<Ctx>) -> mlua::Result<()> {
    let joue = lua.create_table()?;
    joue.set("json", ctx.options.json)?;
    joue.set("acting", ctx.options.yes)?;
    joue.set("dry_run", ctx.options.dry_run)?;
    let exit = lua.create_table()?;
    for e in Exit::ALL.into_iter().chain([Exit::Act, Exit::Waiting]) {
        exit.set(e.name(), e.code())?;
    }
    joue.set("exit", exit)?;
    joue.set(
        "fail",
        lua.create_function(|_, (code, message, hint): (i64, String, Option<String>)| {
            let exit = i32::try_from(code)
                .ok()
                .and_then(Exit::from_code)
                .unwrap_or(Exit::Failed);
            let mut fail = Fail::new(exit, message);
            fail.hint = hint;
            Err::<(), _>(fail.raise())
        })?,
    )?;
    joue.set(
        "array",
        lua.create_function(|lua, t: Table| {
            let shapes: Table = lua.named_registry_value(SHAPES)?;
            shapes.raw_set(&t, true)?;
            Ok(t)
        })?,
    )?;
    joue.set(
        "object",
        lua.create_function(|lua, (t, keys): (Table, Table)| {
            let shapes: Table = lua.named_registry_value(SHAPES)?;
            shapes.raw_set(&t, keys)?;
            Ok(t)
        })?,
    )?;
    joue.set(
        "duration",
        lua.create_function(|_, text: String| {
            Ok(match jouet_output::time::parse_duration(&text) {
                Ok(d) => (Some(d.as_secs_f64()), None),
                Err(e) => (None, Some(e)),
            })
        })?,
    )?;
    joue.set(
        "now",
        lua.create_function(|_, ()| Ok(jouet_output::time::now()))?,
    )?;
    let c = ctx.clone();
    joue.set(
        "call",
        lua.create_function(move |lua, (name, words): (String, Option<Vec<String>>)| {
            c.outside(|| call(lua, &c, &name, &words.unwrap_or_default()))
        })?,
    )?;
    globals.set("joue", joue)?;
    Ok(())
}

/// `joue.call(name, args)`: another routine, in this VM, with its own
/// capabilities, which the caller must hold too. Returns its result and
/// exit code; what it prints is dropped.
fn call(lua: &Lua, ctx: &Ctx, name: &str, words: &[String]) -> mlua::Result<(Value, i32)> {
    let caller = ctx.current.borrow().clone();
    let Some(routine) = ctx.catalog.get(name).cloned() else {
        return Err(Fail::new(Exit::NotFound, format!("joue.call: no routine `{name}`")).raise());
    };
    ready(&routine).map_err(Fail::raise)?;
    let missing: Vec<&str> = routine
        .capabilities
        .iter()
        .filter(|c| !ctx.caps.borrow().contains(c))
        .map(|c| c.name())
        .collect();
    if !missing.is_empty() {
        return Err(Fail::new(
            Exit::Precondition,
            format!(
                "{caller} calls {name}, which needs {} as well",
                missing.join(", ")
            ),
        )
        .hint(format!("add them to {caller}'s capabilities"))
        .raise());
    }
    if ctx.depth.get() >= MAX_DEPTH {
        return Err(Fail::new(
            Exit::Failed,
            format!("joue.call nests more than {MAX_DEPTH} deep"),
        )
        .raise());
    }
    let args = routine.parse_args(words).map_err(|e| {
        let message = match e {
            crate::ArgError::Help(_) => "--help is for a person".to_string(),
            crate::ArgError::Usage(m) => m,
        };
        Fail::new(Exit::Usage, format!("joue.call {name}: {message}")).raise()
    })?;
    let saved_caps = ctx.caps.replace(routine.capabilities.clone());
    let saved_name = ctx.current.replace(routine.name.clone());
    let saved_text = ctx.text.take();
    ctx.depth.set(ctx.depth.get() + 1);
    let result = load(lua, ctx, &routine).and_then(|table| {
        let run: Function = table.raw_get("run")?;
        let args = to_lua(lua, &serde_json::Value::Object(args.values))?;
        ctx.restart_budget();
        let returned: MultiValue = run.call(args)?;
        let mut values = returned.into_iter();
        let value = values.next().unwrap_or(Value::Nil);
        let code = match values.next() {
            Some(Value::Integer(i)) => i32::try_from(i).unwrap_or(1),
            _ => 0,
        };
        Ok((value, code))
    });
    ctx.depth.set(ctx.depth.get() - 1);
    ctx.text.replace(saved_text);
    ctx.current.replace(saved_name);
    ctx.caps.replace(saved_caps);
    result
}

/// The [`Fail`] an error carries, if a primitive or `joue.fail` raised it.
fn fail_of(e: &mlua::Error) -> Option<Fail> {
    match e {
        mlua::Error::ExternalError(err) => err.downcast_ref::<Fail>().cloned(),
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => {
            fail_of(cause)
        }
        _ => None,
    }
}

/// An error's first line, without mlua's `runtime error: ` prefix.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or(text);
    line.strip_prefix("runtime error: ")
        .unwrap_or(line)
        .to_string()
}

#[cfg(test)]
mod tests;
