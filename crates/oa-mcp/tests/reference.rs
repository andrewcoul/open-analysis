//! Keeps `COMMAND_REFERENCE` level with the `Command` enum: every command is
//! documented, and every example parses. Agents learn the command set from
//! this text alone, so a stale entry fails here rather than in an agent.
use oa_mcp::COMMAND_REFERENCE;
use oa_model::Command;

/// Every command tag, read from serde's own list of expected variants so a
/// new variant is picked up without editing this test.
fn tags() -> Vec<String> {
    let err = serde_json::from_str::<Command>(r#"{"command":"no_such_command"}"#)
        .expect_err("an unknown tag is refused")
        .to_string();
    let (_, expected) = err
        .split_once("expected one of ")
        .unwrap_or_else(|| panic!("serde's message changed shape: {err}"));
    let tags: Vec<String> = expected
        .split(", ")
        .map(|t| t.split('`').nth(1).unwrap_or_default().to_string())
        .filter(|t| !t.is_empty())
        .collect();
    assert!(
        ["add_node", "set_level_elevation", "batch"]
            .iter()
            .all(|t| tags.iter().any(|x| x == t)),
        "tag list looks wrong: {tags:?}"
    );
    tags
}

fn mentions(tag: &str) -> bool {
    COMMAND_REFERENCE.match_indices(tag).any(|(at, _)| {
        let before = COMMAND_REFERENCE[..at].chars().next_back();
        let after = COMMAND_REFERENCE[at + tag.len()..].chars().next();
        let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
        !word(before) && !word(after)
    })
}

#[test]
fn every_command_is_documented() {
    let missing: Vec<String> = tags()
        .into_iter()
        .filter(|tag| {
            // The reference says update_* and remove_* exist for every kind,
            // so they are covered wherever the kind's add_ is.
            let add = tag
                .strip_prefix("update_")
                .or_else(|| tag.strip_prefix("remove_"))
                .map(|kind| format!("add_{kind}"));
            !mentions(tag) && !add.is_some_and(|add| mentions(&add))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "COMMAND_REFERENCE has no entry for {missing:?}"
    );
}

/// Each `{"command": ...}` object in the reference, found by matching braces.
fn examples() -> Vec<&'static str> {
    let text = COMMAND_REFERENCE;
    let mut out = vec![];
    for (start, _) in text.match_indices(r#"{"command""#) {
        let mut depth = 0;
        for (i, c) in text[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                out.push(&text[start..=start + i]);
                break;
            }
        }
    }
    out
}

#[test]
fn every_example_parses_as_a_command() {
    let examples = examples();
    let mut parsed = 0;
    for example in &examples {
        // Examples that elide fields with "..." are prose, not JSON.
        if example.contains("...") {
            continue;
        }
        if let Err(e) = serde_json::from_str::<Command>(example) {
            panic!("example does not parse: {e}\n{example}");
        }
        parsed += 1;
    }
    assert!(
        parsed >= 15,
        "only {parsed} of {} examples parsed",
        examples.len()
    );
}
