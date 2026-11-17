// engine zero for agents tool_search call.

//! Deferred-tool search result. No model call.
//!
//! [`crate::matching_names`] picks names. Each name is paired with its schema
//! after [`crate::resolve_refs`], or JSON `null` when `schemas` has no entry.

use serde_json::{json, Map, Value};

use crate::tool_search::{matching_names, resolve_refs};

/// `{ "matches": [ { "name", "schema" } ] }` for tools whose name or description contains `query`.
///
/// An empty query returns `{ "matches": [] }`. A missing schema is JSON `null`.
/// A present schema is copied through [`crate::resolve_refs`]. Nothing calls a model.
pub fn answer(tools: &[(String, String)], query: &str, schemas: &Map<String, Value>) -> Value {
    let matches: Vec<Value> = matching_names(tools, query)
        .into_iter()
        .map(|name| {
            let schema = match schemas.get(&name) {
                Some(schema) => resolve_refs(schema),
                None => Value::Null,
            };
            json!({ "name": name, "schema": schema })
        })
        .collect();
    json!({ "matches": matches })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_query_returns_no_matches() {
        let tools = vec![("read".to_string(), "Read a file".to_string())];
        let mut schemas = Map::new();
        schemas.insert("read".to_string(), json!({ "type": "object" }));
        assert_eq!(answer(&tools, "", &schemas), json!({ "matches": [] }));
        assert_eq!(answer(&[], "read", &Map::new()), json!({ "matches": [] }));
    }

    #[test]
    fn pairs_each_match_with_a_resolved_schema_or_null() {
        let tools = vec![
            ("zzz".to_string(), "nothing".to_string()),
            ("read".to_string(), "Read a file".to_string()),
            ("search".to_string(), "Find text".to_string()),
            ("notes".to_string(), "ReadMe later".to_string()),
        ];
        let schema = json!({
            "$defs": { "Path": { "type": "string" } },
            "properties": { "path": { "$ref": "#/$defs/Path" } }
        });
        let mut schemas = Map::new();
        schemas.insert("read".to_string(), schema.clone());
        schemas.insert("unused".to_string(), json!({ "type": "number" }));
        let result = answer(&tools, "Read", &schemas);
        assert_eq!(
            result,
            json!({
                "matches": [
                    {
                        "name": "read",
                        "schema": {
                            "$defs": { "Path": { "type": "string" } },
                            "properties": { "path": { "type": "string" } }
                        }
                    },
                    { "name": "notes", "schema": null }
                ]
            })
        );
        assert_eq!(schemas.get("read"), Some(&schema));
    }
}
