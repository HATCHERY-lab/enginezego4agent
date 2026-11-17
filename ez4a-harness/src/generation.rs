// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/generation.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! One generation: commit the request, call the model outside the writer, commit the answer.
//! A `request` checkpoint with no assistant entry is a failed attempt. Five of those end the task.
//! A finished tool resumes the same generation. Four committed model calls end it.

use ez4a_llm::{ChatMessage, EventStream, Llm, LlmEvent, ToolCall, ToolSchema, TurnRequest, Usage};
use ez4a_session::{
    EntryDraft, EntryRecord, Session, SubmissionCreate, SubmissionSettlement, SubmissionStatus,
    SubmissionType, TaskCreate, TaskRecord, TaskState, Tx,
};
use ez4a_types::{
    coerce, AbortReason, EntryQuery, JoinPolicy, Json, ScanOrder, TaskOutcome, TaskOutcomeError,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::compact_hook::compact_if_needed;
use crate::compaction_job::Settings;
use crate::context_load::Line;
use crate::generation_plan::{plan, stop_reason, PlanStep};
use crate::tool_path::run_tool_round;
use crate::usage::record_usage;
use crate::Error;

/// Same cap as pi-durable `DEFAULT_RESTART_LIMIT`. The next resume after it does not call the model.
pub const RESTART_CAP: i64 = 5;

/// Model calls one generation may commit. The next call returns [`TurnKind::RestartLimit`].
pub const MODEL_CALL_CAP: i64 = 4;

const GENERATION_KIND: &str = "pi.generation";
const TOOL_KIND: &str = "pi.tool";
const TOOL_RESULT_KIND: &str = "pi.toolResult";
const USER_KIND: &str = "pi.user";
const ASSISTANT_KIND: &str = "pi.assistant";

/// User text and the tools offered for one turn. Tools are stored on the task; nothing runs them.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnInput {
    pub conversation_id: i64,
    pub user_text: String,
    pub tools: Vec<ToolSchema>,
}

/// What the last resume or turn did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TurnKind {
    Answered { entry_id: i64 },
    WaitingTool { entry_id: i64, tool_task_id: i64 },
    RestartLimit,
}

