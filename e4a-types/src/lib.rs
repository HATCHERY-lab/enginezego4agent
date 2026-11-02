// Portions derived from pi-durable 1.5.0 (packages/durable/src/types.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Durable storage records. IDs share one integer namespace. Conversation 1 is the root.

mod apply;

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

pub use apply::{apply, ApplyError, Op, Path, Seg};

pub type Json = serde_json::Value;

/// Reserved root conversation id. [`crate`] minting starts at 2.
pub const ROOT_CONVERSATION_ID: i64 = 1;

/// `Number.MAX_SAFE_INTEGER`. Minting stops once `next_id` leaves this range.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScanOrder {
    Ascending,
    Descending,
}

impl std::fmt::Display for ScanOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanOrder::Ascending => f.write_str("ascending"),
            ScanOrder::Descending => f.write_str("descending"),
        }
    }
}

/// Opaque scan continuation. Round-trip as JSON; storage reads `after` and `order`.
pub type Cursor = Json;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Cursor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitResult {
    pub stored: usize,
    pub refused: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationParent {
    pub conversation_id: i64,
    pub at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationOwner {
    pub conversation_id: i64,
    pub task_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationRecord {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<ConversationParent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ConversationOwner>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextEditAction {
    Omit,
    Replace,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextEdit {
    pub target: i64,
    pub action: ContextEditAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Json>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRecord {
    pub id: i64,
    pub conversation_id: i64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edits: Option<Vec<ContextEdit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by_task_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmissionType {
    Input,
    Write,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmissionStatus {
    Queued,
    Placed,
    Done,
    Unanswered,
}

impl std::fmt::Display for SubmissionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            SubmissionStatus::Queued => "queued",
            SubmissionStatus::Placed => "placed",
            SubmissionStatus::Done => "done",
            SubmissionStatus::Unanswered => "unanswered",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmissionRecord {
    pub id: i64,
    pub conversation_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Json>,
    #[serde(rename = "type")]
    pub ty: SubmissionType,
    pub status: SubmissionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Json>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskOutcomeError {
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Json>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TaskOutcome {
    Completed {
        result: Json,
    },
    Failed {
        error: TaskOutcomeError,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Json>,
    },
    Aborted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Json>,
    },
    Orphaned {
        reason: String,
    },
    Faulted {
        error: TaskOutcomeError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JoinPolicy {
    FailFast,
    AllSettled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Pending,
    Running,
    Waiting,
    Sleeping,
    Completing,
    Terminal,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Waiting => "waiting",
            TaskStatus::Sleeping => "sleeping",
            TaskStatus::Completing => "completing",
            TaskStatus::Terminal => "terminal",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum TaskState {
    Pending {
        checkpoint: Json,
    },
    Running {
        checkpoint: Json,
    },
    Waiting {
        checkpoint: Json,
        on: Vec<i64>,
        policy: JoinPolicy,
    },
    Sleeping {
        checkpoint: Json,
        until: i64,
    },
    Completing {
        outcome: TaskOutcome,
    },
    Terminal {
        outcome: TaskOutcome,
    },
}

impl TaskState {
    pub fn status(&self) -> TaskStatus {
        match self {
            TaskState::Pending { .. } => TaskStatus::Pending,
            TaskState::Running { .. } => TaskStatus::Running,
            TaskState::Waiting { .. } => TaskStatus::Waiting,
            TaskState::Sleeping { .. } => TaskStatus::Sleeping,
            TaskState::Completing { .. } => TaskStatus::Completing,
            TaskState::Terminal { .. } => TaskStatus::Terminal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AbortReason {
    Restart,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<Json>>,
    pub id: i64,
    pub conversation_id: i64,
    pub kind: String,
    pub version: i64,
    pub input: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<i64>,
    pub background: bool,
    pub abort_requested: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abort_reason: Option<AbortReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abandon_on_restart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restarts: Option<i64>,
    pub state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memos: Option<Json>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum DocumentScope {
    Session,
    Conversation { conversation_id: i64 },
    Task { task_id: i64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentHistory {
    Latest,
    Rewindable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentFork {
    AsOf,
    Current,
    Initial,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRecord {
    pub id: i64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<i64>,
    pub scope: DocumentScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<DocumentHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<DocumentFork>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCreate {
    pub id: i64,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub scope: DocumentScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<DocumentHistory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<DocumentFork>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentPoint {
    Current,
    Seq(i64),
}

impl Serialize for DocumentPoint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            DocumentPoint::Current => serializer.serialize_str("current"),
            DocumentPoint::Seq(seq) => serializer.serialize_i64(*seq),
        }
    }
}

impl<'de> Deserialize<'de> for DocumentPoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Json::deserialize(deserializer)?;
        match value {
            Json::String(text) if text == "current" => Ok(DocumentPoint::Current),
            Json::Number(number) => number
                .as_i64()
                .map(DocumentPoint::Seq)
                .ok_or_else(|| de::Error::custom("invalid document point")),
            _ => Err(de::Error::custom("invalid document point")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DocumentContent {
    Base { version: i64, value: Json },
    Delta { version: i64, ops: Vec<Op> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DocumentCopySource {
    pub id: i64,
    pub at: DocumentPoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredDocument {
    pub record: DocumentRecord,
    pub version: i64,
    pub value: Json,
    pub deltas_since_base: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StorageWrite {
    #[serde(rename = "conversation")]
    Conversation { value: ConversationRecord },
    #[serde(rename = "entry")]
    Entry { value: EntryRecord },
    #[serde(rename = "task")]
    Task { value: TaskRecord },
    #[serde(rename = "submission")]
    Submission { value: SubmissionRecord },
    #[serde(rename = "document.create")]
    DocumentCreate {
        record: DocumentCreate,
        content: DocumentContent,
    },
    #[serde(rename = "document.copy")]
    DocumentCopy {
        record: DocumentCreate,
        source: DocumentCopySource,
    },
    #[serde(rename = "document.change")]
    DocumentChange { id: i64, content: DocumentContent },
    #[serde(rename = "document.retire")]
    DocumentRetire { id: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntryHit {
    pub entry: EntryRecord,
    pub commit_seq: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversationQuery {
    pub owner_conversation_id: Option<i64>,
    pub owner_task_id: Option<i64>,
    pub order: Option<ScanOrder>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryQuery {
    pub conversation_id: i64,
    pub min_entry_id: Option<i64>,
    pub max_entry_id: Option<i64>,
    pub order: Option<ScanOrder>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskQuery {
    pub conversation_id: Option<i64>,
    pub kind: Option<String>,
    pub status: Option<TaskStatus>,
    pub abort_requested: Option<bool>,
    pub background: Option<bool>,
    pub order: Option<ScanOrder>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubmissionQuery {
    pub conversation_id: Option<i64>,
    pub status: Option<SubmissionStatus>,
    pub order: Option<ScanOrder>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentAddress {
    pub kind: String,
    pub scope: DocumentScope,
    pub key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentQuery {
    pub scope: DocumentScope,
    pub at: DocumentPoint,
    pub kind: Option<String>,
}

pub fn is_safe_integer(value: i64) -> bool {
    (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&value)
}
