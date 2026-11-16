//! Filesystem tool slice. A waiting `pi.tool` task runs here and its result is committed.
//! [`run_waiting_tool`] does not call a model. [`run_until_answer`] drives the generation
//! until it answers.

mod advisor_loop;
mod coerce;
mod edit_diff;
mod image_guard;
mod jev_gate;
mod mutation_queue;
mod photon;
mod shell;
mod workspace;

pub use advisor_loop::{advisor_doc, review, AdvisoryNote};
pub use coerce::coerce;
pub use edit_diff::apply_hunks;
pub use image_guard::image_refusal;
pub use jev_gate::run_jev_gate;
pub use mutation_queue::{EmptyPath, Mutation, MutationKind, Queue};
pub use photon::{plan, runtime, PhotonOp};
pub use resume::{resume_turns, resume_waiting};
pub use workspace::{ToolEffect, Workspace};

mod resume;

use e4a_harness::{Generation, TurnInput, TurnKind, TurnOutcome, MODEL_CALL_CAP};
use e4a_llm::Llm;
use e4a_session::{EntryDraft, EntryRecord, Session, Tx};
use e4a_types::{Json, TaskOutcome, TaskOutcomeError, TaskState};
use serde_json::json;

static CANCEL_SESSION: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// The ACP client asked to stop one session. `"*"` stops whichever turn takes it.
pub fn request_cancel_session(session_id: &str) {
    if let Ok(mut guard) = CANCEL_SESSION.lock() {
        *guard = Some(session_id.to_string());
    }
}

/// The ACP client asked to stop the foreground turn.
pub fn request_cancel() {
    request_cancel_session("*");
}

fn take_cancel_for(session_id: &str) -> bool {
    let Ok(mut guard) = CANCEL_SESSION.lock() else {
        return false;
    };
    let hit = match guard.as_deref() {
        Some("*") => true,
        Some(id) => id == session_id,
        None => false,
    };
    if hit {
        *guard = None;
    }
    hit
}

const TOOL_KIND: &str = "pi.tool";
const TOOL_RESULT_KIND: &str = "pi.toolResult";
const ASSISTANT_KIND: &str = "pi.assistant";

/// Committed result of the waiting tool task. The parent generation stays waiting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOutcome {
    pub generation_task_id: i64,
    pub tool_task_id: i64,
    pub entry_id: i64,
    pub name: String,
    pub is_error: bool,
    pub content: String,
}

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

impl From<e4a_harness::Error> for Error {
    fn from(error: e4a_harness::Error) -> Self {
        Self(error.to_string())
    }
}

struct PendingTool {
    conversation_id: i64,
    tool_task_id: i64,
    call_id: String,
    name: String,
    arguments: Json,
}

/// Run one generation until it answers or the model-call cap stops it.
///
/// [`Generation::turn`] runs once. While the outcome is [`TurnKind::WaitingTool`],
/// the waiting tool runs and [`Generation::continue_after_tool`] resumes the same
/// generation. [`TurnKind::Answered`] and [`TurnKind::RestartLimit`] stop the loop.
/// An error from `continue_after_tool` is returned. At most [`MODEL_CALL_CAP`]
/// tool rounds run, so a stuck `WaitingTool` cannot spin.
pub async fn run_until_answer<L: Llm>(
    session: &Session,
    llm: &L,
    workspace: &Workspace,
    input: &TurnInput,
) -> Result<TurnOutcome, Error> {
    run_until_answer_in(session, llm, workspace, input, "").await
}

/// Same as [`run_until_answer`], but a matching [`request_cancel_session`] stops the loop.
pub async fn run_until_answer_in<L: Llm>(
    session: &Session,
    llm: &L,
    workspace: &Workspace,
    input: &TurnInput,
    session_id: &str,
) -> Result<TurnOutcome, Error> {
    if take_cancel_for(session_id) {
        return Err(Error::message("cancelled"));
    }
    let mut outcome = Generation::turn(session, llm, input).await?;
    let mut iterations = 0_i64;
    while matches!(outcome.kind, TurnKind::WaitingTool { .. }) {
        if take_cancel_for(session_id) {
            return Err(Error::message("cancelled"));
        }
        if iterations >= MODEL_CALL_CAP {
            break;
        }
        run_round(session, outcome.task_id, workspace).await?;
        outcome = Generation::bind(outcome.task_id, input.conversation_id)
            .continue_after_tool(session, llm)
            .await?;
        iterations = iterations.saturating_add(1);
    }
    Ok(outcome)
}

/// Run the waiting `pi.tool` task pointed at by the generation checkpoint `tools[0]`.
///
/// Arguments are read from the assistant entry. The tool runs outside
/// [`Session::commit`]. One commit appends a `pi.toolResult` entry and marks the
/// tool task terminal, completed or failed with the tool error. This does not
/// call a model and does not resume the generation.
pub async fn run_waiting_tool(
    session: &Session,
    generation_task_id: i64,
    workspace: &Workspace,
) -> Result<ToolOutcome, Error> {
    let (_, tool_task_id) = waiting_of(session, generation_task_id).await?;
    run_waiting_gated(session, generation_task_id, tool_task_id, workspace).await
}

