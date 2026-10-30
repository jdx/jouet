//! Moving values between JSON and Luau: probe results and command output
//! into a script, a recipe's arguments and result out of it.

use mlua::{Lua, Table, Value};
use serde_json::{Map, Number};

/// How deep a Luau value may nest before it is refused, so a table that
/// contains itself ends in an error rather than a stack overflow.
const MAX_DEPTH: usize = 64;

/// A JSON value as Luau sees it: `null` is nil, arrays are sequences.
pub(crate) fn to_lua(lua: &Lua, value: &serde_json::Value) -> mlua::Result<Value> {
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
                table.raw_push(to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
        serde_json::Value::Object(fields) => {
            let table = lua.create_table_with_capacity(0, fields.len())?;
            for (key, item) in fields {
                table.raw_set(key.as_str(), to_lua(lua, item)?)?;
            }
            Value::Table(table)
        }
    })
}

/// A Luau value as JSON. A table with only the keys `1..n` is an array, an
/// empty table is an empty object, and any other table is an object whose
/// keys are strings (numbers are written out). Reads are raw, so no
/// metamethod of the script's runs.
pub(crate) fn from_lua(value: &Value) -> Result<serde_json::Value, String> {
    from_lua_at(value, 0)
}

fn from_lua_at(value: &Value, depth: usize) -> Result<serde_json::Value, String> {
    if depth > MAX_DEPTH {
        return Err("a table nests too deeply (or contains itself)".into());
    }
    Ok(match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => serde_json::Value::from(*i),
        Value::Number(n) => Number::from_f64(*n).map_or(serde_json::Value::Null, Into::into),
        Value::String(s) => serde_json::Value::String(s.to_string_lossy()),
        Value::Table(table) => table_to_json(table, depth)?,
        other => {
            return Err(format!(
                "a {} cannot be turned into JSON",
                other.type_name()
            ));
        }
    })
}

fn table_to_json(table: &Table, depth: usize) -> Result<serde_json::Value, String> {
    let len = table.raw_len();
    let mut count = 0;
    let mut fields = Map::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, item) = pair.map_err(|e| e.to_string())?;
        count += 1;
        let key = match key {
            Value::String(s) => s.to_string_lossy(),
            Value::Integer(i) => i.to_string(),
            Value::Number(n) => n.to_string(),
            other => return Err(format!("a table key cannot be a {}", other.type_name())),
        };
        fields.insert(key, from_lua_at(&item, depth + 1)?);
    }
    if len > 0 && count == len {
        let items = (1..=len)
            .map(|i| {
                let item: Value = table.raw_get(i).map_err(|e| e.to_string())?;
                from_lua_at(&item, depth + 1)
            })
            .collect::<Result<_, _>>()?;
        return Ok(serde_json::Value::Array(items));
    }
    Ok(serde_json::Value::Object(fields))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn values_survive_the_round_trip() {
        let lua = Lua::new();
        let value = json!({
            "n": 3, "f": 1.5, "s": "x", "b": true,
            "list": [1, "two", {"three": 3}], "empty": {}
        });
        let back = from_lua(&to_lua(&lua, &value).unwrap()).unwrap();
        assert_eq!(back, value);
    }

    #[test]
    fn null_is_nil_and_a_table_that_contains_itself_is_refused() {
        let lua = Lua::new();
        assert_eq!(to_lua(&lua, &json!(null)).unwrap(), Value::Nil);
        let t: Table = lua.load("local t = {}; t.t = t; return t").eval().unwrap();
        assert!(from_lua(&Value::Table(t)).unwrap_err().contains("deeply"));
        let f: Value = lua.load("return function() end").eval().unwrap();
        assert!(from_lua(&f).is_err());
    }
}
