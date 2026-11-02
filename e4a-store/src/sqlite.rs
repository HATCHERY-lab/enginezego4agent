// Portions derived from pi-durable 1.5.0 (packages/durable/src/storage/sqlite), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! SQLite backend. Schema version 2 is created directly. Rows are the store: reopen loads them.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::time::Duration;

use e4a_types::{
    CommitResult, ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope, EntryHit, EntryQuery, EntryRecord,
    Page, StoredDocument, SubmissionQuery, SubmissionRecord, TaskQuery, TaskRecord,
};
use rusqlite::{params, Connection, OptionalExtension};

use crate::engine::{Engine, Revision};
use crate::{Error, Storage, StorageWrite};

const SCHEMA_VERSION: i64 = 2;

const SCHEMA_SQL: &str = "
CREATE TABLE durable_schema (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    version INTEGER NOT NULL CHECK (version >= 0)
) STRICT;
INSERT INTO durable_schema (singleton, version) VALUES (1, 2);
CREATE TABLE durable_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    next_id TEXT NOT NULL,
    next_seq INTEGER NOT NULL
) STRICT;
INSERT INTO durable_metadata (singleton, next_id, next_seq) VALUES (1, '2', 1);
CREATE TABLE record_ids (
    id INTEGER PRIMARY KEY,
    record_type TEXT NOT NULL CHECK (record_type IN ('conversation', 'entry', 'task', 'submission', 'document'))
) STRICT;
CREATE TABLE conversations (
    id INTEGER PRIMARY KEY,
    owner_conversation_id INTEGER,
    owner_task_id INTEGER,
    record TEXT NOT NULL CHECK (json_valid(record))
) STRICT;
CREATE INDEX conversations_by_owner_conversation ON conversations (owner_conversation_id, id);
CREATE INDEX conversations_by_owner_task ON conversations (owner_task_id, id);
CREATE TABLE entries (
    id INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL,
    head INTEGER,
    commit_seq INTEGER NOT NULL,
    record TEXT NOT NULL CHECK (json_valid(record))
) STRICT;
CREATE INDEX entries_by_conversation ON entries (conversation_id, id DESC);
CREATE INDEX entry_heads_by_conversation ON entries (conversation_id, id DESC) WHERE head IS NOT NULL;
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'waiting', 'sleeping', 'completing', 'terminal')),
    abort_requested INTEGER NOT NULL CHECK (abort_requested IN (0, 1)),
    background INTEGER NOT NULL CHECK (background IN (0, 1)),
    record TEXT NOT NULL CHECK (json_valid(record))
) STRICT;
CREATE INDEX tasks_by_status ON tasks (status, id);
CREATE INDEX tasks_by_conversation ON tasks (conversation_id, id);
CREATE INDEX tasks_by_kind ON tasks (kind, id);
CREATE INDEX tasks_by_abort_requested ON tasks (abort_requested, id);
CREATE INDEX tasks_by_background ON tasks (background, id);
CREATE TABLE submissions (
    id INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL,
    request_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('queued', 'placed', 'done', 'unanswered')),
    record TEXT NOT NULL CHECK (json_valid(record))
) STRICT;
CREATE INDEX submissions_by_request ON submissions (conversation_id, request_id);
CREATE INDEX submissions_by_conversation ON submissions (conversation_id, id);
CREATE INDEX submissions_by_status ON submissions (status, id);
CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    family INTEGER NOT NULL CHECK (family IN (0, 1)),
    key_value TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('session', 'conversation', 'task')),
    owner_id INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    retired_at INTEGER,
    record TEXT NOT NULL CHECK (json_valid(record))
) STRICT;
CREATE INDEX documents_by_address
    ON documents (kind, scope_kind, owner_id, family, key_value, created_at DESC, retired_at);
CREATE INDEX documents_by_scope ON documents (scope_kind, owner_id, id);
CREATE INDEX documents_by_scope_kind ON documents (scope_kind, owner_id, kind, id);
CREATE TABLE document_revisions (
    document_id INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('base', 'delta')),
    version INTEGER NOT NULL,
    content TEXT NOT NULL CHECK (json_valid(content)),
    PRIMARY KEY (document_id, seq)
) STRICT;
CREATE INDEX document_revisions_by_kind ON document_revisions (document_id, kind, seq DESC);
";