/// Run every waiting tool of one generation's current round, in call order.
/// The generation is released only when the whole batch is terminal, which
/// [`Generation::release_finished_tool`] enforces on its side.
pub async fn run_round(
    session: &Session,
    generation_task_id: i64,
    workspace: &Workspace,
) -> Result<Vec<ToolOutcome>, Error> {
    let (state, _) = waiting_of(session, generation_task_id).await?;
    let ids = waiting_tool_ids(generation_task_id, &state)?;
    let mut outcomes = Vec::with_capacity(ids.len());
    for tool_task_id in ids {
        outcomes.push(
            run_waiting_gated(session, generation_task_id, tool_task_id, workspace).await?,
        );
    }
    Ok(outcomes)
}

/// The generation's waiting state and its first tool task id.
async fn waiting_of(
    session: &Session,
    generation_task_id: i64,
) -> Result<(e4a_types::TaskState, i64), Error> {
    let generation = session
        .task(generation_task_id)
        .await?
        .ok_or_else(|| Error::message(format!("task {generation_task_id} does not exist")))?;
    let first = waiting_tool_id(generation_task_id, &generation.state)?;
    Ok((generation.state, first))
}

async fn run_waiting_gated(
    session: &Session,
    generation_task_id: i64,
    tool_task_id: i64,
    workspace: &Workspace,
) -> Result<ToolOutcome, Error> {
    let pending = load_pending(session, generation_task_id, tool_task_id).await?;
    let effect = if pending.name == "child" {
        child_effect(
            generation_task_id,
            pending.tool_task_id,
            &pending.arguments,
            workspace,
        )
    } else {
        let decision = e4a_harness::decide(e4a_harness::JevSnap {
            tool: &pending.name,
            arguments: &pending.arguments,
            transcript_chars: 0,
        });
        match decision {
            e4a_harness::JevDecision::Stop { reason } => ToolEffect {
                is_error: true,
                content: reason.to_string(),
            },
            e4a_harness::JevDecision::Steer { text } => ToolEffect {
                is_error: true,
                content: text,
            },
            e4a_harness::JevDecision::Continue | e4a_harness::JevDecision::Compact => {
                workspace.call(&pending.name, &pending.arguments)
            }
        }
    };
    e4a_harness::signal_turn(generation_task_id)?;
    let entry_id = session
        .commit(|tx| commit_tool_result(tx, &pending, &effect))
        .await?;
    Ok(ToolOutcome {
        generation_task_id,
        tool_task_id: pending.tool_task_id,
        entry_id,
        name: pending.name,
        is_error: effect.is_error,
        content: effect.content,
    })
}

/// Record a child, then run its command in the workspace root.
///
/// An empty command does not spawn. `spawn_child` only records the child.
/// [`e4a_env::LocalEnv::start_child`] is the process. A non-zero exit is an
/// error result whose text is stderr. A zero exit is stdout.
fn child_effect(
    generation_task_id: i64,
    tool_task_id: i64,
    arguments: &Json,
    workspace: &Workspace,
) -> ToolEffect {
    let command = arguments
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if command.is_empty() {
        return ToolEffect {
            is_error: true,
            content: "command is empty".to_owned(),
        };
    }
    let child = match e4a_harness::spawn_child(generation_task_id, command, tool_task_id) {
        Ok(child) => child,
        Err(text) => {
            return ToolEffect {
                is_error: true,
                content: text,
            };
        }
    };
    let env = match e4a_env::LocalEnv::new(workspace.root()) {
        Ok(env) => env,
        Err(error) => {
            return ToolEffect {
                is_error: true,
                content: error.to_string(),
            };
        }
    };
    match env.start_child(&child.tool) {
        Ok(output) if output.exit_code == 0 => ToolEffect {
            is_error: false,
            content: output.stdout,
        },
        Ok(output) => ToolEffect {
            is_error: true,
            content: output.stderr,
        },
        Err(error) => ToolEffect {
            is_error: true,
            content: error.to_string(),
        },
    }
}

