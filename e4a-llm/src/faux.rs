//! Scripted assistant turn. No key and no network: tests replay text and at most one tool call.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Error, EventStream, Llm, LlmEvent, ToolCall, TurnRequest, Usage};

/// Replays one scripted turn. Each [`Llm::stream`] call counts, including a scripted failure.
#[derive(Debug)]
pub struct FauxLlm {
    text: String,
    tool_call: Option<ToolCall>,
    usage: Usage,
    fail: Option<String>,
    calls: AtomicU64,
}

impl FauxLlm {
    /// Text turn with zero token counts and provider `faux` / model `scripted`.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tool_call: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                provider: "faux".into(),
                model: "scripted".into(),
            },
            fail: None,
            calls: AtomicU64::new(0),
        }
    }

    /// Scripted failure. [`Llm::stream`] counts the call and returns the message.
    pub fn failing(message: impl Into<String>) -> Self {
        let mut llm = Self::text("");
        llm.fail = Some(message.into());
        llm
    }

    pub fn with_tool_call(mut self, call: ToolCall) -> Self {
        self.tool_call = Some(call);
        self
    }

    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = usage;
        self
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    fn events(&self) -> Vec<LlmEvent> {
        let mut events = Vec::new();
        if !self.text.is_empty() {
            events.push(LlmEvent::Text(self.text.clone()));
        }
        if let Some(call) = &self.tool_call {
            events.push(LlmEvent::ToolCall(call.clone()));
        }
        events.push(LlmEvent::Usage(self.usage.clone()));
        events
    }
}

impl Llm for FauxLlm {
    fn stream(
        &self,
        _request: &TurnRequest,
    ) -> impl Future<Output = Result<EventStream, Error>> + Send {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let fail = self.fail.clone();
        let events = self.events();
        async move {
            if let Some(message) = fail {
                return Err(Error::message(message));
            }
            Ok(EventStream::from_events(events))
        }
    }
}
