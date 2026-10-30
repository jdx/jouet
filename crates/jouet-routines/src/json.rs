//! Moving values between JSON and Luau: primitives' answers into a routine,
//! and a routine's arguments and result out of it.
//!
//! A Luau table cannot say whether it is an empty list or an empty object,
//! or in what order its keys go. The shapes table answers both: a routine
//! marks a table with `joue.array(t)` or `joue.object(t, keys)`, and every
//! list a primitive returns is marked an array, so it stays one on the way
//! back out. Other objects have their keys sorted, so output is stable.

use mlua::{Lua, Table, Value};
use serde_json::{Map, Number};

/// How deep a Luau value may nest before it is refused, so a table that
/// contains itself ends in an error rather than a stack overflow.
const MAX_DEPTH: usize = 64;

/// A weak-keyed table of marked tables: `true` for an array, a list of keys
/// for an object whose keys go in that order.
pub(crate) fn shapes(lua: &Lua) -> mlua::Result<Table> {
    let shapes = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.raw_set("__mode", "k")?;
    shapes.set_metatable(Some(meta))?;
    Ok(shapes)
}

/// A JSON value as Luau sees it: `null` is nil, arrays are sequences
/// marked as arrays in `shapes`.
pub(crate) fn to_lua(lua: &Lua, shapes: &Table, value: &serde_json::Value) -> mlua::Result<Value> {
    Ok(match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(s) => Value::String(lua.create_string(s)?),
        serde_json::Value::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for item in items {
                table.raw_push(to_lua(lua, shapes, item)?)?;
            }
            shapes.raw_set(&table, true)?;
            Value::Table(table)
        }
        serde_json::Value::Object(fields) => {
            let table = lua.create_table_with_capacity(0, fields.len())?;
            for (key, item) in fields {
                table.raw_set(key.as_str(), to_lua(lua, shapes, item)?)?;
            }
            Value::Table(table)
        }
    })
}

/// A Luau value as JSON. A table marked as an array, or with only the keys
/// `1..n`, is an array; any other table is an object, empty when it has no
/// keys. Reads are raw, so no metamethod of the routine's runs.
pub(crate) fn from_lua(shapes: &Table, value: &Value) -> Result<serde_json::Value, String> {
    from_lua_at(shapes, value, 0)
}

fn from_lua_at(shapes: &Table, value: &Value, depth: usize) -> Result<serde_json::Value, String> {
    if depth > MAX_DEPTH {
        return Err("a table nests too deeply (or contains itself)".into());
    }
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => serde_json::Value::from(*i),
        // Luau's numbers are all floats: a whole one is written as an
        // integer, as the JSON it came from wrote it.
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9e15 => {
            serde_json::Value::from(*n as i64)
        }
        Value::Number(n) => Number::from_f64(*n).map_or(serde_json::Value::Null, Into::into),
        Value::String(s) => serde_json::Value::String(s.to_string_lossy()),
        Value::Table(table) => table_to_json(shapes, table, depth)?,
        other => {
            return Err(format!(
                "a {} cannot be turned into JSON",
                other.type_name()
            ));
        }
    })
}

fn table_to_json(shapes: &Table, table: &Table, depth: usize) -> Result<serde_json::Value, String> {
    let shape: Value = shapes.raw_get(table).map_err(|e| e.to_string())?;
    let len = table.raw_len();
    let item = |i: usize| -> Result<serde_json::Value, String> {
        let value: Value = table.raw_get(i).map_err(|e| e.to_string())?;
        from_lua_at(shapes, &value, depth + 1)
    };
    if shape == Value::Boolean(true) {
        return Ok(serde_json::Value::Array(
            (1..=len).map(item).collect::<Result<_, _>>()?,
        ));
    }
    let mut fields: Vec<(String, serde_json::Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair.map_err(|e| e.to_string())?;
        let key = match key {
            Value::String(s) => s.to_string_lossy(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            other => return Err(format!("a table key cannot be a {}", other.type_name())),
        };
        fields.push((key, from_lua_at(shapes, &value, depth + 1)?));
    }
    if len > 0 && fields.len() == len && !matches!(shape, Value::Table(_)) {
        return Ok(serde_json::Value::Array(
            (1..=len).map(item).collect::<Result<_, _>>()?,
        ));
    }
    let order: Vec<String> = match shape {
        Value::Table(keys) => keys
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()
            .map_err(|e| format!("joue.object's keys: {e}"))?,
        _ => Vec::new(),
    };
    let rank = |key: &str| order.iter().position(|k| k == key).unwrap_or(usize::MAX);
    fields.sort_by(|(a, _), (b, _)| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    Ok(serde_json::Value::Object(
        fields.into_iter().collect::<Map<_, _>>(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_survive_the_round_trip() {
        let lua = Lua::new();
        let shapes = shapes(&lua).unwrap();
        let value = json!({
            "n": 3, "f": 1.5, "s": "x", "b": true,
            "list": [1, "two", {"three": 3}], "empty": {}, "none": []
        });
        let back = from_lua(&shapes, &to_lua(&lua, &shapes, &value).unwrap()).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn marked_tables_keep_their_shape_and_order() {
        let lua = Lua::new();
        let shapes = shapes(&lua).unwrap();
        lua.globals().set("shapes", &shapes).unwrap();
        let t: Value = lua
            .load(
                r#"local entry = { title = "t", repo = "jdx/a", number = 7 }
                shapes[entry] = { "repo", "number" }
                local empty = {}
                shapes[empty] = true
                return { prs = empty, entry = entry }"#,
            )
            .eval()
            .unwrap();
        let json = from_lua(&shapes, &t).unwrap();
        assert_eq!(
            serde_json::to_string(&json).unwrap(),
            r#"{"entry":{"repo":"jdx/a","number":7,"title":"t"},"prs":[]}"#
        );
    }

    #[test]
    fn a_table_that_contains_itself_is_refused() {
        let lua = Lua::new();
        let shapes = shapes(&lua).unwrap();
        let t: Value = lua.load("local t = {}; t.t = t; return t").eval().unwrap();
        assert!(from_lua(&shapes, &t).unwrap_err().contains("deeply"));
        let f: Value = lua.load("return function() end").eval().unwrap();
        assert!(from_lua(&shapes, &f).is_err());
    }
}
