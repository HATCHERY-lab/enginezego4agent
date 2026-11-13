// Portions derived from pi-durable 1.5.2 (packages/durable/src/documents.ts, packages/durable/src/harness/agent.ts, packages/durable/src/harness/provider.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use e4a_types::{
    DocumentAddress, DocumentCreate, DocumentFork, DocumentHistory, DocumentScope, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Error;

pub const AGENT_KIND: &str = "pi.agent";
pub const PROVIDER_KIND: &str = "pi.provider";
pub const LIVE_KIND: &str = "pi.live";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocScopeKind {
    Session,
    Conversation,
    Task,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkpoint {
    Always,
    Never,
}

impl Checkpoint {
    pub fn when(self, _value: &Json, _ops: &[e4a_types::Op], _deltas_since_base: usize) -> bool {
        match self {
            Checkpoint::Always => true,
            Checkpoint::Never => false,
        }
    }
}

#[derive(Clone)]
pub struct DocDef {
    pub kind: String,
    pub version: i64,
    pub scope: DocScopeKind,
    pub history: Option<DocumentHistory>,
    pub fork: Option<DocumentFork>,
    pub checkpoint: Checkpoint,
    initial: Arc<dyn Fn() -> Json + Send + Sync>,
}

impl DocDef {
    pub fn conversation(
        kind: impl Into<String>,
        version: i64,
        history: DocumentHistory,
        fork: DocumentFork,
        initial: impl Fn() -> Json + Send + Sync + 'static,
        checkpoint: Checkpoint,
    ) -> Self {
        Self {
            kind: kind.into(),
            version,
            scope: DocScopeKind::Conversation,
            history: Some(history),
            fork: Some(fork),
            checkpoint,
            initial: Arc::new(initial),
        }
    }

    pub fn session(
        kind: impl Into<String>,
        version: i64,
        initial: impl Fn() -> Json + Send + Sync + 'static,
        checkpoint: Checkpoint,
    ) -> Self {
        Self {
            kind: kind.into(),
            version,
            scope: DocScopeKind::Session,
            history: None,
            fork: None,
            checkpoint,
            initial: Arc::new(initial),
        }
    }

    pub fn task(
        kind: impl Into<String>,
        version: i64,
        initial: impl Fn() -> Json + Send + Sync + 'static,
        checkpoint: Checkpoint,
    ) -> Self {
        Self {
            kind: kind.into(),
            version,
            scope: DocScopeKind::Task,
            history: None,
            fork: None,
            checkpoint,
            initial: Arc::new(initial),
        }
    }

    pub fn initial(&self) -> Json {
        (self.initial)()
    }

    pub fn address(&self, owner: Option<i64>) -> Result<DocumentAddress, Error> {
        let scope = match self.scope {
            DocScopeKind::Session => DocumentScope::Session,
            DocScopeKind::Conversation => {
                let conversation_id = owner.ok_or_else(|| {
                    Error::message(format!("Document {} requires a conversation ID", self.kind))
                })?;
                DocumentScope::Conversation { conversation_id }
            }
            DocScopeKind::Task => {
                let task_id = owner.ok_or_else(|| {
                    Error::message(format!("Document {} requires a task ID", self.kind))
                })?;
                DocumentScope::Task { task_id }
            }
        };
        Ok(DocumentAddress {
            kind: self.kind.clone(),
            scope,
            key: None,
        })
    }

    pub fn create_record(&self, id: i64, address: &DocumentAddress) -> DocumentCreate {
        DocumentCreate {
            id,
            kind: address.kind.clone(),
            key: address.key.clone(),
            scope: address.scope,
            history: self.history,
            fork: self.fork,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocRef {
    pub(crate) index: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_recent_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background_tokens: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_overrides: Option<BTreeMap<String, CompactionSettings>>,
}

pub fn compaction_fields(settings: &CompactionSettings) -> Result<Json, Error> {
    if let Some(tokens) = settings.reserve_tokens {
        require_non_negative("reserveTokens", tokens)?;
    }
    if let Some(tokens) = settings.keep_recent_tokens {
        require_non_negative("keepRecentTokens", tokens)?;
    }
    if let Some(tokens) = settings.background_tokens {
        require_non_negative("backgroundTokens", tokens)?;
    }
    Ok(serde_json::to_value(settings)?)
}

fn require_non_negative(field: &str, value: i64) -> Result<(), Error> {
    if value >= 0 {
        Ok(())
    } else {
        Err(Error::message(format!(
            "compaction.{field} must be an integer 0 or more, got {value}"
        )))
    }
}

pub fn agent_doc() -> DocDef {
    DocDef::conversation(
        AGENT_KIND,
        1,
        DocumentHistory::Rewindable,
        DocumentFork::AsOf,
        || json!({}),
        Checkpoint::Always,
    )
}

pub fn provider_doc() -> DocDef {
    DocDef::conversation(
        PROVIDER_KIND,
        1,
        DocumentHistory::Latest,
        DocumentFork::Initial,
        || json!({ "sessionId": new_provider_session_id() }),
        Checkpoint::Always,
    )
}

pub fn live_doc() -> DocDef {
    DocDef::conversation(
        LIVE_KIND,
        1,
        DocumentHistory::Rewindable,
        DocumentFork::AsOf,
        || json!({}),
        Checkpoint::Always,
    )
}

pub fn address_id(address: &DocumentAddress) -> String {
    let owner = match address.scope {
        DocumentScope::Session => Json::Null,
        DocumentScope::Conversation { conversation_id } => Json::from(conversation_id),
        DocumentScope::Task { task_id } => Json::from(task_id),
    };
    let scope_kind = match address.scope {
        DocumentScope::Session => "session",
        DocumentScope::Conversation { .. } => "conversation",
        DocumentScope::Task { .. } => "task",
    };
    let key = match &address.key {
        Some(key) => Json::String(key.clone()),
        None => Json::Null,
    };
    serde_json::to_string(&json!([address.kind, scope_kind, owner, key]))
        .expect("address id is JSON of primitives")
}

fn new_provider_session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{time:016x}-{n:016x}")
}
