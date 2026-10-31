//! Moving probe results from JSON into a rule's Luau values.

use mlua::{Lua, Value};

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

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Table;
    use serde_json::json;

    #[test]
    fn null_is_nil_and_arrays_are_sequences() {
        let lua = Lua::new();
        assert_eq!(to_lua(&lua, &json!(null)).unwrap(), Value::Nil);
        let Value::Table(t) = to_lua(&lua, &json!({ "list": [1, "two"], "n": 1.5 })).unwrap()
        else {
            panic!("a table");
        };
        let list: Table = t.raw_get("list").unwrap();
        assert_eq!(list.raw_len(), 2);
        assert_eq!(list.raw_get::<String>(2).unwrap(), "two");
        assert_eq!(t.raw_get::<f64>("n").unwrap(), 1.5);
    }
}