async fn load_pending(
    session: &Session,
    generation_task_id: i64,
    tool_task_id: i64,
) -> Result<PendingTool, Error> {
    let generation = session
        .task(generation_task_id)
        .await?
        .ok_or_else(|| Error::message(format!("task {generation_task_id} does not exist")))?;
    let _ = waiting_tool_ids(generation_task_id, &generation.state)?;
    let tool = session
        .task(tool_task_id)
        .await?
        .ok_or_else(|| Error::message(format!("task {tool_task_id} does not exist")))?;
    if tool.kind != TOOL_KIND {
        return Err(Error::message(format!(
            "task {} is {}, not a tool",
            tool.id, tool.kind
        )));
    }
    if !matches!(tool.state, TaskState::Waiting { .. }) {
        return Err(Error::message(format!(
            "tool task {} is not waiting",
            tool.id
        )));
    }
    if tool.owner != Some(generation_task_id) {
        return Err(Error::message(format!(
            "tool task {} is not owned by generation {generation_task_id}",
            tool.id
        )));
    }
    if tool.conversation_id != generation.conversation_id {
        return Err(Error::message(format!(
            "tool task {} is in another conversation",
            tool.id
        )));
    }
    let assistant_id = tool
        .input
        .get("assistant")
        .and_then(Json::as_i64)
        .ok_or_else(|| Error::message("tool task is missing assistant"))?;
    let call_id = tool
        .input
        .get("callId")
        .and_then(Json::as_str)
        .ok_or_else(|| Error::message("tool task is missing callId"))?
        .to_string();
    let name = tool
        .input
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| Error::message("tool task is missing name"))?
        .to_string();
    let entry = session
        .entry(assistant_id)
        .await?
        .ok_or_else(|| Error::message(format!("assistant entry {assistant_id} does not exist")))?;
    if entry.kind != ASSISTANT_KIND {
        return Err(Error::message(format!(
            "entry {assistant_id} is not an assistant entry"
        )));
    }
    if entry.conversation_id != tool.conversation_id {
        return Err(Error::message(format!(
            "assistant entry {assistant_id} is in another conversation"
        )));
    }
    let arguments = tool_arguments(&entry, &call_id, &name)?;
    Ok(PendingTool {
        conversation_id: tool.conversation_id,
        tool_task_id,
        call_id,
        name,
        arguments,
    })
}

fn waiting_tool_id(generation_id: i64, state: &TaskState) -> Result<i64, Error> {
    waiting_tool_ids(generation_id, state)?
        .first()
        .copied()
        .ok_or_else(|| Error::message("tool round has no tool task"))
}

/// Every waiting tool task id of the generation's current round. Round order
/// is call order.
pub fn waiting_tool_ids(generation_id: i64, state: &TaskState) -> Result<Vec<i64>, Error> {
    let TaskState::Waiting { checkpoint, .. } = state else {
        return Err(Error::message(format!(
            "generation {generation_id} is not waiting on a tool"
        )));
    };
    if checkpoint.get("phase").and_then(Json::as_str) != Some("tools") {
        return Err(Error::message(format!(
            "generation {generation_id} is not waiting on a tool"
        )));
    }
    let ids = checkpoint
        .get("tools")
        .and_then(Json::as_array)
        .map(|tools| tools.iter().filter_map(Json::as_i64).collect::<Vec<_>>())
        .unwrap_or_default();
    if ids.is_empty() {
        return Err(Error::message("tool round has no tool task"));
    }
    Ok(ids)
}

fn tool_arguments(entry: &EntryRecord, call_id: &str, name: &str) -> Result<Json, Error> {
    let messages = entry
        .model
        .as_ref()
        .and_then(Json::as_array)
        .ok_or_else(|| Error::message("assistant entry has no model messages"))?;
    for message in messages {
        let Some(parts) = message.get("content").and_then(Json::as_array) else {
            continue;
        };
        for part in parts {
            if part.get("type").and_then(Json::as_str) != Some("toolCall") {
                continue;
            }
            if part.get("id").and_then(Json::as_str) != Some(call_id) {
                continue;
            }
            if part.get("name").and_then(Json::as_str) != Some(name) {
                continue;
            }
            return Ok(part.get("arguments").cloned().unwrap_or(Json::Null));
        }
    }
    Err(Error::message(format!(
        "assistant entry has no tool call {call_id}"
    )))
}

fn commit_tool_result(
    tx: &mut Tx,
    pending: &PendingTool,
    effect: &ToolEffect,
) -> Result<i64, e4a_session::Error> {
    let mut tool = tx.task(pending.tool_task_id)?.ok_or_else(|| {
        e4a_session::Error::message(format!("task {} does not exist", pending.tool_task_id))
    })?;
    if !matches!(tool.state, TaskState::Waiting { .. }) {
        return Err(e4a_session::Error::message(format!(
            "tool task {} is not waiting",
            pending.tool_task_id
        )));
    }
    let entry = tx.append_entry(
        pending.conversation_id,
        EntryDraft {
            kind: TOOL_RESULT_KIND.into(),
            model: Some(json!([{
                "role": "tool",
                "toolCallId": pending.call_id,
                "name": pending.name,
                "isError": effect.is_error,
                "content": effect.content,
            }])),
            data: None,
            head: None,
            edits: None,
        },
    )?;
    tool.state = if effect.is_error {
        TaskState::Terminal {
            outcome: TaskOutcome::Failed {
                error: TaskOutcomeError {
                    message: effect.content.clone(),
                    detail: None,
                },
                result: Some(json!({ "entryId": entry.id })),
            },
        }
    } else {
        TaskState::Terminal {
            outcome: TaskOutcome::Completed {
                result: json!({ "entryId": entry.id }),
            },
        }
    };
    tx.set_task(tool)?;
    Ok(entry.id)
}
