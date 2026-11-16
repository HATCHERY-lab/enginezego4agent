// Portions derived from pi-durable 1.5.2 (packages/durable/src/session), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Session kernel: one mutation line, group-commit writer thread, publication of committed state.

mod backend;
mod cache;
mod docs;
mod forks;
mod keys;
mod live;
mod live_doc;
mod lookup;
mod session;
mod session_api;
mod store;
mod tails;
mod tx;
mod tx_ops;
mod view;

pub use backend::{describe, parse_kind, StoreKind};
pub use cache::{admits, evict_oldest, Cache};
pub use docs::{
    agent_doc, compaction_fields, live_doc, provider_doc, address_id, Checkpoint, CompactionSettings,
    DocDef, DocRef, DocScopeKind, AGENT_KIND, LIVE_KIND, PROVIDER_KIND,
};
pub use live::{
    cut_off_count, cut_off_too_often, salvageable, RunFailureReason, LIMIT_RESTARTS,
};
pub use live_doc::live_document;
pub use lookup::task_conversation_id;
pub use tails::{advance_tail, mint_key, parse_key, tail_entry_id};
pub use session::{Session, SessionEnd};
pub use session_api::{fork_parent, role_of, usage_delta, visible_entry};
pub use tx::{
    ConversationOwnership, EntryDraft, SubmissionCreate, SubmissionSettlement, TaskCreate, Tx,
};
pub use tx_ops::{append_entry, apply_order, note_usage, set_task_state, TxOp};
pub use view::{ConversationView, ConversationWindow};

pub use ez4a_types::{
    apply, ConversationRecord, DocumentFork, DocumentHistory, DocumentScope, EntryRecord, Json, Op,
    Path, Seg, SubmissionRecord, SubmissionStatus, SubmissionType, TaskRecord, TaskState,
    ROOT_CONVERSATION_ID,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<ez4a_store::Error> for Error {
    fn from(error: ez4a_store::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<ez4a_types::ApplyError> for Error {
    fn from(error: ez4a_types::ApplyError) -> Self {
        Self(error.0)
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}