impl TurnKind {
    pub fn entry_id(&self) -> Option<i64> {
        match self {
            TurnKind::Answered { entry_id } | TurnKind::WaitingTool { entry_id, .. } => {
                Some(*entry_id)
            }
            TurnKind::RestartLimit => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnOutcome {
    pub task_id: i64,
    pub failed_attempts: i64,
    pub called_llm: bool,
    pub kind: TurnKind,
    pub tail: Vec<String>,
}

/// Durable generation task. The model call is never inside [`Session::commit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generation {
    task_id: i64,
    conversation_id: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerationInput {
    submission_id: i64,
    tools: Vec<ToolSchema>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestCheckpoint {
    phase: String,
    attempt: i64,
    cutoff: i64,
    failed_attempts: i64,
    submission_id: i64,
    /// Model calls already committed for this generation.
    #[serde(default)]
    model_calls: i64,
}

struct Answer {
    text: String,
    tools: Vec<ToolCall>,
    usage: Usage,
}

enum Live {
    Terminal(TurnOutcome),
    Waiting(TurnOutcome),
    Request(RequestCheckpoint),
}

impl Generation {
    pub fn task_id(&self) -> i64 {
        self.task_id
    }

    pub fn conversation_id(&self) -> i64 {
        self.conversation_id
    }

    /// An existing generation task. The store is not read.
    pub fn bind(task_id: i64, conversation_id: i64) -> Self {
        Self {
            task_id,
            conversation_id,
        }
    }

    /// Place the user submission and leave the generation running in `request`.
    ///
    /// The task is created at `prepare` and advanced to `request` before the commit returns.
    /// No model call happens here, so a crash leaves a request with no result.
    pub async fn commit_intent(session: &Session, input: &TurnInput) -> Result<Self, Error> {
        if input.user_text.is_empty() {
            return Err(Error::message("a turn needs user text"));
        }
        let conversation_id = input.conversation_id;
        let task_id = session
            .commit(|tx| commit_intent(tx, input))
            .await?;
        Ok(Self {
            task_id,
            conversation_id,
        })
    }

    /// Fresh turn: intent commit, model call, result commit. A fresh intent is not a failed attempt.
    pub async fn turn<L: Llm>(
        session: &Session,
        llm: &L,
        input: &TurnInput,
    ) -> Result<TurnOutcome, Error> {
        let generation = Self::commit_intent(session, input).await?;
        generation.respond(session, llm).await
    }

    /// Stream the model and commit the assistant entry. Does not count a crash.
    pub async fn respond<L: Llm>(&self, session: &Session, llm: &L) -> Result<TurnOutcome, Error> {
        self.execute(session, llm).await
    }

    /// Drop `Waiting` after the tool result is already committed.
    ///
    /// Does not call the model. The next [`Self::resume`] sees a request whose
    /// cutoff is the tool result.
    pub async fn release_finished_tool(&self, session: &Session) -> Result<(), Error> {
        session.commit(|tx| release_after_tool(tx, self)).await?;
        Ok(())
    }

    /// Resume a committed generation.
    ///
    /// `request` with no assistant entry after `cutoff` records one failed attempt. Under the cap
    /// the model is called once. At the cap the task fails and later resumes do not call it.
    pub async fn resume<L: Llm>(&self, session: &Session, llm: &L) -> Result<TurnOutcome, Error> {
        let live = session.commit(|tx| inspect(tx, self)).await?;
        match live {
            Live::Terminal(outcome) | Live::Waiting(outcome) => Ok(outcome),
            Live::Request(checkpoint) => {
                let assistant = session
                    .commit(|tx| assistant_after(tx, self.conversation_id, checkpoint.cutoff))
                    .await?;
                if let Some(entry_id) = assistant {
                    return self
                        .finish_answered(session, &checkpoint, entry_id, false)
                        .await;
                }
                if let Some(outcome) = self.at_model_cap(session, &checkpoint).await? {
                    return Ok(outcome);
                }
                let next = checkpoint.failed_attempts.saturating_add(1);
                if next > RESTART_CAP {
                    return Ok(self.limit_outcome(checkpoint.failed_attempts));
                }
                session
                    .commit(|tx| record_attempt(tx, self.task_id, &checkpoint, next))
                    .await?;
                if next >= RESTART_CAP {
                    return Ok(self.limit_outcome(next));
                }
                self.execute(session, llm).await
            }
        }
    }

    /// Resume after the caller has already run the waiting tool.
    ///
    /// A tool task that is still waiting is an error. This does not run it.
    /// The parent leaves `Waiting` for `request` at the tool-result entry, without
    /// a model call. The model then runs outside the writer.
    /// Text ends the generation. Another tool call waits again and is not run here.
    pub async fn continue_after_tool<L: Llm>(
        &self,
        session: &Session,
        llm: &L,
    ) -> Result<TurnOutcome, Error> {
        let live = session.commit(|tx| inspect(tx, self)).await?;
        match live {
            Live::Terminal(outcome) => Ok(outcome),
            Live::Request(_) => Err(Error::message("generation is not waiting on a tool")),
            Live::Waiting(_) => {
                session.commit(|tx| release_after_tool(tx, self)).await?;
                self.execute(session, llm).await
            }
        }
    }

    async fn at_model_cap(
        &self,
        session: &Session,
        checkpoint: &RequestCheckpoint,
    ) -> Result<Option<TurnOutcome>, Error> {
        if checkpoint.model_calls < MODEL_CALL_CAP {
            return Ok(None);
        }
        session
            .commit(|tx| stop_for_model_cap(tx, self.task_id))
            .await?;
        Ok(Some(self.limit_outcome(checkpoint.failed_attempts)))
    }

    async fn execute<L: Llm>(&self, session: &Session, llm: &L) -> Result<TurnOutcome, Error> {
        let live = session.commit(|tx| inspect(tx, self)).await?;
        match live {
            Live::Terminal(outcome) | Live::Waiting(outcome) => Ok(outcome),
            Live::Request(checkpoint) => {
                if let Some(outcome) = self.at_model_cap(session, &checkpoint).await? {
                    return Ok(outcome);
                }
                let request = session
                    .commit(|tx| turn_request(tx, self, &checkpoint))
                    .await?;
                let answer = collect_answer(llm, &request).await.map_err(failed_model)?;
                if !answer.tools.is_empty() {
                    crate::gate_board::park_turn(self.task_id)?;
                }
                let entry_id = session
                    .commit(|tx| commit_answer(tx, self, &checkpoint, &answer))
                    .await?;
                if answer.tools.is_empty() {
                    let lines = [Line {
                        entry_id,
                        text: answer.text.clone(),
                    }];
                    let estimated_tokens = u64::try_from(answer.usage.input_tokens)
                        .unwrap_or(0)
                        .saturating_add(u64::try_from(answer.usage.output_tokens).unwrap_or(0));
                    let settings = Settings::default();
                    match compact_if_needed(llm, &lines, estimated_tokens, 200_000, &settings)
                        .await
                    {
                        Ok(None) => {}
                        Ok(Some(summary)) => {
                            session
                                .commit(|tx| {
                                    tx.append_entry(
                                        self.conversation_id,
                                        EntryDraft {
                                            kind: "pi.compaction".into(),
                                            model: summary.get("model").cloned(),
                                            data: None,
                                            head: summary
                                                .get("head")
                                                .and_then(|value| value.as_i64()),
                                            edits: None,
                                        },
                                    )?;
                                    Ok(())
                                })
                                .await?;
                        }
                        Err(message) => return Err(Error::message(message)),
                    }
                }
                let (name, finished, ended) = if let Some(call) = answer.tools.first() {
                    (call.name.clone(), false, None)
                } else {
                    ("answer".to_owned(), true, Some("end_turn"))
                };
                let slots = [(self.task_id, name, finished)];
                session
                    .commit_live(self.conversation_id, self.task_id, &slots, ended)
                    .await?;
                let tail = crate::view_follow::follow_lines(session, self.conversation_id, 50)
                    .await
                    .map_err(Error::message)?;
                let rows = session.list_tasks(self.conversation_id).await?;
                let scheduled: Vec<crate::scheduler_js::Task> =
                    rows.iter().map(scheduler_task).collect();
                let mut pending = crate::notify_pass::PendingPass::default();
                let ready = crate::scheduler_loop::drive_pass(&mut pending, &scheduled, |_| {});
                start_ready(session, &rows, ready).await?;
                let failed_attempts = session
                    .task(self.task_id)
                    .await?
                    .and_then(|task| task.restarts)
                    .unwrap_or(checkpoint.failed_attempts);
                let kind = if !answer.tools.is_empty() {
                    let tool_task_id = tool_task_id(session, self.task_id).await?;
                    TurnKind::WaitingTool {
                        entry_id,
                        tool_task_id,
                    }
                } else {
                    TurnKind::Answered { entry_id }
                };
                Ok(TurnOutcome {
                    task_id: self.task_id,
                    failed_attempts,
                    called_llm: true,
                    kind,
                    tail,
                })
            }
        }
    }

    async fn finish_answered(
        &self,
        session: &Session,
        checkpoint: &RequestCheckpoint,
        entry_id: i64,
        called_llm: bool,
    ) -> Result<TurnOutcome, Error> {
        session
            .commit(|tx| settle_answered(tx, self.task_id, checkpoint, entry_id))
            .await?;
        Ok(TurnOutcome {
            task_id: self.task_id,
            failed_attempts: checkpoint.failed_attempts,
            called_llm,
            kind: TurnKind::Answered { entry_id },
            tail: Vec::new(),
        })
    }

    fn limit_outcome(&self, failed_attempts: i64) -> TurnOutcome {
        TurnOutcome {
            task_id: self.task_id,
            failed_attempts,
            called_llm: false,
            kind: TurnKind::RestartLimit,
            tail: Vec::new(),
        }
    }
}

fn commit_intent(tx: &mut Tx, input: &TurnInput) -> Result<i64, ez4a_session::Error> {
    let entry = tx.append_entry(
        input.conversation_id,
        EntryDraft {
            kind: USER_KIND.into(),
            model: Some(json!([{ "role": "user", "content": input.user_text }])),
            data: None,
            head: None,
            edits: None,
        },
    )?;
    let submission = tx.create_submission(SubmissionCreate {
        conversation_id: input.conversation_id,
        request_id: None,
        ty: SubmissionType::Input,
        status: SubmissionStatus::Queued,
        entry: None,
        answer: None,
        reason: None,
        detail: None,
        trace: None,
    })?;
    tx.place_submission(submission.id, entry.id)?;
    let stored = GenerationInput {
        submission_id: submission.id,
        tools: input.tools.clone(),
    };
    let task = tx.create_task(TaskCreate {
        kind: GENERATION_KIND.into(),
        version: 2,
        conversation_id: input.conversation_id,
        input: serde_json::to_value(&stored)?,
        owner: None,
        background: false,
        checkpoint: json!({ "phase": "prepare", "attempt": 1 }),
    })?;
    let mut running = task.clone();
    running.state = TaskState::Running {
        checkpoint: serde_json::to_value(RequestCheckpoint {
            phase: "request".into(),
            attempt: 1,
            cutoff: entry.id,
            failed_attempts: 0,
            submission_id: submission.id,
            model_calls: 0,
        })?,
    };
    tx.set_task(running)?;
    Ok(task.id)
}

fn inspect(tx: &mut Tx, generation: &Generation) -> Result<Live, ez4a_session::Error> {
    let task = load_task(tx, generation.task_id)?;
    match &task.state {
        TaskState::Terminal { outcome } => Ok(Live::Terminal(terminal_outcome(
            &task,
            outcome,
        )?)),
        TaskState::Waiting { checkpoint, .. } => Ok(Live::Waiting(waiting_outcome(
            generation.task_id,
            &task,
            checkpoint,
        )?)),
        TaskState::Running { checkpoint } | TaskState::Pending { checkpoint } => {
            let request = parse_request(checkpoint)?;
            Ok(Live::Request(request))
        }
        _ => Err(ez4a_session::Error::message(format!(
            "generation {} is {}",
            generation.task_id,
            task.state.status()
        ))),
    }
}

fn terminal_outcome(
    task: &TaskRecord,
    outcome: &TaskOutcome,
) -> Result<TurnOutcome, ez4a_session::Error> {
    let failed_attempts = task.restarts.unwrap_or(0);
    let kind = match outcome {
        TaskOutcome::Completed { result } => {
            let entry_id = result
                .get("entryId")
                .and_then(Json::as_i64)
                .ok_or_else(|| {
                    ez4a_session::Error::message(format!(
                        "generation {} completed without an entry",
                        task.id
                    ))
                })?;
            TurnKind::Answered { entry_id }
        }
        TaskOutcome::Failed { error, .. }
            if error
                .detail
                .as_ref()
                .and_then(|detail| detail.get("reason"))
                .and_then(Json::as_str)
                == Some("restart_limit") =>
        {
            TurnKind::RestartLimit
        }
        TaskOutcome::Failed { error, .. } => {
            return Err(ez4a_session::Error::message(error.message.clone()));
        }
        _ => {
            return Err(ez4a_session::Error::message(format!(
                "generation {} is terminal",
                task.id
            )));
        }
    };
    Ok(TurnOutcome {
        task_id: task.id,
        failed_attempts,
        called_llm: false,
        kind,
        tail: Vec::new(),
    })
}

fn waiting_outcome(
    task_id: i64,
    task: &TaskRecord,
    checkpoint: &Json,
) -> Result<TurnOutcome, ez4a_session::Error> {
    if checkpoint.get("phase").and_then(Json::as_str) != Some("tools") {
        return Err(ez4a_session::Error::message(format!(
            "generation {task_id} is waiting"
        )));
    }
    let entry_id = checkpoint
        .get("assistant")
        .and_then(Json::as_i64)
        .ok_or_else(|| ez4a_session::Error::message("tool round has no assistant entry"))?;
    let tool_task_id = first_tool_id(checkpoint)?;
    Ok(TurnOutcome {
        task_id,
        failed_attempts: task.restarts.unwrap_or(0),
        called_llm: false,
        kind: TurnKind::WaitingTool {
            entry_id,
            tool_task_id,
        },
        tail: Vec::new(),
    })
}

fn parse_request(checkpoint: &Json) -> Result<RequestCheckpoint, ez4a_session::Error> {
    if checkpoint.get("phase").and_then(Json::as_str) != Some("request") {
        return Err(ez4a_session::Error::message(
            "generation is not in request",
        ));
    }
    Ok(serde_json::from_value(checkpoint.clone())?)
}

fn assistant_after(
    tx: &mut Tx,
    conversation_id: i64,
    cutoff: i64,
) -> Result<Option<i64>, ez4a_session::Error> {
    let entries = conversation_entries(tx, conversation_id)?;
    Ok(entries
        .into_iter()
        .find(|entry| entry.kind == ASSISTANT_KIND && entry.id > cutoff)
        .map(|entry| entry.id))
}

fn record_attempt(
    tx: &mut Tx,
    task_id: i64,
    checkpoint: &RequestCheckpoint,
    failed_attempts: i64,
) -> Result<(), ez4a_session::Error> {
    let mut task = load_task(tx, task_id)?;
    task.restarts = Some(failed_attempts);
    if failed_attempts >= RESTART_CAP {
        task.state = TaskState::Terminal {
            outcome: TaskOutcome::Failed {
                error: TaskOutcomeError {
                    message: format!(
                        "The generation was interrupted {failed_attempts} times in its request phase"
                    ),
                    detail: Some(json!({ "reason": "restart_limit" })),
                },
                result: None,
            },
        };
    } else {
        let mut next = checkpoint.clone();
        next.failed_attempts = failed_attempts;
        task.state = TaskState::Running {
            checkpoint: serde_json::to_value(&next)?,
        };
    }
    tx.set_task(task)?;
    Ok(())
}

fn turn_request(
    tx: &mut Tx,
    generation: &Generation,
    checkpoint: &RequestCheckpoint,
) -> Result<TurnRequest, ez4a_session::Error> {
    let task = load_task(tx, generation.task_id)?;
    let input: GenerationInput = serde_json::from_value(task.input)
        .map_err(|err| ez4a_session::Error::message(err.to_string()))?;
    if input.submission_id != checkpoint.submission_id {
        return Err(ez4a_session::Error::message(
            "generation input does not match the request checkpoint",
        ));
    }
    let entries = conversation_entries(tx, generation.conversation_id)?;
    let covered = entries
        .iter()
        .filter(|entry| entry.kind == "pi.compaction" && entry.id <= checkpoint.cutoff)
        .filter_map(|entry| entry.head)
        .max();
    let mut messages = entries
        .into_iter()
        .filter(|entry| entry.id <= checkpoint.cutoff)
        .filter(|entry| match covered {
            Some(head) => entry.kind == "pi.compaction" || entry.id > head,
            None => true,
        })
        .filter_map(|entry| message_from_entry(&entry))
        .collect::<Vec<_>>();
    drop_unanswered_tool_calls(&mut messages);
    if messages.is_empty() {
        return Err(ez4a_session::Error::message(
            "the conversation has nothing to answer",
        ));
    }
    Ok(TurnRequest {
        messages,
        tools: input.tools,
    })
}

/// A killed tool leaves an assistant tool call with no `pi.toolResult`.
/// The next provider request must not include that call.
fn drop_unanswered_tool_calls(messages: &mut Vec<ChatMessage>) {
    let answered: std::collections::HashSet<String> = messages
        .iter()
        .filter(|message| message.role == "tool")
        .filter_map(|message| message.tool_call_id.clone())
        .collect();
    for message in messages.iter_mut() {
        message
            .tool_calls
            .retain(|call| answered.contains(&call.id));
    }
    messages.retain(|message| {
        message.role == "tool"
            || message.role == "user"
            || !message.text.is_empty()
            || !message.tool_calls.is_empty()
    });
}

fn load_task(tx: &mut Tx, task_id: i64) -> Result<TaskRecord, ez4a_session::Error> {
    tx.task(task_id)?
        .ok_or_else(|| ez4a_session::Error::message(format!("task {task_id} does not exist")))
}

fn conversation_entries(
    tx: &mut Tx,
    conversation_id: i64,
) -> Result<Vec<EntryRecord>, ez4a_session::Error> {
    let query = EntryQuery {
        conversation_id,
        min_entry_id: None,
        max_entry_id: None,
        order: Some(ScanOrder::Ascending),
    };
    let entries = tx.scan_entries(&query, usize::MAX)?;
    Ok(entries
        .into_iter()
        .filter(|entry| entry.conversation_id == conversation_id)
        .collect())
}

fn message_from_entry(entry: &EntryRecord) -> Option<ChatMessage> {
    let model = entry.model.as_ref()?.as_array()?.first()?;
    let role = model.get("role").and_then(Json::as_str).unwrap_or("user");
    if entry.kind == TOOL_RESULT_KIND || role == "tool" {
        let text = model
            .get("content")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string();
        let tool_call_id = model
            .get("toolCallId")
            .and_then(Json::as_str)
            .map(str::to_string);
        let is_error = model.get("isError").and_then(Json::as_bool).unwrap_or(false);
        return Some(ChatMessage {
            role: "tool".into(),
            text,
            tool_calls: Vec::new(),
            tool_call_id,
            is_error,
        });
    }
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    match model.get("content") {
        Some(Json::String(value)) => text = value.clone(),
        Some(Json::Array(parts)) => {
            for part in parts {
                match part.get("type").and_then(Json::as_str) {
                    Some("text") => {
                        if let Some(piece) = part.get("text").and_then(Json::as_str) {
                            text.push_str(piece);
                        }
                    }
                    Some("toolCall") => {
                        let id = part.get("id").and_then(Json::as_str);
                        let name = part.get("name").and_then(Json::as_str);
                        if let (Some(id), Some(name)) = (id, name) {
                            tool_calls.push(ToolCall {
                                id: id.to_string(),
                                name: name.to_string(),
                                arguments: part.get("arguments").cloned().unwrap_or(Json::Null),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    Some(ChatMessage {
        role: role.to_string(),
        text,
        tool_calls,
        tool_call_id: None,
        is_error: false,
    })
}

fn release_after_tool(tx: &mut Tx, generation: &Generation) -> Result<(), ez4a_session::Error> {
    let mut task = load_task(tx, generation.task_id)?;
    if task.conversation_id != generation.conversation_id {
        return Err(ez4a_session::Error::message(
            "generation is in another conversation",
        ));
    }
    let checkpoint = match &task.state {
        TaskState::Waiting { checkpoint, .. } => checkpoint.clone(),
        _ => {
            return Err(ez4a_session::Error::message(
                "generation is not waiting on a tool",
            ));
        }
    };
    if checkpoint.get("phase").and_then(Json::as_str) != Some("tools") {
        return Err(ez4a_session::Error::message(
            "generation is not waiting on a tool",
        ));
    }
    let model_calls = checkpoint_model_calls(&checkpoint);
    let tool_id_list = tool_ids(&checkpoint)?;
    let mut entry_id = None;
    for tool_task_id in tool_id_list {
        let tool = load_task(tx, tool_task_id)?;
        if tool.kind != TOOL_KIND {
            return Err(ez4a_session::Error::message(format!(
                "task {} is not a tool",
                tool.id
            )));
        }
        if tool.owner != Some(generation.task_id) {
            return Err(ez4a_session::Error::message(
                "tool task is not owned by this generation",
            ));
        }
        // The cutoff is the batch's last result: the next request must see
        // every result of the round, and entry ids only grow.
        entry_id = Some(tool_result_entry(tx, &tool)?.max(entry_id.unwrap_or(i64::MIN)));
    }
    let entry_id = entry_id
        .ok_or_else(|| ez4a_session::Error::message("tool round has no tool task"))?;
    let stored: GenerationInput = serde_json::from_value(task.input.clone())
        .map_err(|err| ez4a_session::Error::message(err.to_string()))?;
    let request = RequestCheckpoint {
        phase: "request".into(),
        attempt: 1,
        cutoff: entry_id,
        failed_attempts: task.restarts.unwrap_or(0),
        submission_id: stored.submission_id,
        model_calls,
    };
    task.state = TaskState::Running {
        checkpoint: serde_json::to_value(&request)?,
    };
    tx.set_task(task)?;
    Ok(())
}

fn stop_for_model_cap(tx: &mut Tx, task_id: i64) -> Result<(), ez4a_session::Error> {
    let mut task = load_task(tx, task_id)?;
    if matches!(task.state, TaskState::Terminal { .. }) {
        return Ok(());
    }
    task.state = TaskState::Terminal {
        outcome: TaskOutcome::Failed {
            error: TaskOutcomeError {
                message: format!("The generation made {MODEL_CALL_CAP} model calls"),
                detail: Some(json!({ "reason": "restart_limit" })),
            },
            result: None,
        },
    };
    tx.set_task(task)?;
    Ok(())
}

fn first_tool_id(checkpoint: &Json) -> Result<i64, ez4a_session::Error> {
    tool_ids(checkpoint)?
        .first()
        .copied()
        .ok_or_else(|| ez4a_session::Error::message("tool round has no tool task"))
}

/// Every tool task id of the round, in call order.
fn tool_ids(checkpoint: &Json) -> Result<Vec<i64>, ez4a_session::Error> {
    let ids = checkpoint
        .get("tools")
        .and_then(Json::as_array)
        .ok_or_else(|| ez4a_session::Error::message("tool round has no tool task"))?
        .iter()
        .filter_map(Json::as_i64)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err(ez4a_session::Error::message("tool round has no tool task"));
    }
    Ok(ids)
}

fn checkpoint_model_calls(checkpoint: &Json) -> i64 {
    checkpoint
        .get("modelCalls")
        .and_then(Json::as_i64)
        .unwrap_or(0)
}

fn tool_result_entry(tx: &mut Tx, tool: &TaskRecord) -> Result<i64, ez4a_session::Error> {
    if matches!(tool.state, TaskState::Waiting { .. }) {
        return Err(ez4a_session::Error::message(
            "tool task is still waiting",
        ));
    }
    let TaskState::Terminal { outcome } = &tool.state else {
        return Err(ez4a_session::Error::message(format!(
            "tool task {} is not terminal",
            tool.id
        )));
    };
    let result = match outcome {
        TaskOutcome::Completed { result } => result,
        TaskOutcome::Failed {
            result: Some(result),
            ..
        } => result,
        _ => {
            return Err(ez4a_session::Error::message(
                "tool task ended without a result entry",
            ));
        }
    };
    let entry_id = result.get("entryId").and_then(Json::as_i64).ok_or_else(|| {
        ez4a_session::Error::message("tool task result has no entry")
    })?;
    let entry = tx.entry(entry_id)?.ok_or_else(|| {
        ez4a_session::Error::message(format!("entry {entry_id} does not exist"))
    })?;
    if entry.kind != TOOL_RESULT_KIND || entry.conversation_id != tool.conversation_id {
        return Err(ez4a_session::Error::message(format!(
            "entry {entry_id} is not a tool result"
        )));
    }
    Ok(entry_id)
}

async fn collect_answer<L: Llm>(llm: &L, request: &TurnRequest) -> Result<Answer, Error> {
    let mut stream = llm.stream(request).await?;
    let events = drain(&mut stream).await?;
    let mut text = String::new();
    let mut tools = Vec::new();
    let mut usage = None;
    for event in events {
        match event {
            LlmEvent::Text(chunk) => text.push_str(&chunk),
            LlmEvent::ToolCall(call) => tools.push(call),
            LlmEvent::Usage(next) => usage = Some(next),
        }
    }
    if text.is_empty() && tools.is_empty() {
        return Err(Error::message("model turn was empty"));
    }
    let usage = usage.ok_or_else(|| Error::message("model turn did not report usage"))?;
    Ok(Answer { text, tools, usage })
}

async fn drain(stream: &mut EventStream) -> Result<Vec<LlmEvent>, Error> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    Ok(events)
}

/// Stored arguments follow the offered tool schema. No matching name leaves them unchanged.
fn coerced_arguments(task: &TaskRecord, call: &ToolCall) -> Json {
    let Ok(input) = serde_json::from_value::<GenerationInput>(task.input.clone()) else {
        return call.arguments.clone();
    };
    let Some(tool) = input.tools.iter().find(|tool| tool.name == call.name) else {
        return call.arguments.clone();
    };
    coerce(&tool.parameters, &call.arguments)
}

fn offered_parameters(task: &TaskRecord, name: &str) -> Option<Json> {
    let input = serde_json::from_value::<GenerationInput>(task.input.clone()).ok()?;
    input
        .tools
        .into_iter()
        .find(|tool| tool.name == name)
        .map(|tool| tool.parameters)
}

/// Coerced arguments after one tool round. The journal is not appended.
fn round_arguments(task: &TaskRecord, call: &ToolCall) -> Result<Json, ez4a_session::Error> {
    let arguments = coerced_arguments(task, call);
    let schema = offered_parameters(task, &call.name);
    let journal = run_tool_round(&call.id, &call.name, arguments.clone(), schema.as_ref())
        .map_err(ez4a_session::Error::message)?;
    if journal.get("toolCallId").and_then(Json::as_str) != Some(call.id.as_str())
        || journal.get("name").and_then(Json::as_str) != Some(call.name.as_str())
    {
        return Err(ez4a_session::Error::message(
            "tool round does not match the stored call",
        ));
    }
    Ok(journal.get("arguments").cloned().unwrap_or(arguments))
}

/// A failed model call keeps its message when the stop string is `error`.
fn failed_model(err: Error) -> Error {
    match stop_reason(false, true) {
        "error" => err,
        other => Error::message(format!("{other}: {err}")),
    }
}

fn commit_answer(
    tx: &mut Tx,
    generation: &Generation,
    checkpoint: &RequestCheckpoint,
    answer: &Answer,
) -> Result<i64, ez4a_session::Error> {
    let task = load_task(tx, generation.task_id)?;
    record_usage(tx, generation.conversation_id, &answer.usage)?;
    let mut stored_arguments = Vec::with_capacity(answer.tools.len());
    for call in &answer.tools {
        stored_arguments.push(round_arguments(&task, call)?);
    }
    let mut entry_id = None;
    let mut tool_started = false;
    for step in plan(!answer.tools.is_empty()) {
        match step {
            PlanStep::Prepare | PlanStep::Request => {}
            PlanStep::AppendAssistant => {
                entry_id = Some(append_assistant(
                    tx,
                    generation,
                    answer,
                    &stored_arguments,
                )?);
            }
            PlanStep::AppendTool => {
                let id = entry_id.ok_or_else(|| {
                    ez4a_session::Error::message("tool round has no assistant entry")
                })?;
                if answer.tools.is_empty() {
                    return Err(ez4a_session::Error::message(
                        "tool round has no tool call",
                    ));
                }
                start_tool_tasks(tx, generation, &task, checkpoint, &answer.tools, id)?;
                tool_started = true;
            }
            PlanStep::Settle => {
                if tool_started {
                    continue;
                }
                let id = entry_id.ok_or_else(|| {
                    ez4a_session::Error::message("generation completed without an entry")
                })?;
                settle_from(tx, task.clone(), checkpoint, id)?;
            }
        }
    }
    entry_id.ok_or_else(|| ez4a_session::Error::message("generation completed without an entry"))
}

fn append_assistant(
    tx: &mut Tx,
    generation: &Generation,
    answer: &Answer,
    arguments: &[Json],
) -> Result<i64, ez4a_session::Error> {
    let mut content = Vec::new();
    if !answer.text.is_empty() {
        content.push(json!({ "type": "text", "text": answer.text }));
    }
    for (call, arguments) in answer.tools.iter().zip(arguments) {
        content.push(json!({
            "type": "toolCall",
            "id": call.id,
            "name": call.name,
            "arguments": arguments,
        }));
    }
    let stop = stop_reason(!answer.tools.is_empty(), false);
    let entry = tx.append_entry(
        generation.conversation_id,
        EntryDraft {
            kind: ASSISTANT_KIND.into(),
            model: Some(json!([{
                "role": "assistant",
                "content": content,
                "provider": answer.usage.provider,
                "model": answer.usage.model,
                "usage": {
                    "input": answer.usage.input_tokens,
                    "output": answer.usage.output_tokens,
                    "cost": 0
                },
                "stopReason": stop,
            }])),
            data: None,
            head: None,
            edits: None,
        },
    )?;
    Ok(entry.id)
}

/// One `pi.tool` task per call of the round. The generation waits on all of
/// them at once (`tools` lists every id), so a model that calls tools in
/// parallel has the whole batch run before its next request.
fn start_tool_tasks(
    tx: &mut Tx,
    generation: &Generation,
    task: &TaskRecord,
    checkpoint: &RequestCheckpoint,
    calls: &[ToolCall],
    entry_id: i64,
) -> Result<(), ez4a_session::Error> {
    let mut tool_ids = Vec::with_capacity(calls.len());
    for call in calls {
        let tool = tx.create_task(TaskCreate {
            kind: TOOL_KIND.into(),
            version: 3,
            conversation_id: generation.conversation_id,
            input: json!({
                "kind": "model",
                "assistant": entry_id,
                "callId": call.id,
                "name": call.name,
            }),
            owner: Some(generation.task_id),
            background: false,
            checkpoint: json!({ "phase": "call" }),
        })?;
        let mut waiting = tool.clone();
        waiting.state = TaskState::Waiting {
            checkpoint: json!({ "phase": "call" }),
            on: Vec::new(),
            policy: JoinPolicy::AllSettled,
        };
        tx.set_task(waiting)?;
        tool_ids.push(json!(tool.id));
    }
    let mut parent = task.clone();
    let model_calls = checkpoint.model_calls.saturating_add(1);
    let on: Vec<i64> = tool_ids.iter().filter_map(Json::as_i64).collect();
    parent.state = TaskState::Waiting {
        checkpoint: json!({
            "phase": "tools",
            "assistant": entry_id,
            "tools": tool_ids,
            "pending": [],
            "modelCalls": model_calls,
        }),
        on,
        policy: JoinPolicy::AllSettled,
    };
    tx.set_task(parent)?;
    Ok(())
}

fn settle_answered(
    tx: &mut Tx,
    task_id: i64,
    checkpoint: &RequestCheckpoint,
    entry_id: i64,
) -> Result<(), ez4a_session::Error> {
    let task = load_task(tx, task_id)?;
    settle_from(tx, task, checkpoint, entry_id)
}

fn settle_from(
    tx: &mut Tx,
    mut task: TaskRecord,
    checkpoint: &RequestCheckpoint,
    entry_id: i64,
) -> Result<(), ez4a_session::Error> {
    tx.settle_submission(
        checkpoint.submission_id,
        SubmissionSettlement::Done { answer: entry_id },
    )?;
    task.state = TaskState::Terminal {
        outcome: TaskOutcome::Completed {
            result: json!({ "entryId": entry_id }),
        },
    };
    tx.set_task(task)?;
    Ok(())
}

fn scheduler_task(record: &TaskRecord) -> crate::scheduler_js::Task {
    crate::scheduler_js::Task {
        id: record.id,
        kind: record.kind.clone(),
        state: scheduler_state(&record.state).to_owned(),
        version: record.version,
        abort_requested: record.abort_requested,
        abort_reason: match record.abort_reason {
            Some(AbortReason::Restart) => "restart".to_owned(),
            None => String::new(),
        },
        attempt: 0,
    }
}

fn scheduler_state(state: &TaskState) -> &'static str {
    match state {
        TaskState::Pending { .. } => "idle",
        TaskState::Running { .. } => "running",
        TaskState::Waiting { .. } | TaskState::Sleeping { .. } => "waiting",
        TaskState::Completing { .. } | TaskState::Terminal { .. } => "done",
    }
}

async fn start_ready(
    session: &Session,
    rows: &[TaskRecord],
    ready: Option<i64>,
) -> Result<(), Error> {
    let Some(ready_id) = ready else {
        return Ok(());
    };
    let Some(row) = rows.iter().find(|task| task.id == ready_id) else {
        return Err(Error::message(format!("task {ready_id} does not exist")));
    };
    let mut running = row.clone();
    let TaskState::Pending { checkpoint } = running.state else {
        return Ok(());
    };
    running.state = TaskState::Running { checkpoint };
    session
        .commit(move |tx| {
            tx.set_task(running)?;
            Ok(())
        })
        .await?;
    Ok(())
}

async fn tool_task_id(session: &Session, generation_id: i64) -> Result<i64, Error> {
    let task = session
        .task(generation_id)
        .await?
        .ok_or_else(|| Error::message(format!("task {generation_id} does not exist")))?;
    let TaskState::Waiting { checkpoint, .. } = task.state else {
        return Err(Error::message(format!(
            "generation {generation_id} is not waiting on a tool"
        )));
    };
    checkpoint
        .get("tools")
        .and_then(Json::as_array)
        .and_then(|tools| tools.first())
        .and_then(Json::as_i64)
        .ok_or_else(|| Error::message("tool round has no tool task"))
}
