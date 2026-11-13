// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/generation.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! One generation: commit the request, call the model outside the writer, commit the answer.
//! A `request` checkpoint with no assistant entry is a failed attempt. Five of those end the task.

use e4a_llm::{ChatMessage, EventStream, Llm, LlmEvent, ToolCall, ToolSchema, TurnRequest, Usage};
use e4a_session::{
    EntryDraft, EntryRecord, Session, SubmissionCreate, SubmissionSettlement, SubmissionStatus,
    SubmissionType, TaskCreate, TaskRecord, TaskState, Tx,
};
use e4a_types::{EntryQuery, JoinPolicy, Json, ScanOrder, TaskOutcome, TaskOutcomeError};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::usage::record_usage;
use crate::Error;

/// Same cap as pi-durable `DEFAULT_RESTART_LIMIT`. The next resume after it does not call the model.
pub const RESTART_CAP: i64 = 5;

const GENERATION_KIND: &str = "pi.generation";
const TOOL_KIND: &str = "pi.tool";
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
}

struct Answer {
    text: String,
    tool: Option<ToolCall>,
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

    async fn execute<L: Llm>(&self, session: &Session, llm: &L) -> Result<TurnOutcome, Error> {
        let live = session.commit(|tx| inspect(tx, self)).await?;
        match live {
            Live::Terminal(outcome) | Live::Waiting(outcome) => Ok(outcome),
            Live::Request(checkpoint) => {
                let request = session
                    .commit(|tx| turn_request(tx, self, &checkpoint))
                    .await?;
                let answer = collect_answer(llm, &request).await?;
                let entry_id = session
                    .commit(|tx| commit_answer(tx, self, &checkpoint, &answer))
                    .await?;
                let failed_attempts = session
                    .task(self.task_id)
                    .await?
                    .and_then(|task| task.restarts)
                    .unwrap_or(checkpoint.failed_attempts);
                let kind = if answer.tool.is_some() {
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
        })
    }

    fn limit_outcome(&self, failed_attempts: i64) -> TurnOutcome {
        TurnOutcome {
            task_id: self.task_id,
            failed_attempts,
            called_llm: false,
            kind: TurnKind::RestartLimit,
        }
    }
}

fn commit_intent(tx: &mut Tx, input: &TurnInput) -> Result<i64, e4a_session::Error> {
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
        })?,
    };
    tx.set_task(running)?;
    Ok(task.id)
}

fn inspect(tx: &mut Tx, generation: &Generation) -> Result<Live, e4a_session::Error> {
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
        _ => Err(e4a_session::Error::message(format!(
            "generation {} is {}",
            generation.task_id,
            task.state.status()
        ))),
    }
}

