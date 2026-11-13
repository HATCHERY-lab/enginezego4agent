//! Scripted assistant turns. No key and no network: tests replay text and at most one tool call.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::{Error, EventStream, Llm, LlmEvent, ToolCall, TurnRequest, Usage};

#[derive(Clone, Debug)]
struct Script {
    text: String,
    tool_call: Option<ToolCall>,
    usage: Usage,
    fail: Option<String>,
}

impl Script {
    fn text(text: String) -> Self {
        Self {
            text,
            tool_call: None,
            usage: Usage {
                input_tokens: 0,
                output_tokens: 0,
                provider: "faux".into(),
                model: "scripted".into(),
            },
            fail: None,
        }
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

/// Replays scripted turns in order. After the last script, further calls replay that script.
/// One script therefore returns the same turn on every call. Each [`Llm::stream`] counts,
/// including a scripted failure.
#[derive(Debug)]
pub struct FauxLlm {
    turns: Vec<Script>,
    cursor: AtomicU64,
    calls: AtomicU64,
    seen: Mutex<Vec<TurnRequest>>,
}

impl FauxLlm {
    /// Text turn with zero token counts and provider `faux` / model `scripted`.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            turns: vec![Script::text(text.into())],
            cursor: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Scripted failure. [`Llm::stream`] counts the call and returns the message.
    pub fn failing(message: impl Into<String>) -> Self {
        let mut llm = Self::text("");
        llm.turns
            .first_mut()
            .expect("text() installs one script")
            .fail = Some(message.into());
        llm
    }

    /// Queue `next`'s scripts after this one's.
    pub fn then(mut self, next: Self) -> Self {
        self.turns.extend(next.turns);
        self
    }

    /// Attach a tool call to the last scripted turn.
    pub fn with_tool_call(mut self, call: ToolCall) -> Self {
        if let Some(turn) = self.turns.last_mut() {
            turn.tool_call = Some(call);
        }
        self
    }

    /// Replace usage on the last scripted turn.
    pub fn with_usage(mut self, usage: Usage) -> Self {
        if let Some(turn) = self.turns.last_mut() {
            turn.usage = usage;
        }
        self
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    /// Requests received by [`Llm::stream`], in call order.
    pub fn requests(&self) -> Vec<TurnRequest> {
        self.seen
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    fn script(&self, index: u64) -> Result<&Script, Error> {
        let len = self.turns.len();
        if len == 0 {
            return Err(Error::message("faux has no script"));
        }
        let last = len.saturating_sub(1);
        let index = usize::try_from(index).unwrap_or(last);
        self.turns
            .get(index.min(last))
            .ok_or_else(|| Error::message("faux has no script"))
    }
}

impl Llm for FauxLlm {
    fn stream(
        &self,
        request: &TurnRequest,
    ) -> impl Future<Output = Result<EventStream, Error>> + Send {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let prepared = self
            .script(self.cursor.fetch_add(1, Ordering::Relaxed))
            .map(|script| (script.fail.clone(), script.events()));
        {
            self.seen
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(request.clone());
        }
        async move {
            let (fail, events) = prepared?;
            if let Some(message) = fail {
                return Err(Error::message(message));
            }
            Ok(EventStream::from_events(events))
        }
    }
}
