//! Generation harness. The model call sits between commits and does not hold the store writer.

mod compaction;
mod compaction_job;
mod context;
mod context_bounds;
mod define;
mod errors;
mod generation;
mod generation_plan;
mod graph;
mod harness_open;
mod inbox;
mod json_merge;
mod live_run;
mod open_opts;
mod partial;
mod prompt;
mod provider;
mod registry;
mod run_state;
mod scheduler;
mod scheduler_js;
mod search;
mod signals;
mod submissions;
mod telemetry;
mod timers;
mod tool_path;
mod tool_prep;
mod tool_run;
mod tool_search;
mod tool_task;
mod transcript;
mod truncate;
mod usage;
mod view;

pub use compaction::{estimate_context, select_cut, serialize_conversation};
pub use compaction_job::{should_compact, summary_entry, summary_instruction, Settings};
pub use context::{order_tool_results, Message};
pub use context_bounds::{clamp_bounds, token_estimate, within, Bounds};
pub use define::{define_extension, define_tool, ExtensionDef, ToolDef};
pub use errors::error_chain;
pub use generation::{Generation, TurnInput, TurnKind, TurnOutcome, MODEL_CALL_CAP, RESTART_CAP};
pub use generation_plan::{message_chars, plan, stop_reason, PlanStep};
pub use graph::{children, roots, would_cycle, TaskNode};
pub use harness_open::open_sqlite;
pub use inbox::{has_queued_reset, prepare_boundary, withdraw, InboxItem, QueueMode};
pub use json_merge::merge_object;
pub use live_run::{
    add_compaction_status, clear_progress, end_run, fail_compaction_status, finish_slot,
    remove_compaction_status, remove_nested_slots, tool_slot, tool_slot_mut, CompactionStatus,
    LiveRun, ToolSlot,
};
pub use open_opts::{
    normalize_open, AgentSettings, Effort, ModelRef, OpenOptions, ThinkingLevel,
    DEFAULT_RESTART_LIMIT,
};
pub use partial::{push_delta, PartialText};
pub use prompt::{plan_system_entries, replay_sections, Section};
pub use provider::ensure_provider_session_id;
pub use registry::Registry;
pub use run_state::{
    advance, blocked_reason, blocked_subject, ended_by_crashes, next_attempt, BlockedReason,
    BlockedSubject, Invocation, RegisteredDefinition, RunEvent, RunPhase,
};
pub use scheduler::{Scheduler, Waiter};
pub use scheduler_js::{migrate, note_crash, pick_ready, request_abort, Task};
pub use search::find_line;
pub use signals::{cancel_waits, park, resume, signal, Signal};
pub use submissions::{admit, withdraw_kind, Submission};
pub use telemetry::TELEMETRY_SCHEMA;
pub use timers::backoff_ms;
pub use tool_path::run_tool_round;
pub use tool_prep::{prepare_call, same_call, PreparedCall};
pub use tool_run::{harness_error, nest, tool_result_entry, NestedCall, ToolExecution};
pub use tool_search::{matching_names, resolve_refs};
pub use tool_task::ToolPhase;
pub use transcript::{render, visible_lines, TranscriptLine};
pub use truncate::truncate_chars;
pub use usage::usage_doc;
pub use view::window;

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
