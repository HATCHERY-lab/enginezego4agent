// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/tool.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/tool, MIT, see LICENSES/pi-MIT.txt.

//! Shape of a tool call before it runs.
//!
//! Upstream `prepare` may rewrite arguments, then validation checks the tool
//! schema. This step only requires a call id, a name, and a JSON object.
//! `null` is the empty object. Property values are stored as given.

use serde_json::{Map, Value};

/// A tool call whose arguments are a JSON object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

/// Reject an empty `call_id` or `name`.
///
/// `arguments` must be a JSON object. JSON `null` becomes `{}`. An array,
/// string, number, or boolean is an error.
pub fn prepare_call(
    call_id: &str,
    name: &str,
    arguments: Value,
) -> Result<PreparedCall, String> {
    if call_id.is_empty() {
        return Err("tool call id is empty".to_owned());
    }
    if name.is_empty() {
        return Err("tool name is empty".to_owned());
    }
    let arguments = match arguments {
        Value::Null => Value::Object(Map::new()),
        Value::Object(map) => Value::Object(map),
        other => {
            return Err(format!(
                "tool arguments must be a JSON object, got {}",
                json_kind(&other)
            ));
        }
    };
    Ok(PreparedCall {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        arguments,
    })
}

/// True when `call_id` and `name` match. Arguments are not compared.
pub fn same_call(a: &PreparedCall, b: &PreparedCall) -> bool {
    a.call_id == b.call_id && a.name == b.name
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn null_arguments_become_an_empty_object() {
        let call = prepare_call("c1", "read", Value::Null).unwrap();
        assert_eq!(call.call_id, "c1");
        assert_eq!(call.name, "read");
        assert_eq!(call.arguments, json!({}));
    }

    #[test]
    fn object_arguments_are_kept() {
        let args = json!({"path": "a.rs", "limit": null});
        let call = prepare_call("c1", "read", args.clone()).unwrap();
        assert_eq!(call.arguments, args);
    }

    #[test]
    fn empty_id_or_name_is_rejected() {
        assert_eq!(
            prepare_call("", "read", json!({})).unwrap_err(),
            "tool call id is empty"
        );
        assert_eq!(
            prepare_call("c1", "", json!({})).unwrap_err(),
            "tool name is empty"
        );
        assert_eq!(
            prepare_call("", "", Value::Null).unwrap_err(),
            "tool call id is empty"
        );
    }

    #[test]
    fn non_object_arguments_are_rejected() {
        assert_eq!(
            prepare_call("c1", "read", json!("x")).unwrap_err(),
            "tool arguments must be a JSON object, got string"
        );
        assert_eq!(
            prepare_call("c1", "read", json!(1)).unwrap_err(),
            "tool arguments must be a JSON object, got number"
        );
        assert_eq!(
            prepare_call("c1", "read", json!(true)).unwrap_err(),
            "tool arguments must be a JSON object, got boolean"
        );
        assert_eq!(
            prepare_call("c1", "read", json!([1])).unwrap_err(),
            "tool arguments must be a JSON object, got array"
        );
    }

    #[test]
    fn same_call_matches_id_and_name_only() {
        let left = prepare_call("c1", "read", json!({"path": "a"})).unwrap();
        let right = prepare_call("c1", "read", json!({"path": "b"})).unwrap();
        let other_id = prepare_call("c2", "read", json!({"path": "a"})).unwrap();
        let other_name = prepare_call("c1", "write", json!({"path": "a"})).unwrap();
        assert!(same_call(&left, &right));
        assert!(!same_call(&left, &other_id));
        assert!(!same_call(&left, &other_name));
    }
}
