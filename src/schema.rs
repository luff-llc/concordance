//! A deliberately bounded evaluator for the bundled schema, not arbitrary user schemas.
use crate::{fail, Result};
use serde_json::Value;

pub const SCHEMA: &str = include_str!("../schema/catalog.schema.json");

pub fn valid_id(id: &str) -> bool {
    fn part(s: &str) -> bool {
        s.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            && s.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    }
    id.split_once('.').is_some_and(|(a, b)| part(a) && part(b))
}

pub fn validate(value: &Value) -> Result<()> {
    let root: Value = serde_json::from_str(SCHEMA)?;
    check(value, &root, &root, "$")
}

fn check(value: &Value, rule: &Value, root: &Value, at: &str) -> Result<()> {
    if rule == &Value::Bool(true) {
        return Ok(());
    }
    if rule == &Value::Bool(false) {
        return fail(format!("{at}: unknown field"));
    }
    let rules = rule.as_object().ok_or("invalid bundled schema")?;
    // New assertion keywords must be implemented, never silently ignored.
    for key in rules.keys() {
        if ![
            "$schema",
            "$id",
            "$defs",
            "$ref",
            "title",
            "description",
            "type",
            "required",
            "properties",
            "additionalProperties",
            "items",
            "enum",
            "minItems",
            "minLength",
            "pattern",
            "uniqueItems",
            "minimum",
            "maximum",
            "minProperties",
        ]
        .contains(&key.as_str())
        {
            return fail(format!("unsupported bundled schema keyword: {key}"));
        }
    }
    if let Some(reference) = rule.get("$ref").and_then(Value::as_str) {
        let target = root
            .pointer(
                reference
                    .strip_prefix('#')
                    .ok_or("nonlocal schema reference")?,
            )
            .ok_or("missing schema reference")?;
        check(value, target, root, at)?;
    }
    if let Some(kind) = rule.get("type").and_then(Value::as_str) {
        let matches = match kind {
            "array" => value.is_array(),
            "object" => value.is_object(),
            "string" => value.is_string(),
            "integer" => value.is_u64() || value.is_i64(),
            _ => return fail(format!("unsupported schema type: {kind}")),
        };
        if !matches {
            return fail(format!("{at}: expected {kind}"));
        }
    }
    if let Some(allowed) = rule.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return fail(format!(
                "{at}: invalid value {value}; expected one of {allowed:?}"
            ));
        }
    }
    if let Some(object) = value.as_object() {
        if object.len()
            < rule
                .get("minProperties")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize
        {
            return fail(format!("{at}: needs at least one platform"));
        }
        if let Some(required) = rule.get("required").and_then(Value::as_array) {
            for name in required {
                let name = name.as_str().ok_or("invalid required key")?;
                if !object.contains_key(name) {
                    return fail(format!("{at}: missing required field {name}"));
                }
            }
        }
        for (name, child) in object {
            let default = Value::Bool(true);
            let child_rule = rule
                .get("properties")
                .and_then(|p| p.get(name))
                .unwrap_or_else(|| rule.get("additionalProperties").unwrap_or(&default));
            check(child, child_rule, root, &format!("{at}.{name}"))?;
        }
    }
    if let Some(array) = value.as_array() {
        if array.len() < rule.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize {
            return fail(format!("{at}: too few items"));
        }
        for (i, child) in array.iter().enumerate() {
            if rule.get("uniqueItems") == Some(&Value::Bool(true)) && array[..i].contains(child) {
                return fail(format!("{at}: duplicate item {child}"));
            }
            if let Some(items) = rule.get("items") {
                check(child, items, root, &format!("{at}[{i}]"))?;
            }
        }
    }
    if let Some(text) = value.as_str() {
        if text.chars().count()
            < rule.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize
        {
            return fail(format!("{at}: empty text"));
        }
        if let Some(pattern) = rule.get("pattern") {
            if pattern.as_str() != Some(r"^[a-z][a-z0-9-]*\.[a-z][a-z0-9-]*$") {
                return fail("unsupported schema pattern");
            }
            if !valid_id(text) {
                return fail(format!("{at}: invalid stable ID {text:?}"));
            }
        }
    }
    if let Some(number) = value.as_f64() {
        if rule
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || rule
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
        {
            return fail(format!("{at}: number out of range"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_ids() {
        for id in ["reader.open", "a-b.c2-d"] {
            assert!(valid_id(id));
        }
        for id in ["A.b", "a.b.c", "a_b.c", "a.", "a/b.c", "a.2b", "a\\b.c"] {
            assert!(!valid_id(id));
        }
    }
    #[test]
    fn evaluator_fails_closed() {
        assert!(check(
            &Value::Null,
            &serde_json::json!({"futureKeyword":true}),
            &Value::Null,
            "$"
        )
        .is_err());
    }
}
