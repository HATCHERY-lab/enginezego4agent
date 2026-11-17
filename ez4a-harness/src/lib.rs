//! Generation harness. The model call sits between commits and does not hold the store writer.

mod advisor;
mod advisor_agent;
mod broker;
mod compaction;
mod compaction_job;
mod compaction_model;
mod compaction_run;
mod compressor_agent;
mod context;
mod context_bounds;
mod context_load;
mod define;
mod deputy_agent;
mod dev_agent;
mod manager_agent;
mod errors;
mod generation;
mod generation_plan;
mod graph;
mod harness_open;
mod inbox;
mod jev;
mod json_merge;
mod live_run;
mod notify_pass;
mod open_opts;
mod overlay;
mod overseer;
mod partial;
mod prompt;
mod provider;
mod provider_caps;
mod registry;
mod role_catalog;
mod run_state;
mod scheduler;
mod shepherd;
mod scheduler_drive;
mod scheduler_js;
mod search;
mod signals;
mod sleep;
mod slot;
mod ssh_env;
mod subagent;
mod submissions;
mod telemetry;
mod timers;
mod tool_path;
mod tool_prep;
mod tool_run;
mod tool_search;
mod tool_search_call;
mod tool_task;
mod transcript;
mod truncate;
mod usage;
mod view;
mod view_sub;
mod provider_close;
mod scheduler_loop;
mod gate;
mod compact_hook;
mod advisor_turn;
mod view_follow;
mod gate_board;

pub use advisor::advisor_tools;
pub use advisor_agent::{advisor_agent, advisor_fallback};
pub use broker::{broker, KeyBroker};
pub use compaction::{estimate_context, select_cut, serialize_conversation};
pub use compaction_job::{should_compact, summary_entry, summary_instruction, Settings};
pub use compaction_model::compact_model;
pub use compaction_run::compact_with;
pub use compressor_agent::{compressor_agent, summary_max_tokens};
pub use context::{order_tool_results, Message};
pub use context_bounds::{clamp_bounds, token_estimate, within, Bounds};
pub use context_load::{load_lines, Line};
pub use define::{define_extension, define_tool, ExtensionDef, ToolDef};
pub use dev_agent::{dev_agent, fallback_model, AgentRole};
pub use deputy_agent::{deputy_agent, DEPUTY_INSTRUCTIONS};
pub use manager_agent::{hatchery_admin_agent, manager_agent};
pub use errors::error_chain;
pub use generation::{Generation, TurnInput, TurnKind, TurnOutcome, MODEL_CALL_CAP, RESTART_CAP};
pub use generation_plan::{message_chars, plan, stop_reason, PlanStep};
pub use graph::{children, roots, would_cycle, TaskNode};
pub use harness_open::open_sqlite;
pub use inbox::{has_queued_reset, prepare_boundary, withdraw, InboxItem, QueueMode};
pub use jev::{decide, jev_tool_schemas, parse, parse_line, prompt, JevDecision, JevSnap};
pub use json_merge::merge_object;
pub use live_run::{
    add_compaction_status, clear_progress, end_run, fail_compaction_status, finish_slot,
    remove_compaction_status, remove_nested_slots, tool_slot, tool_slot_mut, CompactionStatus,
    LiveRun, ToolSlot,
};
pub use notify_pass::{FinalizePass, PendingPass, RunnablePass};
pub use open_opts::{
    normalize_open, AgentSettings, Effort, ModelRef, OpenOptions, ThinkingLevel,
    DEFAULT_RESTART_LIMIT,
};
pub use overlay::{after_tool, Overlay};
pub use overseer::{Action, Directive, Kind, Observation, Overseer, OverseerConfig, SessionId};
pub use partial::{push_delta, PartialText};
pub use prompt::{plan_system_entries, replay_sections, Section};
pub use provider::ensure_provider_session_id;
pub use provider_caps::{wire_for, ProviderCaps, WireAction};
pub use registry::Registry;
pub use role_catalog::{find, load, builtin, RoleDef};
pub use shepherd::{decide as decide_directive, flock, ApprovalVerdict, Order, Policy};
pub use run_state::{
    advance, blocked_reason, blocked_subject, ended_by_crashes, next_attempt, BlockedReason,
    BlockedSubject, Invocation, RegisteredDefinition, RunEvent, RunPhase,
};
pub use scheduler::{Scheduler, Waiter};
pub use scheduler_drive::{on_abort, on_crash, step};
pub use scheduler_js::{migrate, note_crash, pick_ready, request_abort, Task};
pub use search::find_line;
pub use signals::{cancel_waits, park, resume, signal, Signal};
pub use sleep::sleep_until;
pub use slot::{ProviderSlot, EZ4A_SLOT};
pub use ssh_env::{parse_target, SshTarget};
pub use subagent::{spawn_child, Child};
pub use submissions::{admit, withdraw_kind, Submission};
pub use telemetry::TELEMETRY_SCHEMA;
pub use timers::backoff_ms;
pub use tool_path::run_tool_round;
pub use tool_prep::{prepare_call, same_call, PreparedCall};
pub use tool_run::{harness_error, nest, tool_result_entry, NestedCall, ToolExecution};
pub use tool_search::{matching_names, resolve_refs};
pub use tool_search_call::answer;
pub use tool_task::ToolPhase;
pub use transcript::{render, visible_lines, TranscriptLine};
pub use truncate::truncate_chars;
pub use usage::usage_doc;
pub use view::window;
pub use view_sub::on_lines;
pub use provider_close::ProviderSessions;
pub use scheduler_loop::{crash_task, drive_pass};
pub use gate::Gate;
pub use provider_close::shutdown;
pub use compact_hook::compact_if_needed;
pub use advisor_turn::advise;
pub use view_follow::follow_lines;
pub use broker::resolve;
pub use gate_board::{park_turn, signal_turn};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<ez4a_session::Error> for Error {
    fn from(error: ez4a_session::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<ez4a_llm::Error> for Error {
    fn from(error: ez4a_llm::Error) -> Self {
        Self(error.to_string())
    }
}