fn terminal_outcome(
    task: &TaskRecord,
    outcome: &TaskOutcome,
) -> Result<TurnOutcome, e4a_session::Error> {
    let failed_attempts = task.restarts.unwrap_or(0);
    let kind = match outcome {
        TaskOutcome::Completed { result } => {
            let entry_id = result
                .get("entryId")
                .and_then(Json::as_i64)
                .ok_or_else(|| {
                    e4a_session::Error::message(format!(
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
            return Err(e4a_session::Error::message(error.message.clone()));
        }
        _ => {
            return Err(e4a_session::Error::message(format!(
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
    })
}

fn waiting_outcome(
    task_id: i64,
    task: &TaskRecord,
    checkpoint: &Json,
) -> Result<TurnOutcome, e4a_session::Error> {
    if checkpoint.get("phase").and_then(Json::as_str) != Some("tools") {
        return Err(e4a_session::Error::message(format!(
            "generation {task_id} is waiting"
        )));
    }
    let entry_id = checkpoint
        .get("assistant")
        .and_then(Json::as_i64)
        .ok_or_else(|| e4a_session::Error::message("tool round has no assistant entry"))?;
    let tool_task_id = checkpoint
        .get("tools")
        .and_then(Json::as_array)
        .and_then(|tools| tools.first())
        .and_then(Json::as_i64)
        .ok_or_else(|| e4a_session::Error::message("tool round has no tool task"))?;
    Ok(TurnOutcome {
        task_id,
        failed_attempts: task.restarts.unwrap_or(0),
        called_llm: false,
        kind: TurnKind::WaitingTool {
            entry_id,
            tool_task_id,
        },
    })
}

fn parse_request(checkpoint: &Json) -> Result<RequestCheckpoint, e4a_session::Error> {
    if checkpoint.get("phase").and_then(Json::as_str) != Some("request") {
        return Err(e4a_session::Error::message(
            "generation is not in request",
        ));
    }
    Ok(serde_json::from_value(checkpoint.clone())?)
}

fn assistant_after(
    tx: &mut Tx,
    conversation_id: i64,
    cutoff: i64,
) -> Result<Option<i64>, e4a_session::Error> {
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
) -> Result<(), e4a_session::Error> {
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
) -> Result<TurnRequest, e4a_session::Error> {
    let task = load_task(tx, generation.task_id)?;
    let input: GenerationInput = serde_json::from_value(task.input)
        .map_err(|err| e4a_session::Error::message(err.to_string()))?;
    if input.submission_id != checkpoint.submission_id {
        return Err(e4a_session::Error::message(
            "generation input does not match the request checkpoint",
        ));
    }
    let entries = conversation_entries(tx, generation.conversation_id)?;
    let messages = entries
        .into_iter()
        .filter(|entry| entry.id <= checkpoint.cutoff)
        .filter_map(|entry| message_from_entry(&entry))
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Err(e4a_session::Error::message(
            "the conversation has nothing to answer",
        ));
    }
    Ok(TurnRequest {
        messages,
        tools: input.tools,
    })
}

fn load_task(tx: &mut Tx, task_id: i64) -> Result<TaskRecord, e4a_session::Error> {
    tx.task(task_id)?
        .ok_or_else(|| e4a_session::Error::message(format!("task {task_id} does not exist")))
}

fn conversation_entries(
    tx: &mut Tx,
    conversation_id: i64,
) -> Result<Vec<EntryRecord>, e4a_session::Error> {
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
    let model = entry.model.as_ref()?.as_array()?.first()?.clone();
    let role = model
        .get("role")
        .and_then(Json::as_str)
        .unwrap_or("user")
        .to_string();
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
        role,
        text,
        tool_calls,
    })
}

async fn collect_answer<L: Llm>(llm: &L, request: &TurnRequest) -> Result<Answer, Error> {
    let mut stream = llm.stream(request).await?;
    let events = drain(&mut stream).await?;
    let mut text = String::new();
    let mut tool = None;
    let mut usage = None;
    for event in events {
        match event {
            LlmEvent::Text(chunk) => text.push_str(&chunk),
            LlmEvent::ToolCall(call) => {
                if tool.is_some() {
                    return Err(Error::message(
                        "only one tool call is committed in this slice",
                    ));
                }
                tool = Some(call);
            }
            LlmEvent::Usage(next) => usage = Some(next),
        }
    }
    if text.is_empty() && tool.is_none() {
        return Err(Error::message("model turn was empty"));
    }
    let usage = usage.ok_or_else(|| Error::message("model turn did not report usage"))?;
    Ok(Answer { text, tool, usage })
}

async fn drain(stream: &mut EventStream) -> Result<Vec<LlmEvent>, Error> {
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event);
    }
    Ok(events)
}

fn commit_answer(
    tx: &mut Tx,
    generation: &Generation,
    checkpoint: &RequestCheckpoint,
    answer: &Answer,
) -> Result<i64, e4a_session::Error> {
    let task = load_task(tx, generation.task_id)?;
    record_usage(tx, generation.conversation_id, &answer.usage)?;
    let mut content = Vec::new();
    if !answer.text.is_empty() {
        content.push(json!({ "type": "text", "text": answer.text }));
    }
    if let Some(call) = &answer.tool {
        content.push(json!({
            "type": "toolCall",
            "id": call.id,
            "name": call.name,
            "arguments": call.arguments,
        }));
    }
    let stop_reason = if answer.tool.is_some() {
        "toolUse"
    } else {
        "stop"
    };
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
                "stopReason": stop_reason,
            }])),
            data: None,
            head: None,
            edits: None,
        },
    )?;
    if let Some(call) = &answer.tool {
        let tool = tx.create_task(TaskCreate {
            kind: TOOL_KIND.into(),
            version: 3,
            conversation_id: generation.conversation_id,
            input: json!({
                "kind": "model",
                "assistant": entry.id,
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
        let mut parent = task;
        parent.state = TaskState::Waiting {
            checkpoint: json!({
                "phase": "tools",
                "assistant": entry.id,
                "tools": [tool.id],
                "pending": [],
            }),
            on: vec![tool.id],
            policy: JoinPolicy::AllSettled,
        };
        tx.set_task(parent)?;
    } else {
        settle_from(tx, task, checkpoint, entry.id)?;
    }
    Ok(entry.id)
}

fn settle_answered(
    tx: &mut Tx,
    task_id: i64,
    checkpoint: &RequestCheckpoint,
    entry_id: i64,
) -> Result<(), e4a_session::Error> {
    let task = load_task(tx, task_id)?;
    settle_from(tx, task, checkpoint, entry_id)
}

fn settle_from(
    tx: &mut Tx,
    mut task: TaskRecord,
    checkpoint: &RequestCheckpoint,
    entry_id: i64,
) -> Result<(), e4a_session::Error> {
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
