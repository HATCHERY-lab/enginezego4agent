// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/generation.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/generation, MIT, see LICENSES/pi-MIT.txt.

//! Step list for one generation, fixed before the model is called.
//!
//! Upstream walks `prepare`, then `request` (the model call), then appends the
//! assistant message, starts a tool round when the answer has tool calls, and
//! settles the run. This module only names that order. It does not call a model.

/// One step of a generation, in the order [`plan`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStep {
    /// Render the system prompt and tool loadout, then commit the request.
    Prepare,
    /// The request checkpoint. The model call itself is not this step.
    Request,
    /// Append the assistant message after the response is classified.
    AppendAssistant,
    /// Start the tool round for calls on that assistant message.
    AppendTool,
    /// Settle the run's inputs. Always last.
    Settle,
}

/// Steps for one turn.
///
/// Always `Prepare`, `Request`, `AppendAssistant`. `AppendTool` is inserted
/// only when `has_tools`. `Settle` is always last.
pub fn plan(has_tools: bool) -> Vec<PlanStep> {
    let mut steps = Vec::with_capacity(if has_tools { 5 } else { 4 });
    steps.push(PlanStep::Prepare);
    steps.push(PlanStep::Request);
    steps.push(PlanStep::AppendAssistant);
    if has_tools {
        steps.push(PlanStep::AppendTool);
    }
    steps.push(PlanStep::Settle);
    steps
}

/// Stop reason stored on the assistant message.
///
/// `failed` wins, including when a tool was also seen. Otherwise a tool yields
/// `tool_use`, and a plain answer yields `end_turn`.
pub fn stop_reason(saw_tool: bool, failed: bool) -> &'static str {
    if failed {
        "error"
    } else if saw_tool {
        "tool_use"
    } else {
        "end_turn"
    }
}

/// Unicode scalar count of `text`. Not the byte length.
pub fn message_chars(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::{message_chars, plan, stop_reason, PlanStep};

    #[test]
    fn plan_without_tools_ends_at_settle() {
        assert_eq!(
            plan(false),
            vec![
                PlanStep::Prepare,
                PlanStep::Request,
                PlanStep::AppendAssistant,
                PlanStep::Settle,
            ]
        );
    }

    #[test]
    fn plan_with_tools_inserts_append_tool_before_settle() {
        assert_eq!(
            plan(true),
            vec![
                PlanStep::Prepare,
                PlanStep::Request,
                PlanStep::AppendAssistant,
                PlanStep::AppendTool,
                PlanStep::Settle,
            ]
        );
    }

    #[test]
    fn stop_reason_prefers_error_then_tool_use() {
        assert_eq!(stop_reason(false, true), "error");
        assert_eq!(stop_reason(true, true), "error");
        assert_eq!(stop_reason(true, false), "tool_use");
        assert_eq!(stop_reason(false, false), "end_turn");
    }

    #[test]
    fn message_chars_counts_scalars_not_bytes() {
        assert_eq!(message_chars(""), 0);
        assert_eq!(message_chars("héllo"), 5);
        assert_ne!(message_chars("héllo"), "héllo".len());
        assert_eq!(message_chars("👍"), 1);
        assert_ne!(message_chars("👍"), "👍".len());
    }
}
