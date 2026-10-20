//! Minimal RFC 6902 JSON Patch application for `patch_sample` mock bodies.
//!
//! Operations are applied once when a rule revision is created, so the runtime
//! never carries a patch around. This implements the standard six operations
//! and returns the failing operation index in the error.

use serde_json::Value;

pub fn apply_json_patch(document: &[u8], operations: &[Value]) -> Result<Vec<u8>, String> {
    let text = std::str::from_utf8(document).map_err(|_| "sample_body_not_utf8".to_string())?;
    let mut value: Value =
        serde_json::from_str(text).map_err(|_| "sample_body_not_json".to_string())?;

    for (index, operation) in operations.iter().enumerate() {
        let op = operation
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("op {index}: missing op"))?;
        let path = operation
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("op {index}: missing path"))?;
        match op {
            "add" => {
                let added = operation
                    .get("value")
                    .cloned()
                    .ok_or_else(|| format!("op {index} add {path}: missing value"))?;
                add(&mut value, path, added)
                    .map_err(|reason| format!("op {index} add {path}: {reason}"))?;
            }
            "replace" => {
                let replacement = operation
                    .get("value")
                    .cloned()
                    .ok_or_else(|| format!("op {index} replace {path}: missing value"))?;
                replace(&mut value, path, replacement)
                    .map_err(|reason| format!("op {index} replace {path}: {reason}"))?;
            }
            "remove" => {
                remove(&mut value, path)
                    .map_err(|reason| format!("op {index} remove {path}: {reason}"))?;
            }
            "test" => {
                let expected = operation
                    .get("value")
                    .ok_or_else(|| format!("op {index} test {path}: missing value"))?;
                let actual = get(&value, path)
                    .ok_or_else(|| format!("op {index} test {path}: path does not exist"))?;
                if actual != expected {
                    return Err(format!("op {index} test {path}: value differs"));
                }
            }
            "copy" => {
                let from = operation
                    .get("from")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("op {index} copy {path}: missing from"))?;
                let copied = get(&value, from)
                    .cloned()
                    .ok_or_else(|| format!("op {index} copy {path}: from does not exist"))?;
                add(&mut value, path, copied)
                    .map_err(|reason| format!("op {index} copy {path}: {reason}"))?;
            }
            "move" => {
                let from = operation
                    .get("from")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("op {index} move {path}: missing from"))?;
                let moved = remove(&mut value, from)
                    .map_err(|reason| format!("op {index} move {path}: {reason}"))?;
                add(&mut value, path, moved)
                    .map_err(|reason| format!("op {index} move {path}: {reason}"))?;
            }
            other => return Err(format!("op {index}: unsupported op {other}")),
        }
    }

    serde_json::to_vec(&value).map_err(|error| error.to_string())
}

fn tokens(path: &str) -> Result<Vec<String>, String> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    if !path.starts_with('/') {
        return Err("path must start with /".to_string());
    }
    Ok(path[1..]
        .split('/')
        .map(|token| token.replace("~1", "/").replace("~0", "~"))
        .collect())
}

fn get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for token in tokens(path).ok()? {
        current = match current {
            Value::Object(map) => map.get(&token)?,
            Value::Array(items) => items.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn add(value: &mut Value, path: &str, added: Value) -> Result<(), String> {
    let tokens = tokens(path)?;
    if tokens.is_empty() {
        *value = added;
        return Ok(());
    }
    let Some((last, parents)) = tokens.split_last() else {
        return Err("empty path".to_string());
    };
    let parent = get_mut(value, parents).ok_or("parent path does not exist")?;
    match parent {
        Value::Object(map) => {
            map.insert(last.clone(), added);
            Ok(())
        }
        Value::Array(items) => {
            let index = if last == "-" {
                items.len()
            } else {
                last.parse::<usize>()
                    .map_err(|_| "array index must be a number")?
                    .min(items.len() + 1)
            };
            if index > items.len() {
                return Err("array index out of bounds".to_string());
            }
            items.insert(index, added);
            Ok(())
        }
        _ => Err("parent is not an object or array".to_string()),
    }
}

fn replace(value: &mut Value, path: &str, replacement: Value) -> Result<(), String> {
    let tokens = tokens(path)?;
    if tokens.is_empty() {
        *value = replacement;
        return Ok(());
    }
    let Some((last, parents)) = tokens.split_last() else {
        return Err("empty path".to_string());
    };
    let parent = get_mut(value, parents).ok_or("parent path does not exist")?;
    match parent {
        Value::Object(map) => {
            if !map.contains_key(last) {
                return Err("path does not exist".to_string());
            }
            map.insert(last.clone(), replacement);
            Ok(())
        }
        Value::Array(items) => {
            let index = last
                .parse::<usize>()
                .map_err(|_| "array index must be a number")?;
            let Some(slot) = items.get_mut(index) else {
                return Err("array index out of bounds".to_string());
            };
            *slot = replacement;
            Ok(())
        }
        _ => Err("parent is not an object or array".to_string()),
    }
}

fn remove(value: &mut Value, path: &str) -> Result<Value, String> {
    let tokens = tokens(path)?;
    if tokens.is_empty() {
        return Err("cannot remove the document root".to_string());
    }
    let Some((last, parents)) = tokens.split_last() else {
        return Err("empty path".to_string());
    };
    let parent = get_mut(value, parents).ok_or("parent path does not exist")?;
    match parent {
        Value::Object(map) => map
            .remove(last)
            .ok_or_else(|| "path does not exist".to_string()),
        Value::Array(items) => {
            let index = last
                .parse::<usize>()
                .map_err(|_| "array index must be a number")?;
            if index >= items.len() {
                return Err("array index out of bounds".to_string());
            }
            Ok(items.remove(index))
        }
        _ => Err("parent is not an object or array".to_string()),
    }
}

fn get_mut<'a>(value: &'a mut Value, tokens: &[String]) -> Option<&'a mut Value> {
    let mut current = value;
    for token in tokens {
        current = match current {
            Value::Object(map) => map.get_mut(token)?,
            Value::Array(items) => items.get_mut(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::*;
    use serde_json::json;

    #[test]
    fn applies_replace_add_remove_and_test() {
        let document = br#"{"price":10,"items":[{"id":1}],"name":"old"}"#;
        let patched = apply_json_patch(
            document,
            &[
                json!({"op": "replace", "path": "/price", "value": 0}),
                json!({"op": "add", "path": "/stock", "value": 3}),
                json!({"op": "remove", "path": "/name"}),
                json!({"op": "test", "path": "/items/0/id", "value": 1}),
                json!({"op": "copy", "from": "/stock", "path": "/items/-"}),
            ],
        )
        .expect("patch");
        let value: Value = serde_json::from_slice(&patched).expect("json");
        assert_eq!(value["price"], 0);
        assert_eq!(value["stock"], 3);
        assert!(value.get("name").is_none());
        assert_eq!(value["items"][1], 3);
    }

    #[test]
    fn reports_the_failing_operation() {
        let error = apply_json_patch(
            br#"{"price":10}"#,
            &[json!({"op": "replace", "path": "/missing", "value": 1})],
        )
        .expect_err("must fail");
        assert!(error.contains("op 0"));
        assert!(error.contains("/missing"));
    }
}
