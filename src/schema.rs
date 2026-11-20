//! A deliberately bounded evaluator for the bundled schema, not arbitrary user schemas.
use crate::{fail, Result};
use serde_json::Value;

pub const SCHEMA: &str = include_str!("../schema/catalog.schema.json");

pub fn valid_id(id: &str) -> bool {
    id.split_once('.')
        .is_some_and(|(a, b)| valid_slug(a) && valid_slug(b))
}

/// Lowercase slug: a letter, then letters, digits and hyphens.
pub fn valid_slug(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Calendar date in YYYY-MM-DD form with a plausible month and day.
pub fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<u32> {
        s.get(r)
            .filter(|p| p.bytes().all(|c| c.is_ascii_digit()))
            .and_then(|p| p.parse().ok())
    };
    matches!((digits(0..4), digits(5..7), digits(8..10)), (Some(_), Some(m), Some(d)) if (1..=12).contains(&m) && (1..=31).contains(&d))
}

/// Abbreviated or full hexadecimal Git object name.
pub fn valid_commit(s: &str) -> bool {
    (7..=64).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_hexdigit())
}

pub fn validate(value: &Value) -> Result<()> {
    validate_document(value, SCHEMA)
}

pub fn validate_document(value: &Value, document: &str) -> Result<()> {
    let root: Value = serde_json::from_str(document)?;
    let mut errors = Vec::new();
    check(value, &root, &root, "$", &mut errors)?;
    if errors.is_empty() {
        Ok(())
    } else {
        fail(errors.join("\n"))
    }
}

/// Every independent schema violation in a document.
pub fn validate_all(value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let result = serde_json::from_str(SCHEMA)
        .map_err(|e| e.to_string())
        .and_then(|root| check(value, &root, &root, "$", &mut errors).map_err(|e| e.to_string()));
    if let Err(error) = result {
        errors.push(error);
    }
    errors
}

fn check(
    value: &Value,
    rule: &Value,
    root: &Value,
    at: &str,
    errors: &mut Vec<String>,
) -> Result<()> {
    if rule == &Value::Bool(true) {
        return Ok(());
    }
    if rule == &Value::Bool(false) {
        errors.push(format!("{at}: unknown field"));
        return Ok(());
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
            "maxItems",
            "minLength",
            "pattern",
            "uniqueItems",
            "minimum",
            "maximum",
            "minProperties",
            "anyOf",
        ]
        .contains(&key.as_str())
        {
            return fail(format!("unsupported bundled schema keyword: {key}"));
        }
    }
    if let Some(alternatives) = rule.get("anyOf").and_then(Value::as_array) {
        let mut first = None;
        let mut matched = false;
        for alternative in alternatives {
            let mut attempt = Vec::new();
            check(value, alternative, root, at, &mut attempt)?;
            if attempt.is_empty() {
                matched = true;
                break;
            }
            first.get_or_insert(attempt);
        }
        if !matched {
            errors.push(format!("{at}: matches none of the allowed forms"));
            errors.extend(first.unwrap_or_default());
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
        check(value, target, root, at, errors)?;
    }
    if let Some(kind) = rule.get("type").and_then(Value::as_str) {
        let matches = match kind {
            "array" => value.is_array(),
            "object" => value.is_object(),
            "string" => value.is_string(),
            "integer" => value.is_u64() || value.is_i64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            _ => return fail(format!("unsupported schema type: {kind}")),
        };
        if !matches {
            errors.push(format!("{at}: expected {kind}"));
            return Ok(());
        }
    }
    if let Some(allowed) = rule.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            errors.push(format!(
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
            errors.push(format!("{at}: needs at least one platform"));
        }
        if let Some(required) = rule.get("required").and_then(Value::as_array) {
            for name in required {
                let name = name.as_str().ok_or("invalid required key")?;
                if !object.contains_key(name) {
                    errors.push(format!("{at}: missing required field {name}"));
                }
            }
        }
        for (name, child) in object {
            let default = Value::Bool(true);
            let child_rule = rule
                .get("properties")
                .and_then(|p| p.get(name))
                .unwrap_or_else(|| rule.get("additionalProperties").unwrap_or(&default));
            check(child, child_rule, root, &format!("{at}.{name}"), errors)?;
        }
    }
    if let Some(array) = value.as_array() {
        if array.len() < rule.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize {
            errors.push(format!("{at}: too few items"));
        }
        if rule
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| array.len() > max as usize)
        {
            errors.push(format!("{at}: too many items"));
        }
        for (i, child) in array.iter().enumerate() {
            if rule.get("uniqueItems") == Some(&Value::Bool(true)) && array[..i].contains(child) {
                errors.push(format!("{at}: duplicate item {child}"));
            }
            if let Some(items) = rule.get("items") {
                check(child, items, root, &format!("{at}[{i}]"), errors)?;
            }
        }
    }
    if let Some(text) = value.as_str() {
        if text.chars().count()
            < rule.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize
        {
            errors.push(format!("{at}: empty text"));
        }
        if let Some(pattern) = rule.get("pattern") {
            // Only the bundled patterns are understood; anything else fails closed.
            let (valid, what) = match pattern.as_str() {
                Some(r"^[a-z][a-z0-9-]*\.[a-z][a-z0-9-]*$") => (valid_id(text), "stable ID"),
                Some(r"^[a-z][a-z0-9-]*$") => (valid_slug(text), "slug"),
                Some(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$") => (valid_date(text), "date (YYYY-MM-DD)"),
                _ => return fail("unsupported schema pattern"),
            };
            if !valid {
                errors.push(format!("{at}: invalid {what} {text:?}"));
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
            errors.push(format!("{at}: number out of range"));
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
            "$",
            &mut Vec::new()
        )
        .is_err());
    }
}
