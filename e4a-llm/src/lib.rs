//! Model turns. [`Llm`] yields text, tool-call, and usage events and does not speak HTTP.

mod faux;
mod openrouter;

pub use faux::FauxLlm;
pub use openrouter::OpenRouterClient;

use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// One chat message offered to a model.
///
/// Tool results set [`Self::tool_call_id`] and [`Self::is_error`]. Both default
/// empty so a message written before those fields still parses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// JSON schema of one tool the model may call. This slice does not run it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// One model-issued call. Arguments are the provider JSON object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Token counts for one turn. Cost is recorded by the harness; faux turns use zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub provider: String,
    pub model: String,
}

/// Events a streamed turn yields. HTTP frames never cross this type.
#[derive(Clone, Debug, PartialEq)]
pub enum LlmEvent {
    Text(String),
    ToolCall(ToolCall),
    Usage(Usage),
}

/// Messages plus the tool schemas offered for one turn.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSchema>,
}

/// Pull iterator of [`LlmEvent`]. Polling it must not touch the session store.
#[derive(Debug)]
pub struct EventStream {
    inner: std::vec::IntoIter<LlmEvent>,
}

impl EventStream {
    pub(crate) fn from_events(events: Vec<LlmEvent>) -> Self {
        Self {
            inner: events.into_iter(),
        }
    }

    pub async fn next(&mut self) -> Option<LlmEvent> {
        self.inner.next()
    }
}

/// Stream one assistant turn. Implementations own any network; the trait does not.
pub trait Llm: Send + Sync {
    fn stream(
        &self,
        request: &TurnRequest,
    ) -> impl Future<Output = Result<EventStream, Error>> + Send;
}
