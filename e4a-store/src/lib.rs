// Portions derived from pi-durable 1.5.0 (packages/durable/src/storage), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Synchronous `Storage`. Semantic refusals come back on [`CommitResult`]; every other failure is [`Error`].

mod engine;
mod memory;
mod sqlite;

pub use e4a_types::*;
pub use memory::MemoryStorage;
pub use sqlite::SqliteStorage;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self(error.to_string())
    }
}

pub trait Storage {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error>;

    fn mint_id(&self) -> Result<i64, Error>;

    fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error>;

    fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error>;

    fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error>;

    fn entry_in(&self, conversation_id: i64, id: i64) -> Result<Option<EntryHit>, Error>;

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error>;

    fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error>;

    fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error>;

    fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error>;

    fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error>;

    fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error>;

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error>;

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<DocumentRecord>, Error>;

    fn document(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error>;

    fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<DocumentRecord>, Error>;

    /// Permanently delete a conversation and its entries, tasks, submissions, and documents.
    ///
    /// [`ROOT_CONVERSATION_ID`] is refused: the root cannot be deleted. Children that
    /// point at `id` as parent stay; this does not recurse. A later [`Storage::conversation`]
    /// is `Ok(None)`.
    fn delete_conversation(&self, id: i64) -> Result<(), Error>;

    fn close(&self) -> Result<(), Error>;
}
