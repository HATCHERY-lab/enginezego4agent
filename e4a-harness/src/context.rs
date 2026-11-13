// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/context.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Order tool results so each one follows the assistant call that produced it.

/// One transcript message.
///
/// `calls` lists tool-call ids on an assistant message and is empty for every
/// other role. `tool_call_id` is set on a tool result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub role: String,
    pub text: String,
    pub tool_call_id: Option<String>,
    pub calls: Vec<String>,
}

/// Move each `role == "tool"` message to sit immediately after the assistant
/// whose `calls` contain its id, in call order.
///
/// The first assistant that lists the id owns it. A tool message whose id is
/// missing, or that no assistant lists, stays where it is. A call with no
/// result is left absent; nothing is synthesized.
pub fn order_tool_results(messages: &[Message]) -> Vec<Message> {
    let mut attached: Vec<Vec<usize>> = vec![Vec::new(); messages.len()];
    let mut moved = vec![false; messages.len()];
    for (index, message) in messages.iter().enumerate() {
        if message.role != "tool" {
            continue;
        }
        let Some(id) = message.tool_call_id.as_deref() else {
            continue;
        };
        let Some(assistant) = assistant_index(messages, id) else {
            continue;
        };
        attached[assistant].push(index);
        moved[index] = true;
    }
    for (assistant, group) in attached.iter_mut().enumerate() {
        if group.is_empty() {
            continue;
        }
        order_like_calls(&messages[assistant].calls, messages, group);
    }

    let mut ordered = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        if moved[index] {
            continue;
        }
        ordered.push(message.clone());
        for tool_index in &attached[index] {
            ordered.push(messages[*tool_index].clone());
        }
    }
    ordered
}

fn assistant_index(messages: &[Message], tool_call_id: &str) -> Option<usize> {
    messages.iter().position(|message| {
        message.role == "assistant" && message.calls.iter().any(|call| call == tool_call_id)
    })
}

/// Stable call order. Extra results for an id follow that id's first slot.
fn order_like_calls(calls: &[String], messages: &[Message], group: &mut Vec<usize>) {
    let mut used = vec![false; group.len()];
    let mut ordered = Vec::with_capacity(group.len());
    let mut seen: Vec<&str> = Vec::new();
    for call in calls {
        if seen.iter().any(|previous| *previous == call.as_str()) {
            continue;
        }
        seen.push(call.as_str());
        for (pos, tool_index) in group.iter().enumerate() {
            if used[pos] {
                continue;
            }
            if messages[*tool_index].tool_call_id.as_deref() == Some(call.as_str()) {
                used[pos] = true;
                ordered.push(*tool_index);
            }
        }
    }
    for (pos, tool_index) in group.iter().enumerate() {
        if !used[pos] {
            ordered.push(*tool_index);
        }
    }
    *group = ordered;
}

#[cfg(test)]
mod tests {
    use super::{order_tool_results, Message};

    fn message(role: &str, text: &str, tool_call_id: Option<&str>, calls: &[&str]) -> Message {
        Message {
            role: role.to_owned(),
            text: text.to_owned(),
            tool_call_id: tool_call_id.map(str::to_owned),
            calls: calls.iter().copied().map(str::to_owned).collect(),
        }
    }

    fn texts(messages: &[Message]) -> Vec<&str> {
        messages.iter().map(|message| message.text.as_str()).collect()
    }

    #[test]
    fn tool_results_follow_their_assistant_in_call_order() {
        let messages = vec![
            message("user", "go", None, &[]),
            message("assistant", "call", None, &["a", "b"]),
            message("tool", "rb", Some("b"), &[]),
            message("user", "between", None, &[]),
            message("tool", "ra", Some("a"), &[]),
            message("assistant", "done", None, &[]),
        ];
        let ordered = order_tool_results(&messages);
        assert_eq!(
            texts(&ordered),
            ["go", "call", "ra", "rb", "between", "done"]
        );
    }

    #[test]
    fn missing_id_stays_in_place() {
        let messages = vec![
            message("tool", "orphan", None, &[]),
            message("user", "q", None, &[]),
            message("assistant", "call", None, &["a"]),
            message("tool", "no-such", Some("z"), &[]),
            message("tool", "ok", Some("a"), &[]),
        ];
        let ordered = order_tool_results(&messages);
        assert_eq!(texts(&ordered), ["orphan", "q", "call", "ok", "no-such"]);
    }

    #[test]
    fn tool_before_its_assistant_moves_after_it() {
        let messages = vec![
            message("tool", "early", Some("a"), &[]),
            message("user", "q", None, &[]),
            message("assistant", "call", None, &["a"]),
        ];
        let ordered = order_tool_results(&messages);
        assert_eq!(texts(&ordered), ["q", "call", "early"]);
    }

    #[test]
    fn each_assistant_keeps_only_its_own_calls() {
        let messages = vec![
            message("assistant", "first", None, &["a"]),
            message("assistant", "second", None, &["a", "b"]),
            message("tool", "tb", Some("b"), &[]),
            message("tool", "ta", Some("a"), &[]),
        ];
        let ordered = order_tool_results(&messages);
        assert_eq!(texts(&ordered), ["first", "ta", "second", "tb"]);
    }

    #[test]
    fn missing_result_is_not_synthesized() {
        let messages = vec![
            message("assistant", "call", None, &["a", "b"]),
            message("tool", "only-a", Some("a"), &[]),
        ];
        let ordered = order_tool_results(&messages);
        assert_eq!(ordered.len(), 2);
        assert_eq!(texts(&ordered), ["call", "only-a"]);
        assert!(ordered
            .iter()
            .filter(|message| message.role != "assistant")
            .all(|message| message.calls.is_empty()));
    }
}
