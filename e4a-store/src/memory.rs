// Portions derived from pi-durable 1.5.0 (packages/durable/src/storage/memory.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Detached memory backend. Receipts run after every accepted batch is stored.

use std::cell::{Cell, RefCell};

use e4a_types::{
    CommitResult, ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentPoint,
    DocumentQuery, DocumentRecord, EntryHit, EntryQuery, EntryRecord, Page, StoredDocument,
    SubmissionQuery, SubmissionRecord, TaskQuery, TaskRecord,
};

use crate::engine::Engine;
use crate::{Error, Storage, StorageWrite};

pub struct MemoryStorage {
    engine: RefCell<Engine>,
    closed: Cell<bool>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            engine: RefCell::new(Engine::new()),
            closed: Cell::new(false),
        }
    }

    fn ensure(&self) -> Result<(), Error> {
        if self.closed.get() {
            Err(Error::message("MemoryStorage is closed"))
        } else {
            Ok(())
        }
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl Storage for MemoryStorage {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error> {
        self.ensure()?;
        let (seqs, refused) = self.engine.borrow_mut().apply_batches(batches)?;
        for (index, seq) in seqs.iter().enumerate() {
            receive(index, *seq);
        }
        Ok(CommitResult {
            stored: seqs.len(),
            refused,
        })
    }

    fn mint_id(&self) -> Result<i64, Error> {
        self.ensure()?;
        self.engine.borrow_mut().mint_id()
    }

    fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().conversation(id)
    }

    fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().scan_conversations(
            query.owner_conversation_id,
            query.owner_task_id,
            query.order,
            limit,
            cursor,
        )
    }

    fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error> {
        self.ensure()?;
        self.engine.borrow().entry(id)
    }

    fn entry_in(&self, conversation_id: i64, id: i64) -> Result<Option<EntryHit>, Error> {
        self.ensure()?;
        self.engine.borrow().entry_in(conversation_id, id)
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error> {
        self.ensure()?;
        self.engine
            .borrow()
            .find_latest_head_marker(conversation_id, at_or_before)
    }

    fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().scan_entries(query, limit, cursor)
    }

    fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().task(id)
    }

    fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().scan_tasks(query, limit, cursor)
    }

    fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().submission(id)
    }

    fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().scan_submissions(query, limit, cursor)
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        self.ensure()?;
        self.engine
            .borrow()
            .submission_by_request(conversation_id, request_id)
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<DocumentRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().find_document(address, at)
    }

    fn document(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error> {
        self.ensure()?;
        self.engine.borrow().document(id, at)
    }

    fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<DocumentRecord>, Error> {
        self.ensure()?;
        self.engine.borrow().scan_documents(query, limit, cursor)
    }

    fn close(&self) -> Result<(), Error> {
        self.closed.set(true);
        Ok(())
    }
}
