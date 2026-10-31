use super::*;

fn parse_err(code: &str) -> (usize, String) {
    match parse(code) {
        Ok(rules) => panic!("parsed {} rules from {code}", rules.len()),
        Err(e) => e,
    }
}

#[test]
fn a_file_holds_any_number_of_rules_each_with_its_line() {
    let rules = parse(
        "#:schema x\n[[rule]]\nname = \"a\"\nwhy = \"w\"\nmessage = \"m\"\n[rule.match]\ncommand = \"ls\"\n\n[[rule]]\nname = \"b\"\nwhy = \"w\"\nscript = \"b.luau\"\n",
    )
    .unwrap();
    let lines: Vec<(usize, &str)> = rules.iter().map(|(l, r)| (*l, r.name.0.as_str())).collect();
    assert_eq!(lines, [(2, "a"), (9, "b")]);
    assert_eq!(rules[0].1.matchers.len(), 1);
    assert_eq!(rules[1].1.script.as_ref().unwrap().0, "b.luau");
    assert!(parse("").unwrap().is_empty());
}

#[test]
fn match_is_a_table_or_a_list_of_them() {
    let one = parse("[[rule]]\nname = \"a\"\nwhy = \"w\"\nmatch = { command = \"ls\" }\n").unwrap();
    assert_eq!(one[0].1.matchers.len(), 1);
    let many = parse(
        "[[rule]]\nname = \"a\"\nwhy = \"w\"\n[[rule.match]]\ncommand = \"ls\"\n[[rule.match]]\ncommand = [\"rm\", \"mv\"]\nflags = \"-f\"\n",
    )
    .unwrap();
    assert_eq!(many[0].1.matchers.len(), 2);
    assert_eq!(many[0].1.matchers[1].command.len(), 2);
}

#[test]
fn errors_name_the_line_and_what_is_wrong() {
    let (line, e) =
        parse_err("[[rule]]\nname = \"a\"\nwhy = \"w\"\n\n[rule.match]\ncomand = \"ls\"\n");
    assert_eq!(line, 6);
    assert!(e.contains("unknown field `comand`"), "{e}");
    let (line, e) = parse_err("[[rule]]\nname = \"A b\"\n");
    assert_eq!(line, 2);
    assert!(e.contains("not a rule name"), "{e}");
    let (line, e) = parse_err("[[rule]]\nname = \"a\"\nwhy = \"w\"\nmode = \"block\"\n");
    assert_eq!(line, 4);
    assert!(e.contains("`warn`") && e.contains("`deny`"), "{e}");
    let (_, e) = parse_err(
        "[[rule]]\nname = \"a\"\nwhy = \"w\"\nmatch = { command = \"ls\", flags = \"-fu\" }\n",
    );
    assert!(e.contains("cluster") && e.contains("`-f`, `-u`"), "{e}");
    let (_, e) = parse_err(
        "[[rule]]\nname = \"a\"\nwhy = \"w\"\nmatch = { command = \"ls\", flags = \"force\" }\n",
    );
    assert!(e.contains("not an option"), "{e}");
    let (_, e) = parse_err(
        "[[rule]]\nname = \"a\"\nwhy = \"w\"\nmatch = { command = \"ls\", text = \"(\" }\n",
    );
    assert!(e.contains("not a regular expression"), "{e}");
    let (_, e) = parse_err("[[rule]]\nname = \"a\"\nwhy = \"w\"\nscript = \"../x.luau\"\n");
    assert!(e.contains("in this rule's directory"), "{e}");
    let (_, e) = parse_err("[[rule]]\nname = \"a\"\nwhy = \"w\"\nscript = \"x.lua\"\n");
    assert!(e.contains(".luau"), "{e}");
    let (_, e) =
        parse_err("[[rule]]\nname = \"a\"\nwhy = \"w\"\n[rule.examples]\ndeny = [\"x\"]\n");
    assert!(e.contains("unknown field `deny`"), "{e}");
    let (_, e) = parse_err(
        "[[rule]]\nname = \"a\"\nwhy = \"w\"\n[rule.examples]\nfires = [{ comand = \"x\" }]\n",
    );
    assert!(e.contains("unknown field `comand`"), "{e}");
    let (_, e) = parse_err("[[rule]]\nname = \"a\"\nwhy = \"w\"\nmatch = { command = \"\" }\n");
    assert!(e.contains("names a program"), "{e}");
}

/// The schema, as a JSON value.
fn schema_value() -> serde_json::Value {
    serde_json::from_str(&schema()).unwrap()
}

#[test]
fn the_schema_describes_every_key_the_parser_reads() {
    let schema = schema_value();
    assert_eq!(schema["$id"], SCHEMA_URL);
    let defs = &schema["$defs"];
    let keys = |def: &str| -> Vec<String> {
        let mut keys: Vec<String> = defs[def]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{def}: {schema}"))
            .keys()
            .cloned()
            .collect();
        keys.sort();
        keys
    };
    assert_eq!(
        keys("Rule"),
        [
            "author", "created", "examples", "match", "message", "mode", "name", "needs", "on",
            "requires", "scope", "script", "suggest", "why"
        ]
    );
    assert_eq!(
        keys("Matcher"),
        [
            "args",
            "background",
            "command",
            "exact_flags",
            "flags",
            "fnox",
            "input",
            "max_args",
            "min_code_lines",
            "min_read_lines",
            "paths",
            "prints",
            "secret_args",
            "secret_files",
            "secret_vars",
            "seen",
            "text",
            "tool",
            "unless_args",
            "unless_flags",
            "unless_input",
            "unless_paths",
            "unless_with",
            "value_flags",
            "via",
            "with"
        ]
    );
    assert_eq!(
        keys("ExampleCall"),
        [
            "action",
            "claims",
            "code_lines",
            "command",
            "exit_code",
            "fnox",
            "git",
            "harness",
            "input",
            "kind",
            "path",
            "read_lines",
            "scope",
            "seen",
            "session",
            "stack",
            "tool"
        ]
    );
    let mut required: Vec<&str> = defs["Rule"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["name", "why"]);
    assert!(!schema.to_string().contains("null"), "TOML has no null");
    // Every key has a description, which editors show.
    for def in ["Rule", "Requires", "Matcher", "ExampleCall"] {
        for (key, value) in defs[def]["properties"].as_object().unwrap() {
            assert!(
                value.get("description").is_some(),
                "{def}.{key} has no description"
            );
        }
    }
}
