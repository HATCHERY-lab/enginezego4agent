// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/tool.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/tool, MIT, see LICENSES/pi-MIT.txt.

//! Tool-result data and nested-call depth.
//!
//! Nothing here spawns a process or writes the store. [`tool_result_entry`] is
//! the object a later commit can append.

use serde_json::{json, Value};

const MAX_NEST_DEPTH: u8 = 8;

/// One tool execution. `content` is the text the journal stores.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolExecution {
    pub call_id: String,
    pub name: String,
    pub is_error: bool,
    pub content: String,
}

/// An error result the harness writes itself.
///
/// `call_id` is empty, `name` is `harness`, `is_error` is true, and `content`
/// is `{code}: {message}`.
pub fn harness_error(code: &str, message: &str) -> ToolExecution {
    ToolExecution {
        call_id: String::new(),
        name: "harness".to_owned(),
        is_error: true,
        content: format!("{code}: {message}"),
    }
}

/// Journal object for one tool result.
///
/// `toolCallId` and `name` are the call being recorded. `isError` and
/// `content` come from `result`. `conversationId` is `conversation_id`.
pub fn tool_result_entry(
    conversation_id: i64,
    call_id: &str,
    name: &str,
    result: &ToolExecution,
) -> Value {
    json!({
        "role": "tool",
        "toolCallId": call_id,
        "name": name,
        "isError": result.is_error,
        "content": result.content,
        "conversationId": conversation_id,
    })
}

/// A nested tool call under `parent_call_id`. Depth 1 is the first call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NestedCall {
    pub parent_call_id: String,
    pub call_id: String,
    pub depth: u8,
}

/// Child of `parent` one level deeper.
///
/// `parent_call_id` is the parent's `call_id`. Depth starts at 1 and a depth
/// above 8 is rejected. `call_id` is stored as given; it is not joined onto
/// the parent.
pub fn nest(parent: &NestedCall, call_id: &str) -> Result<NestedCall, String> {
    let depth = parent.depth.checked_add(1).unwrap_or(u8::MAX);
    if depth > MAX_NEST_DEPTH {
        return Err(format!("nested call depth {depth} exceeds {MAX_NEST_DEPTH}"));
    }
    Ok(NestedCall {
        parent_call_id: parent.call_id.clone(),
        call_id: call_id.to_owned(),
        depth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_error_names_the_harness_and_joins_code_and_message() {
        let error = harness_error("tool_unavailable", "Tool read is not available");
        assert_eq!(error.call_id, "");
        assert_eq!(error.name, "harness");
        assert!(error.is_error);
        assert_eq!(error.content, "tool_unavailable: Tool read is not available");
    }

    #[test]
    fn tool_result_entry_is_the_journal_object() {
        let result = ToolExecution {
            call_id: String::new(),
            name: "harness".to_owned(),
            is_error: true,
            content: "blocked: no".to_owned(),
        };
        let entry = tool_result_entry(7, "call-1", "read", &result);
        assert_eq!(
            entry,
            json!({
                "role": "tool",
                "toolCallId": "call-1",
                "name": "read",
                "isError": true,
                "content": "blocked: no",
                "conversationId": 7,
            })
        );
    }

    #[test]
    fn nest_starts_at_one_and_rejects_past_eight() {
        let root = NestedCall {
            parent_call_id: String::new(),
            call_id: "root".to_owned(),
            depth: 1,
        };
        let child = nest(&root, "child").unwrap();
        assert_eq!(
            child,
            NestedCall {
                parent_call_id: "root".to_owned(),
                call_id: "child".to_owned(),
                depth: 2,
            }
        );

        let mut current = root;
        for _ in 1..MAX_NEST_DEPTH {
            current = nest(&current, "n").unwrap();
        }
        assert_eq!(current.depth, MAX_NEST_DEPTH);
        let error = nest(&current, "too-deep").unwrap_err();
        assert_eq!(error, "nested call depth 9 exceeds 8");
    }
}
