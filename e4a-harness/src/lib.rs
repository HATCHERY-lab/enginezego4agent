//! Generation harness. The model call sits between commits and does not hold the store writer.

mod compaction;
mod context;
mod define;
mod errors;
mod generation;
mod inbox;
mod json_merge;
mod partial;
mod prompt;
mod provider;
mod registry;
mod scheduler;
mod search;
mod submissions;
mod telemetry;
mod timers;
mod tool_task;
mod transcript;
mod truncate;
mod usage;

pub use compaction::{estimate_context, select_cut, serialize_conversation};
pub use context::{order_tool_results, Message};
pub use define::{define_extension, define_tool, ExtensionDef, ToolDef};
pub use errors::error_chain;
pub use generation::{Generation, TurnInput, TurnKind, TurnOutcome, MODEL_CALL_CAP, RESTART_CAP};
pub use inbox::{has_queued_reset, prepare_boundary, withdraw, InboxItem, QueueMode};
pub use json_merge::merge_object;
pub use partial::{push_delta, PartialText};
pub use prompt::{plan_system_entries, replay_sections, Section};
pub use provider::ensure_provider_session_id;
pub use registry::Registry;
pub use scheduler::{Scheduler, Waiter};
pub use search::find_line;
pub use submissions::{admit, withdraw_kind, Submission};
pub use telemetry::TELEMETRY_SCHEMA;
pub use timers::backoff_ms;
pub use tool_task::ToolPhase;
pub use transcript::{render, visible_lines, TranscriptLine};
pub use truncate::truncate_chars;
pub use usage::usage_doc;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<e4a_session::Error> for Error {
    fn from(error: e4a_session::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<e4a_llm::Error> for Error {
    fn from(error: e4a_llm::Error) -> Self {
        Self(error.to_string())
    }
}
