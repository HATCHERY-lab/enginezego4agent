//! OpenRouter is not wired.
//!
//! genai 0.7.4 streams an OpenAI-compatible chat with tools via
//! `Client::exec_chat_stream(model, ChatRequest::new(messages).with_tools(tools), Some(&ChatOptions::default().with_capture_tool_calls(true)))`.
//! `AdapterKind::OpenRouter` (`openrouter::…`) delegates to `OpenAIAdapter` at
//! `https://openrouter.ai/api/v1/`. The crate forbids its own `unsafe` and is the full
//! multi-provider HTTP client, so this slice does not depend on it and does not call it.

use std::future::Future;

use crate::{Error, EventStream, Llm, TurnRequest};

/// Typed stand-in. Constructing it does not read a key and does not open a socket.
#[derive(Clone, Debug)]
pub struct OpenRouterClient {
    model: String,
}

impl OpenRouterClient {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }
}

impl Llm for OpenRouterClient {
    fn stream(
        &self,
        _request: &TurnRequest,
    ) -> impl Future<Output = Result<EventStream, Error>> + Send {
        let _model = self.model();
        async { Err(Error::message("live client not wired")) }
    }
}
