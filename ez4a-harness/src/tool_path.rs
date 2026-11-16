// engine zero for agents tool path. Uses pi-durable derived helpers, MIT, see LICENSES/pi-MIT.txt.

//! One tool round that does not run a process or write the disk.
//!
//! `prepare_call` and `tool_result_entry` are the pi-durable derived helpers.
//! Known tools are queued. An unknown name is a harness error.

use serde_json::Value;

use crate::tool_prep::prepare_call;
use crate::tool_run::{harness_error, tool_result_entry, ToolExecution};

/// Not committed. A later slice supplies the live conversation when it stores this.
const UNCOMMITTED_CONVERSATION: i64 = 0;

/// Prepare one tool call and return its journal object.
///
/// `prepare_call` turns JSON `null` into `{}`. When `schema` is present, those
/// arguments are replaced with `ez4a_types::coerce`. `read`, `write`, `edit`,
/// `bash`, and `powershell` are not executed: `isError` is false and `content`
/// is `queued:{name}`. Any other name is `harness_error("unknown_tool", name)`
/// passed through `tool_result_entry` with this `call_id`.
///
/// The object is that journal entry plus `arguments` after prepare and coerce.
/// `conversationId` is 0 because nothing is committed.
pub fn run_tool_round(
    call_id: &str,
    name: &str,
    arguments: Value,
    schema: Option<&Value>,
) -> Result<Value, String> {
    let mut prepared = prepare_call(call_id, name, arguments)?;
    if let Some(schema) = schema {
        prepared.arguments = ez4a_types::coerce(schema, &prepared.arguments);
    }
    let result = if queued_tool(&prepared.name) {
        ToolExecution {
            call_id: prepared.call_id.clone(),
            name: prepared.name.clone(),
            is_error: false,
            content: format!("queued:{}", prepared.name),
        }
    } else {
        harness_error("unknown_tool", &prepared.name)
    };
    let mut entry = tool_result_entry(
        UNCOMMITTED_CONVERSATION,
        &prepared.call_id,
        &prepared.name,
        &result,
    );
    if let Some(object) = entry.as_object_mut() {
        object.insert("arguments".to_owned(), prepared.arguments);
    }
    Ok(entry)
}

fn queued_tool(name: &str) -> bool {
    matches!(name, "read" | "write" | "edit" | "bash" | "powershell")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn null_arguments_become_empty_object_and_unknown_tool_is_an_error() {
        let schema = json!({
            "type": "object",
            "properties": { "timeout": { "type": "integer" } }
        });
        for name in ["read", "write", "edit", "bash", "powershell"] {
            let entry = run_tool_round("c1", name, Value::Null, Some(&schema)).unwrap();
            assert_eq!(entry["arguments"], json!({}), "{name}");
            assert_eq!(entry["isError"], json!(false), "{name}");
            assert_eq!(entry["content"], json!(format!("queued:{name}")), "{name}");
            assert_eq!(entry["toolCallId"], json!("c1"), "{name}");
            assert_eq!(entry["name"], json!(name), "{name}");
            assert_eq!(entry["role"], json!("tool"), "{name}");
            assert_eq!(entry["conversationId"], json!(0), "{name}");
        }

        let coerced = run_tool_round(
            "c2",
            "bash",
            json!({ "timeout": "20" }),
            Some(&schema),
        )
        .unwrap();
        assert_eq!(coerced["arguments"], json!({ "timeout": 20 }));
        assert_eq!(coerced["content"], json!("queued:bash"));
        assert_eq!(coerced["isError"], json!(false));

        let unknown = run_tool_round("c9", "grep", Value::Null, None).unwrap();
        assert_eq!(unknown["arguments"], json!({}));
        assert_eq!(unknown["isError"], json!(true));
        assert_eq!(unknown["toolCallId"], json!("c9"));
        assert_eq!(unknown["name"], json!("grep"));
        assert_eq!(unknown["content"], json!("unknown_tool: grep"));
    }
}
