// Portions derived from pi-durable 1.5.2 tool-search, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Deferred-tool name match.
//!
//! Upstream `tool_search` ranks with BM25 and, since 1.5.1, follows a local
//! `$ref` into root `$defs`. This port resolves those refs and returns names
//! whose name or description contains the query. Ranking is not copied.

use serde_json::Value;

const MAX_REF_REPLACEMENTS: usize = 8;

/// Copy `schema`, replacing each `#/$defs/Name` with that root def.
///
/// A name that is not in root `$defs` stays the original `$ref` object.
/// Only the `#/$defs/Name` form is local; a longer pointer is left alone.
/// Each hop along a ref chain counts toward eight replacements, then the
/// next `$ref` is kept, so a recursive schema cannot loop.
pub fn resolve_refs(schema: &Value) -> Value {
    let defs = schema
        .as_object()
        .and_then(|object| object.get("$defs"))
        .and_then(Value::as_object);
    resolve_value(schema, defs, 0)
}

fn resolve_value(
    value: &Value,
    defs: Option<&serde_json::Map<String, Value>>,
    depth: usize,
) -> Value {
    if let Some(name) = local_def_name(value) {
        if depth < MAX_REF_REPLACEMENTS {
            if let Some(def) = defs.and_then(|defs| defs.get(name)) {
                return resolve_value(def, defs, depth + 1);
            }
        }
        return value.clone();
    }
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve_value(item, defs, depth))
                .collect(),
        ),
        Value::Object(object) => {
            let mut resolved = serde_json::Map::new();
            for (key, child) in object {
                resolved.insert(key.clone(), resolve_value(child, defs, depth));
            }
            Value::Object(resolved)
        }
        other => other.clone(),
    }
}

/// `#/$defs/Name` with a single pointer segment, if this value is that object.
fn local_def_name(value: &Value) -> Option<&str> {
    let reference = value.as_object()?.get("$ref")?.as_str()?;
    let name = reference.strip_prefix("#/$defs/")?;
    if name.is_empty() || name.contains('/') {
        None
    } else {
        Some(name)
    }
}

/// Names whose name or description contains `query`, in input order.
///
/// The match is case-sensitive. An empty query matches nothing.
pub fn matching_names(tools: &[(String, String)], query: &str) -> Vec<String> {
    if query.is_empty() {
        return Vec::new();
    }
    tools
        .iter()
        .filter(|(name, description)| name.contains(query) || description.contains(query))
        .map(|(name, _)| name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replaces_a_local_def_and_keeps_missing_refs() {
        let schema = json!({
            "$defs": {
                "Item": { "type": "string", "description": "an item" },
                "Wrap": { "$ref": "#/$defs/Item" }
            },
            "type": "object",
            "properties": {
                "name": { "$ref": "#/$defs/Item", "description": "dropped" },
                "wrap": { "$ref": "#/$defs/Wrap" },
                "missing": { "$ref": "#/$defs/Nope", "description": "keep" },
                "remote": { "$ref": "https://example.com/schema" },
                "nested_pointer": { "$ref": "#/$defs/Item/type" }
            },
            "anyOf": [
                { "$ref": "#/$defs/Item" },
                { "type": "null" }
            ]
        });
        let resolved = resolve_refs(&schema);
        let item = json!({"type": "string", "description": "an item"});
        assert_eq!(resolved["properties"]["name"], item);
        assert_eq!(resolved["properties"]["wrap"], item);
        assert_eq!(
            resolved["properties"]["missing"],
            json!({"$ref": "#/$defs/Nope", "description": "keep"})
        );
        assert_eq!(
            resolved["properties"]["remote"],
            json!({"$ref": "https://example.com/schema"})
        );
        assert_eq!(
            resolved["properties"]["nested_pointer"],
            json!({"$ref": "#/$defs/Item/type"})
        );
        assert_eq!(resolved["anyOf"][0], item);
        assert_eq!(schema["properties"]["name"]["$ref"], "#/$defs/Item");
        assert_eq!(resolve_refs(&json!(1)), json!(1));
        assert_eq!(resolve_refs(&json!(null)), json!(null));
    }

    #[test]
    fn recursive_ref_stops_after_eight_replacements() {
        let schema = json!({
            "$defs": {
                "Loop": {
                    "properties": { "next": { "$ref": "#/$defs/Loop" } }
                }
            },
            "properties": {
                "loop": { "$ref": "#/$defs/Loop" }
            }
        });
        let resolved = resolve_refs(&schema);
        let mut cursor = &resolved["properties"]["loop"];
        for _ in 0..super::MAX_REF_REPLACEMENTS {
            assert!(cursor.get("properties").is_some());
            cursor = &cursor["properties"]["next"];
        }
        assert_eq!(cursor, &json!({"$ref": "#/$defs/Loop"}));
    }

    #[test]
    fn matching_names_is_case_sensitive_and_keeps_input_order() {
        let tools = vec![
            ("zzz".to_string(), "nothing".to_string()),
            ("read".to_string(), "Read a file".to_string()),
            ("search".to_string(), "Find text".to_string()),
            ("ReadMe".to_string(), "notes".to_string()),
        ];
        assert!(matching_names(&tools, "").is_empty());
        assert!(matching_names(&tools, "Nope").is_empty());
        assert_eq!(
            matching_names(&tools, "Read"),
            vec!["read".to_string(), "ReadMe".to_string()]
        );
        assert_eq!(
            matching_names(&tools, "search"),
            vec!["search".to_string()]
        );
        assert_eq!(matching_names(&tools, "text"), vec!["search".to_string()]);
        assert!(matching_names(&[], "read").is_empty());
    }
}