pub struct SqliteStorage {
    engine: RefCell<Engine>,
    conn: RefCell<Option<Connection>>,
    closed: Cell<bool>,
}

impl SqliteStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        ensure_schema(&conn)?;
        let engine = load_engine(&conn)?;
        Ok(Self {
            engine: RefCell::new(engine),
            conn: RefCell::new(Some(conn)),
            closed: Cell::new(false),
        })
    }

    fn ensure(&self) -> Result<(), Error> {
        if self.closed.get() {
            Err(Error::message("SqliteStorage is closed"))
        } else {
            Ok(())
        }
    }
}

impl Storage for SqliteStorage {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error> {
        self.ensure()?;
        let (seqs, refused) = {
            let mut engine = self.engine.borrow_mut();
            let conn_guard = self.conn.borrow();
            let conn = conn_guard
                .as_ref()
                .ok_or_else(|| Error::message("SqliteStorage is closed"))?;
            let (seqs, refused) = engine.apply_batches(batches)?;
            if !seqs.is_empty() {
                if let Err(error) = persist(conn, &engine) {
                    *engine = load_engine(conn)?;
                    return Err(error);
                }
                *engine = load_engine(conn)?;
            }
            (seqs, refused)
        };
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
        if self.closed.get() {
            return Ok(());
        }
        self.closed.set(true);
        if let Some(conn) = self.conn.borrow_mut().take() {
            let _ = conn.query_row("PRAGMA wal_checkpoint(FULL)", [], |row| {
                row.get::<_, i64>(0)
            });
            drop(conn);
        }
        Ok(())
    }
}

fn ensure_schema(conn: &Connection) -> Result<(), Error> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'durable_schema'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        conn.execute_batch(SCHEMA_SQL)?;
        return Ok(());
    }
    let version: Option<i64> = conn
        .query_row(
            "SELECT version FROM durable_schema WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    match version {
        Some(SCHEMA_VERSION) => Ok(()),
        Some(version) => Err(Error::message(format!(
            "SQLite schema version {version} is not supported version {SCHEMA_VERSION}"
        ))),
        None => Err(Error::message("SQLite schema metadata is missing")),
    }
}

fn load_engine(conn: &Connection) -> Result<Engine, Error> {
    let (next_id_text, next_seq): (String, i64) = conn.query_row(
        "SELECT next_id, next_seq FROM durable_metadata WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let next_id: i64 = next_id_text
        .parse()
        .map_err(|_| Error::message("Durable SQLite metadata next_id is invalid"))?;
    let mut engine = Engine::new();
    {
        let mut stmt = conn.prepare("SELECT record FROM conversations ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let record: ConversationRecord = serde_json::from_str(&row?)?;
            engine.load_conversation(record);
        }
    }
    {
        let mut stmt = conn.prepare("SELECT record, commit_seq FROM entries ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (record_json, commit_seq) = row?;
            let record: EntryRecord = serde_json::from_str(&record_json)?;
            engine.load_entry(record, commit_seq);
        }
    }
    {
        let mut stmt = conn.prepare("SELECT record FROM tasks ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let record: TaskRecord = serde_json::from_str(&row?)?;
            engine.load_task(record);
        }
    }
    {
        let mut stmt = conn.prepare("SELECT record FROM submissions ORDER BY id")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let record: SubmissionRecord = serde_json::from_str(&row?)?;
            engine.load_submission(record);
        }
    }
    let documents: Vec<(i64, String)> = {
        let mut stmt = conn.prepare("SELECT id, record FROM documents ORDER BY id")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (id, record_json) in documents {
        let record: DocumentRecord = serde_json::from_str(&record_json)?;
        let revisions = {
            let mut stmt = conn.prepare(
                "SELECT seq, kind, version, content FROM document_revisions WHERE document_id = ?1 ORDER BY seq",
            )?;
            let rows = stmt.query_map([id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut revisions = Vec::new();
            for row in rows {
                let (seq, kind, version, content) = row?;
                let content = match kind.as_str() {
                    "base" => DocumentContent::Base {
                        version,
                        value: serde_json::from_str(&content)?,
                    },
                    "delta" => DocumentContent::Delta {
                        version,
                        ops: serde_json::from_str(&content)?,
                    },
                    other => {
                        return Err(Error::message(format!(
                            "unknown document revision kind {other}"
                        )))
                    }
                };
                revisions.push(Revision { seq, content });
            }
            revisions
        };
        engine.load_document(record, revisions);
    }
    engine.finish_load(next_id, next_seq);
    Ok(engine)
}

fn persist(conn: &Connection, engine: &Engine) -> Result<(), Error> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DELETE FROM document_revisions;
         DELETE FROM documents;
         DELETE FROM submissions;
         DELETE FROM tasks;
         DELETE FROM entries;
         DELETE FROM conversations;
         DELETE FROM record_ids;",
    )?;
    for (id, kind) in engine.record_types() {
        tx.execute(
            "INSERT INTO record_ids (id, record_type) VALUES (?1, ?2)",
            params![id, kind.as_str()],
        )?;
    }
    for record in engine.conversations().values() {
        let (owner_conversation, owner_task) = match record.owner {
            Some(owner) => (Some(owner.conversation_id), Some(owner.task_id)),
            None => (None, None),
        };
        tx.execute(
            "INSERT INTO conversations (id, owner_conversation_id, owner_task_id, record) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.id,
                owner_conversation,
                owner_task,
                serde_json::to_string(record)?
            ],
        )?;
    }
    for record in engine.entries().values() {
        let commit_seq = engine.entry_commit_seqs()[&record.id];
        tx.execute(
            "INSERT INTO entries (id, conversation_id, head, commit_seq, record) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.conversation_id,
                record.head,
                commit_seq,
                serde_json::to_string(record)?
            ],
        )?;
    }
    for record in engine.tasks().values() {
        tx.execute(
            "INSERT INTO tasks (id, conversation_id, kind, status, abort_requested, background, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                record.id,
                record.conversation_id,
                encode_indexed(&record.kind)?,
                record.state.status().to_string(),
                i64::from(record.abort_requested),
                i64::from(record.background),
                serde_json::to_string(record)?
            ],
        )?;
    }
    for record in engine.submissions().values() {
        let request_id = record
            .request_id
            .as_deref()
            .map(encode_indexed)
            .transpose()?;
        tx.execute(
            "INSERT INTO submissions (id, conversation_id, request_id, status, record) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                record.id,
                record.conversation_id,
                request_id,
                record.status.to_string(),
                serde_json::to_string(record)?
            ],
        )?;
    }
    for stored in engine.documents().values() {
        let record = &stored.record;
        let (scope_kind, owner_id) = scope_columns(&record.scope);
        let family = i64::from(record.key.is_some());
        tx.execute(
            "INSERT INTO documents
                (id, kind, family, key_value, scope_kind, owner_id, created_at, retired_at, record)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.id,
                encode_indexed(&record.kind)?,
                family,
                encode_indexed(record.key.as_deref().unwrap_or(""))?,
                scope_kind,
                owner_id,
                record.created_at,
                record.retired_at,
                serde_json::to_string(record)?
            ],
        )?;
        for revision in &stored.revisions {
            let (kind, version, content) = encode_revision(&revision.content)?;
            tx.execute(
                "INSERT INTO document_revisions (document_id, seq, kind, version, content)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![record.id, revision.seq, kind, version, content],
            )?;
        }
    }
    tx.execute(
        "UPDATE durable_metadata SET next_id = ?1, next_seq = ?2 WHERE singleton = 1",
        params![engine.next_id().to_string(), engine.next_seq()],
    )?;
    tx.commit()?;
    Ok(())
}

fn encode_indexed(value: &str) -> Result<String, Error> {
    Ok(serde_json::to_string(value)?)
}

fn scope_columns(scope: &DocumentScope) -> (&'static str, i64) {
    match scope {
        DocumentScope::Session => ("session", 0),
        DocumentScope::Conversation { conversation_id } => ("conversation", *conversation_id),
        DocumentScope::Task { task_id } => ("task", *task_id),
    }
}

fn encode_revision(content: &DocumentContent) -> Result<(&'static str, i64, String), Error> {
    match content {
        DocumentContent::Base { version, value } => {
            Ok(("base", *version, serde_json::to_string(value)?))
        }
        DocumentContent::Delta { version, ops } => {
            Ok(("delta", *version, serde_json::to_string(ops)?))
        }
    }
}
