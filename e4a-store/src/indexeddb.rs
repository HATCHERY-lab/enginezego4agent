// Portions derived from pi-durable 1.5.2 (packages/durable/src/storage/indexeddb.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 storage/indexeddb, MIT, see LICENSES/pi-MIT.txt.

//! In-memory object stores with the pi-durable IndexedDB layout.
//!
//! No browser and no disk. [`open`] keeps the database name and the records
//! in the process. [`IdbStorage`] is the session backend over those stores.
//! Keys match when their JSON text matches.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use e4a_types::{
    CommitResult, ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope, EntryHit, EntryQuery, EntryRecord,
    Page, StoredDocument, SubmissionQuery, SubmissionRecord, TaskQuery, TaskRecord,
};
use serde_json::Value;

use crate::engine::{BatchWrite, DocumentWrite, Engine, Revision};
use crate::{Error, Storage, StorageWrite};

const META_KEY: &str = "state";

/// One `createIndex` call: name plus key path, in path order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectIndex {
    pub name: String,
    pub key_path: Vec<String>,
}

/// One `createObjectStore` call.
///
/// `key_path` is the store key path. A single property is one element; a
/// compound path such as `["documentId", "seq"]` keeps every property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStore {
    pub name: String,
    pub key_path: Vec<String>,
    pub indexes: Vec<ObjectIndex>,
}

/// Object stores created by the version-1 upgrade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub stores: Vec<ObjectStore>,
}

/// Store names, key paths, and indexes from `createSchema`.
pub fn schema() -> Schema {
    Schema {
        stores: vec![
            object_store("meta", &["key"], vec![]),
            object_store("record_ids", &["id"], vec![]),
            object_store(
                "conversations",
                &["id"],
                vec![
                    index(
                        "conversations_by_owner_conversation",
                        &["ownerConversationId", "id"],
                    ),
                    index("conversations_by_owner_task", &["ownerTaskId", "id"]),
                ],
            ),
            object_store(
                "entries",
                &["id"],
                vec![
                    index("entries_by_conversation", &["conversationId", "id"]),
                    index("entry_heads_by_conversation", &["headConversationId", "id"]),
                ],
            ),
            object_store(
                "tasks",
                &["id"],
                vec![
                    index("tasks_by_status", &["status", "id"]),
                    index("tasks_by_conversation", &["conversationId", "id"]),
                    index("tasks_by_kind", &["kind", "id"]),
                    index("tasks_by_abort_requested", &["abortRequested", "id"]),
                    index("tasks_by_background", &["background", "id"]),
                ],
            ),
            object_store(
                "submissions",
                &["id"],
                vec![
                    index("submissions_by_request", &["conversationId", "requestId"]),
                    index("submissions_by_conversation", &["conversationId", "id"]),
                    index("submissions_by_status", &["status", "id"]),
                ],
            ),
            object_store(
                "documents",
                &["id"],
                vec![
                    index(
                        "documents_by_address",
                        &[
                            "kind",
                            "scopeKind",
                            "ownerId",
                            "family",
                            "keyValue",
                            "createdAt",
                            "id",
                        ],
                    ),
                    index("documents_by_scope", &["scopeKind", "ownerId", "id"]),
                    index(
                        "documents_by_scope_kind",
                        &["scopeKind", "ownerId", "kind", "id"],
                    ),
                ],
            ),
            object_store(
                "document_revisions",
                &["documentId", "seq"],
                vec![index(
                    "document_revisions_by_kind",
                    &["documentId", "kind", "seq"],
                )],
            ),
        ],
    }
}

fn object_store(name: &str, key_path: &[&str], indexes: Vec<ObjectIndex>) -> ObjectStore {
    ObjectStore {
        name: name.to_owned(),
        key_path: owned_path(key_path),
        indexes,
    }
}

fn index(name: &str, key_path: &[&str]) -> ObjectIndex {
    ObjectIndex {
        name: name.to_owned(),
        key_path: owned_path(key_path),
    }
}

fn owned_path(key_path: &[&str]) -> Vec<String> {
    key_path.iter().map(|part| (*part).to_owned()).collect()
}

#[derive(Clone, Debug)]
struct StoreBody {
    indexes: Vec<ObjectIndex>,
    rows: BTreeMap<String, Value>,
}

/// Process-local database. The name is kept and never used as a path.
#[derive(Clone, Debug)]
pub struct MemoryIdb {
    name: String,
    stores: BTreeMap<String, StoreBody>,
}

/// Open an empty in-memory database.
///
/// An empty `name` is rejected. The upgrade seeds `meta` at `"state"` with
/// `nextId: 2` and `nextSeq: 1`, the same row `createSchema` inserts.
pub fn open(name: &str) -> Result<MemoryIdb, String> {
    if name.is_empty() {
        return Err("database name is empty".to_owned());
    }
    let mut stores = BTreeMap::new();
    for object_store in schema().stores {
        stores.insert(
            object_store.name.clone(),
            StoreBody {
                indexes: object_store.indexes,
                rows: BTreeMap::new(),
            },
        );
    }
    let mut database = MemoryIdb {
        name: name.to_owned(),
        stores,
    };
    database.put(
        "meta",
        &Value::String(META_KEY.to_owned()),
        &serde_json::json!({
            "key": META_KEY,
            "nextId": 2,
            "nextSeq": 1,
        }),
    )?;
    Ok(database)
}

impl MemoryIdb {
    /// Database name passed to [`open`].
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Insert or replace `value` at `key`.
    ///
    /// `key` is stored by its JSON text. A later key with the same text
    /// overwrites the row.
    pub fn put(&mut self, store: &str, key: &Value, value: &Value) -> Result<(), String> {
        let text = json_text(key)?;
        let body = self.store_mut(store)?;
        body.rows.insert(text, value.clone());
        Ok(())
    }

    /// Record stored at `key`, compared by JSON text.
    pub fn get(&self, store: &str, key: &Value) -> Result<Option<Value>, String> {
        let text = json_text(key)?;
        let body = self.store(store)?;
        Ok(body.rows.get(&text).cloned())
    }

    /// Remove the row at `key`, if one is stored. A missing key is success.
    pub fn delete(&mut self, store: &str, key: &Value) -> Result<(), String> {
        let text = json_text(key)?;
        let body = self.store_mut(store)?;
        body.rows.remove(&text);
        Ok(())
    }

    /// Records whose index key has the same JSON text as `value`.
    ///
    /// A one-property index key is that property. A compound index key is a
    /// JSON array of the key-path values, in path order. A missing property
    /// leaves the record out of the index. Rows come back in primary-key
    /// JSON text order.
    pub fn get_all_by_index(
        &self,
        store: &str,
        index: &str,
        value: &Value,
    ) -> Result<Vec<Value>, String> {
        let body = self.store(store)?;
        let path = body
            .indexes
            .iter()
            .find(|candidate| candidate.name == index)
            .map(|candidate| &candidate.key_path)
            .ok_or_else(|| format!("unknown index: {index}"))?;
        let wanted = json_text(value)?;
        let mut rows = Vec::new();
        for row in body.rows.values() {
            let Some(projected) = index_key(row, path) else {
                continue;
            };
            if json_text(&projected)? == wanted {
                rows.push(row.clone());
            }
        }
        Ok(rows)
    }

    fn values(&self, store: &str) -> Result<Vec<Value>, String> {
        Ok(self.store(store)?.rows.values().cloned().collect())
    }

    fn store(&self, name: &str) -> Result<&StoreBody, String> {
        self.stores
            .get(name)
            .ok_or_else(|| format!("unknown object store: {name}"))
    }

    fn store_mut(&mut self, name: &str) -> Result<&mut StoreBody, String> {
        self.stores
            .get_mut(name)
            .ok_or_else(|| format!("unknown object store: {name}"))
    }
}

fn json_text(value: &Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| error.to_string())
}

/// Index key for `path`. `None` when any property is absent.
///
/// One property yields that JSON value. Several yield a JSON array.
fn index_key(value: &Value, path: &[String]) -> Option<Value> {
    let Value::Object(object) = value else {
        return None;
    };
    let mut parts = Vec::with_capacity(path.len());
    for field in path {
        parts.push(object.get(field)?.clone());
    }
    match parts.len() {
        0 => None,
        1 => parts.into_iter().next(),
        _ => Some(Value::Array(parts)),
    }
}

/// Process-local IndexedDB session store.
///
/// [`IdbStorage::open`] takes the database name. There is no browser, disk, or
/// network. Reads use the durable engine. Accepted batches land in the object
/// stores before the commit returns. `RefCell` makes this `!Sync`; one writer
/// thread owns the handle.
pub struct IdbStorage {
    engine: RefCell<Engine>,
    database: RefCell<MemoryIdb>,
    closed: Cell<bool>,
}

impl IdbStorage {
    /// Open an empty database named `name`.
    ///
    /// An empty name is rejected. The seeded `meta` row matches [`open`].
    pub fn open(name: &str) -> Result<Self, Error> {
        let database = open(name).map_err(Error::message)?;
        let engine = load_engine(&database)?;
        Ok(Self {
            engine: RefCell::new(engine),
            database: RefCell::new(database),
            closed: Cell::new(false),
        })
    }

    fn ensure(&self) -> Result<(), Error> {
        if self.closed.get() {
            Err(Error::message("IdbStorage is closed"))
        } else {
            Ok(())
        }
    }

    fn read<T>(&self, read: impl FnOnce(&Engine) -> Result<T, Error>) -> Result<T, Error> {
        self.ensure()?;
        read(&self.engine.borrow())
    }
}

impl Storage for IdbStorage {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error> {
        self.ensure()?;
        let applied = {
            let mut engine = self.engine.borrow_mut();
            engine.apply_batches(batches)?
        };
        if !applied.seqs.is_empty() {
            let next_id = self.engine.borrow().next_id();
            let next_seq = self.engine.borrow().next_seq();
            let persist_result = {
                let mut database = self.database.borrow_mut();
                persist(&mut database, &applied.writes, next_id, next_seq)
            };
            if let Err(error) = persist_result {
                let loaded = {
                    let database = self.database.borrow();
                    load_engine(&database)?
                };
                *self.engine.borrow_mut() = loaded;
                return Err(error);
            }
        }
        for (index, seq) in applied.seqs.iter().enumerate() {
            receive(index, *seq);
        }
        Ok(CommitResult {
            stored: applied.seqs.len(),
            refused: applied.refused,
        })
    }

    fn mint_id(&self) -> Result<i64, Error> {
        self.ensure()?;
        self.engine.borrow_mut().mint_id()
    }

    fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        self.read(|engine| engine.conversation(id))
    }

    fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error> {
        self.read(|engine| {
            engine.scan_conversations(
                query.owner_conversation_id,
                query.owner_task_id,
                query.order,
                limit,
                cursor,
            )
        })
    }

    fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error> {
        self.read(|engine| engine.entry(id))
    }

    fn entry_in(&self, conversation_id: i64, id: i64) -> Result<Option<EntryHit>, Error> {
        self.read(|engine| engine.entry_in(conversation_id, id))
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error> {
        self.read(|engine| engine.find_latest_head_marker(conversation_id, at_or_before))
    }

    fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error> {
        self.read(|engine| engine.scan_entries(query, limit, cursor))
    }

    fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.read(|engine| engine.task(id))
    }

    fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error> {
        self.read(|engine| engine.scan_tasks(query, limit, cursor))
    }

    fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        self.read(|engine| engine.submission(id))
    }

    fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error> {
        self.read(|engine| engine.scan_submissions(query, limit, cursor))
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        self.read(|engine| engine.submission_by_request(conversation_id, request_id))
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<DocumentRecord>, Error> {
        self.read(|engine| engine.find_document(address, at))
    }

    fn document(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error> {
        self.read(|engine| engine.document(id, at))
    }

    fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<DocumentRecord>, Error> {
        self.read(|engine| engine.scan_documents(query, limit, cursor))
    }

    fn delete_conversation(&self, id: i64) -> Result<(), Error> {
        self.ensure()?;
        self.engine.borrow().ensure_deletable(id)?;
        {
            let mut database = self.database.borrow_mut();
            delete_conversation_rows(&mut database, id)?;
        }
        self.engine.borrow_mut().delete_conversation(id)
    }

    fn close(&self) -> Result<(), Error> {
        self.closed.set(true);
        Ok(())
    }
}

fn load_engine(database: &MemoryIdb) -> Result<Engine, Error> {
    let meta = database
        .get("meta", &Value::String(META_KEY.to_owned()))
        .map_err(Error::message)?
        .ok_or_else(|| Error::message("IndexedDB meta state is missing"))?;
    let next_id = meta
        .get("nextId")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::message("IndexedDB meta nextId is invalid"))?;
    let next_seq = meta
        .get("nextSeq")
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::message("IndexedDB meta nextSeq is invalid"))?;
    let mut engine = Engine::new();
    for row in sorted_rows(database, "conversations")? {
        engine.load_conversation(serde_json::from_value(record_value(&row)?)?);
    }
    for row in sorted_rows(database, "entries")? {
        let commit_seq = require_i64(&row, "commitSeq")?;
        engine.load_entry(serde_json::from_value(record_value(&row)?)?, commit_seq);
    }
    for row in sorted_rows(database, "tasks")? {
        engine.load_task(serde_json::from_value(record_value(&row)?)?);
    }
    for row in sorted_rows(database, "submissions")? {
        engine.load_submission(serde_json::from_value(record_value(&row)?)?);
    }
    let revisions = database
        .values("document_revisions")
        .map_err(Error::message)?;
    for row in sorted_rows(database, "documents")? {
        let record: DocumentRecord = serde_json::from_value(record_value(&row)?)?;
        let mut document_revisions = Vec::new();
        for revision in &revisions {
            if revision.get("documentId").and_then(Value::as_i64) == Some(record.id) {
                document_revisions.push(decode_revision(revision)?);
            }
        }
        document_revisions.sort_by_key(|revision| revision.seq);
        engine.load_document(record, document_revisions);
    }
    engine.finish_load(next_id, next_seq);
    Ok(engine)
}

fn sorted_rows(database: &MemoryIdb, store: &str) -> Result<Vec<Value>, Error> {
    let mut rows = database.values(store).map_err(Error::message)?;
    rows.sort_by_key(|row| row.get("id").and_then(Value::as_i64).unwrap_or(i64::MIN));
    Ok(rows)
}

fn record_value(row: &Value) -> Result<Value, Error> {
    row.get("record")
        .cloned()
        .ok_or_else(|| Error::message("IndexedDB row is missing record"))
}

fn require_i64(row: &Value, field: &str) -> Result<i64, Error> {
    row.get(field)
        .and_then(Value::as_i64)
        .ok_or_else(|| Error::message(format!("IndexedDB row is missing {field}")))
}

fn decode_revision(row: &Value) -> Result<Revision, Error> {
    let seq = require_i64(row, "seq")?;
    let version = require_i64(row, "version")?;
    let kind = row
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::message("IndexedDB revision kind is missing"))?;
    let content_value = row
        .get("content")
        .cloned()
        .ok_or_else(|| Error::message("IndexedDB revision content is missing"))?;
    let content = match kind {
        "base" => DocumentContent::Base {
            version,
            value: content_value,
        },
        "delta" => DocumentContent::Delta {
            version,
            ops: serde_json::from_value(content_value)?,
        },
        other => {
            return Err(Error::message(format!(
                "unknown document revision kind {other}"
            )))
        }
    };
    Ok(Revision { seq, content })
}

/// Apply `writes` on a copy. The caller's database changes only after every row is stored.
fn persist(
    database: &mut MemoryIdb,
    writes: &[BatchWrite],
    next_id: i64,
    next_seq: i64,
) -> Result<(), Error> {
    let mut updated = database.clone();
    for batch in writes {
        write_batch(&mut updated, batch)?;
    }
    store_meta(&mut updated, next_id, next_seq)?;
    *database = updated;
    Ok(())
}

fn write_batch(database: &mut MemoryIdb, batch: &BatchWrite) -> Result<(), Error> {
    for record in &batch.conversations {
        claim_id(database, record.id, "conversation")?;
        let row = conversation_row(record)?;
        database
            .put("conversations", &serde_json::json!(record.id), &row)
            .map_err(Error::message)?;
    }
    for record in &batch.entries {
        claim_id(database, record.id, "entry")?;
        let row = entry_row(record, batch.seq)?;
        database
            .put("entries", &serde_json::json!(record.id), &row)
            .map_err(Error::message)?;
    }
    for record in &batch.tasks {
        claim_id(database, record.id, "task")?;
        let row = task_row(record)?;
        database
            .put("tasks", &serde_json::json!(record.id), &row)
            .map_err(Error::message)?;
    }
    for record in &batch.submissions {
        claim_id(database, record.id, "submission")?;
        let row = submission_row(record)?;
        database
            .put("submissions", &serde_json::json!(record.id), &row)
            .map_err(Error::message)?;
    }
    for document in &batch.documents {
        write_document(database, document)?;
    }
    Ok(())
}

fn write_document(database: &mut MemoryIdb, document: &DocumentWrite) -> Result<(), Error> {
    if document.inserted {
        claim_id(database, document.record.id, "document")?;
    }
    let row = document_row(&document.record)?;
    database
        .put(
            "documents",
            &serde_json::json!(document.record.id),
            &row,
        )
        .map_err(Error::message)?;
    if document.replace_revisions {
        delete_revisions(database, document.record.id)?;
    }
    for revision in &document.revisions {
        let row = revision_row(document.record.id, revision)?;
        database
            .put(
                "document_revisions",
                &serde_json::json!([document.record.id, revision.seq]),
                &row,
            )
            .map_err(Error::message)?;
    }
    Ok(())
}

fn conversation_row(record: &ConversationRecord) -> Result<Value, Error> {
    let mut row = serde_json::Map::new();
    row.insert("id".to_owned(), serde_json::json!(record.id));
    if let Some(owner) = record.owner {
        row.insert(
            "ownerConversationId".to_owned(),
            serde_json::json!(owner.conversation_id),
        );
        row.insert("ownerTaskId".to_owned(), serde_json::json!(owner.task_id));
    }
    row.insert("record".to_owned(), serde_json::to_value(record)?);
    Ok(Value::Object(row))
}

fn entry_row(record: &EntryRecord, commit_seq: i64) -> Result<Value, Error> {
    let mut row = serde_json::Map::new();
    row.insert("id".to_owned(), serde_json::json!(record.id));
    row.insert(
        "conversationId".to_owned(),
        serde_json::json!(record.conversation_id),
    );
    if record.head.is_some() {
        row.insert(
            "headConversationId".to_owned(),
            serde_json::json!(record.conversation_id),
        );
    }
    row.insert("commitSeq".to_owned(), serde_json::json!(commit_seq));
    row.insert("record".to_owned(), serde_json::to_value(record)?);
    Ok(Value::Object(row))
}

fn task_row(record: &TaskRecord) -> Result<Value, Error> {
    let mut row = serde_json::Map::new();
    row.insert("id".to_owned(), serde_json::json!(record.id));
    row.insert(
        "conversationId".to_owned(),
        serde_json::json!(record.conversation_id),
    );
    row.insert("kind".to_owned(), serde_json::json!(record.kind));
    row.insert(
        "status".to_owned(),
        serde_json::json!(record.state.status().to_string()),
    );
    row.insert(
        "abortRequested".to_owned(),
        serde_json::json!(record.abort_requested),
    );
    row.insert("background".to_owned(), serde_json::json!(record.background));
    row.insert("record".to_owned(), serde_json::to_value(record)?);
    Ok(Value::Object(row))
}

fn submission_row(record: &SubmissionRecord) -> Result<Value, Error> {
    let mut row = serde_json::Map::new();
    row.insert("id".to_owned(), serde_json::json!(record.id));
    row.insert(
        "conversationId".to_owned(),
        serde_json::json!(record.conversation_id),
    );
    if let Some(request_id) = &record.request_id {
        row.insert("requestId".to_owned(), serde_json::json!(request_id));
    }
    row.insert("status".to_owned(), serde_json::json!(record.status.to_string()));
    row.insert("record".to_owned(), serde_json::to_value(record)?);
    Ok(Value::Object(row))
}

fn document_row(record: &DocumentRecord) -> Result<Value, Error> {
    let (scope_kind, owner_id) = scope_columns(&record.scope);
    let mut row = serde_json::Map::new();
    row.insert("id".to_owned(), serde_json::json!(record.id));
    row.insert("kind".to_owned(), serde_json::json!(record.kind));
    row.insert("scopeKind".to_owned(), serde_json::json!(scope_kind));
    row.insert("ownerId".to_owned(), serde_json::json!(owner_id));
    row.insert(
        "family".to_owned(),
        serde_json::json!(i64::from(record.key.is_some())),
    );
    row.insert(
        "keyValue".to_owned(),
        serde_json::json!(record.key.as_deref().unwrap_or("")),
    );
    row.insert("createdAt".to_owned(), serde_json::json!(record.created_at));
    if let Some(retired_at) = record.retired_at {
        row.insert("retiredAt".to_owned(), serde_json::json!(retired_at));
    }
    row.insert("record".to_owned(), serde_json::to_value(record)?);
    Ok(Value::Object(row))
}

fn revision_row(document_id: i64, revision: &Revision) -> Result<Value, Error> {
    let (kind, version, content) = match &revision.content {
        DocumentContent::Base { version, value } => ("base", *version, value.clone()),
        DocumentContent::Delta { version, ops } => ("delta", *version, serde_json::to_value(ops)?),
    };
    Ok(serde_json::json!({
        "documentId": document_id,
        "seq": revision.seq,
        "kind": kind,
        "version": version,
        "content": content,
    }))
}

fn scope_columns(scope: &DocumentScope) -> (&'static str, i64) {
    match scope {
        DocumentScope::Session => ("session", 0),
        DocumentScope::Conversation { conversation_id } => ("conversation", *conversation_id),
        DocumentScope::Task { task_id } => ("task", *task_id),
    }
}

fn claim_id(database: &mut MemoryIdb, id: i64, kind: &str) -> Result<(), Error> {
    let key = serde_json::json!(id);
    if database
        .get("record_ids", &key)
        .map_err(Error::message)?
        .is_some()
    {
        return Ok(());
    }
    database
        .put(
            "record_ids",
            &key,
            &serde_json::json!({ "id": id, "recordType": kind }),
        )
        .map_err(Error::message)
}

fn store_meta(database: &mut MemoryIdb, next_id: i64, next_seq: i64) -> Result<(), Error> {
    database
        .put(
            "meta",
            &Value::String(META_KEY.to_owned()),
            &serde_json::json!({
                "key": META_KEY,
                "nextId": next_id,
                "nextSeq": next_seq,
            }),
        )
        .map_err(Error::message)
}

fn delete_revisions(database: &mut MemoryIdb, document_id: i64) -> Result<(), Error> {
    let rows = database
        .values("document_revisions")
        .map_err(Error::message)?;
    for row in rows {
        if row.get("documentId").and_then(Value::as_i64) != Some(document_id) {
            continue;
        }
        let seq = require_i64(&row, "seq")?;
        database
            .delete("document_revisions", &serde_json::json!([document_id, seq]))
            .map_err(Error::message)?;
    }
    Ok(())
}

/// Delete one conversation's rows. Children that point at `id` stay.
fn delete_conversation_rows(database: &mut MemoryIdb, id: i64) -> Result<(), Error> {
    if database
        .get("conversations", &serde_json::json!(id))
        .map_err(Error::message)?
        .is_none()
    {
        return Err(Error::message(format!("Unknown conversation: {id}")));
    }
    let tasks = database.values("tasks").map_err(Error::message)?;
    let mut task_ids = Vec::new();
    for row in &tasks {
        if row.get("conversationId").and_then(Value::as_i64) == Some(id) {
            task_ids.push(require_i64(row, "id")?);
        }
    }
    let documents = database.values("documents").map_err(Error::message)?;
    let mut document_ids = Vec::new();
    for row in &documents {
        let scope = row.get("scopeKind").and_then(Value::as_str).unwrap_or("");
        let owner = row.get("ownerId").and_then(Value::as_i64);
        let owned = match (scope, owner) {
            ("conversation", Some(owner)) if owner == id => true,
            ("task", Some(owner)) if task_ids.contains(&owner) => true,
            _ => false,
        };
        if owned {
            document_ids.push(require_i64(row, "id")?);
        }
    }
    for document_id in &document_ids {
        delete_revisions(database, *document_id)?;
        drop_record(database, "documents", *document_id)?;
    }
    drop_matching(database, "entries", id)?;
    drop_matching(database, "tasks", id)?;
    drop_matching(database, "submissions", id)?;
    drop_record(database, "conversations", id)
}

fn drop_matching(database: &mut MemoryIdb, store: &str, conversation_id: i64) -> Result<(), Error> {
    let rows = database.values(store).map_err(Error::message)?;
    for row in rows {
        if row.get("conversationId").and_then(Value::as_i64) != Some(conversation_id) {
            continue;
        }
        drop_record(database, store, require_i64(&row, "id")?)?;
    }
    Ok(())
}

fn drop_record(database: &mut MemoryIdb, store: &str, id: i64) -> Result<(), Error> {
    let key = serde_json::json!(id);
    database.delete(store, &key).map_err(Error::message)?;
    database
        .delete("record_ids", &key)
        .map_err(Error::message)
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    #[test]
    fn schema_copies_the_upgrade_path() {
        let schema = schema();
        let names: Vec<&str> = schema.stores.iter().map(|store| store.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "meta",
                "record_ids",
                "conversations",
                "entries",
                "tasks",
                "submissions",
                "documents",
                "document_revisions",
            ]
        );
        assert_eq!(schema.stores[0].key_path, ["key"]);
        assert!(schema.stores[0].indexes.is_empty());
        assert_eq!(schema.stores[1].key_path, ["id"]);
        assert_eq!(
            index_names(&schema.stores[2]),
            [
                "conversations_by_owner_conversation",
                "conversations_by_owner_task",
            ]
        );
        assert_eq!(
            schema.stores[2].indexes[0].key_path,
            ["ownerConversationId", "id"]
        );
        assert_eq!(
            schema.stores[2].indexes[1].key_path,
            ["ownerTaskId", "id"]
        );
        assert_eq!(
            index_names(&schema.stores[3]),
            ["entries_by_conversation", "entry_heads_by_conversation"]
        );
        assert_eq!(
            index_names(&schema.stores[4]),
            [
                "tasks_by_status",
                "tasks_by_conversation",
                "tasks_by_kind",
                "tasks_by_abort_requested",
                "tasks_by_background",
            ]
        );
        assert_eq!(
            index_names(&schema.stores[5]),
            [
                "submissions_by_request",
                "submissions_by_conversation",
                "submissions_by_status",
            ]
        );
        assert_eq!(schema.stores[5].indexes[0].key_path, ["conversationId", "requestId"]);
        assert_eq!(
            index_names(&schema.stores[6]),
            [
                "documents_by_address",
                "documents_by_scope",
                "documents_by_scope_kind",
            ]
        );
        assert_eq!(
            schema.stores[6].indexes[0].key_path,
            [
                "kind",
                "scopeKind",
                "ownerId",
                "family",
                "keyValue",
                "createdAt",
                "id",
            ]
        );
        assert_eq!(schema.stores[7].key_path, ["documentId", "seq"]);
        assert_eq!(
            schema.stores[7].indexes[0].key_path,
            ["documentId", "kind", "seq"]
        );
        assert_eq!(schema, super::schema());
    }

    fn index_names(store: &ObjectStore) -> Vec<&str> {
        store.indexes.iter().map(|index| index.name.as_str()).collect()
    }

    #[test]
    fn open_rejects_an_empty_name_and_keeps_any_other() {
        assert_eq!(open("").unwrap_err(), "database name is empty");
        let database = open("pi:durable").unwrap();
        assert_eq!(database.name(), "pi:durable");
        let meta = database.get("meta", &Value::String(META_KEY.to_owned())).unwrap();
        assert_eq!(
            meta,
            Some(serde_json::json!({"key": "state", "nextId": 2, "nextSeq": 1}))
        );
    }

    #[test]
    fn put_get_delete_compare_keys_by_json_text() {
        let mut database = open("keys").unwrap();
        let mut first = Map::new();
        first.insert("b".to_owned(), serde_json::json!(1));
        first.insert("a".to_owned(), serde_json::json!(2));
        let key = Value::Object(first);
        database
            .put("record_ids", &key, &serde_json::json!({"id": key, "tag": "row"}))
            .unwrap();
        let mut second = Map::new();
        second.insert("a".to_owned(), serde_json::json!(2));
        second.insert("b".to_owned(), serde_json::json!(1));
        let found = database.get("record_ids", &Value::Object(second)).unwrap();
        assert_eq!(found.unwrap()["tag"], "row");

        database
            .put("record_ids", &serde_json::json!(1), &serde_json::json!({"tag": "n"}))
            .unwrap();
        database
            .put(
                "record_ids",
                &serde_json::json!("1"),
                &serde_json::json!({"tag": "s"}),
            )
            .unwrap();
        assert_eq!(
            database.get("record_ids", &serde_json::json!(1)).unwrap().unwrap()["tag"],
            "n"
        );
        assert_eq!(
            database
                .get("record_ids", &serde_json::json!("1"))
                .unwrap()
                .unwrap()["tag"],
            "s"
        );

        database.delete("record_ids", &serde_json::json!(1)).unwrap();
        assert_eq!(database.get("record_ids", &serde_json::json!(1)).unwrap(), None);
        database.delete("record_ids", &serde_json::json!(1)).unwrap();
        assert_eq!(
            database
                .put("missing", &serde_json::json!(1), &serde_json::json!(null))
                .unwrap_err(),
            "unknown object store: missing"
        );
    }

    #[test]
    fn index_reads_project_the_key_path() {
        let mut database = open("idx").unwrap();
        database
            .put(
                "submissions",
                &serde_json::json!(1),
                &serde_json::json!({"id": 1, "conversationId": 5, "requestId": "r"}),
            )
            .unwrap();
        database
            .put(
                "submissions",
                &serde_json::json!(2),
                &serde_json::json!({"id": 2, "conversationId": 5, "requestId": "r"}),
            )
            .unwrap();
        database
            .put(
                "submissions",
                &serde_json::json!(3),
                &serde_json::json!({"id": 3, "conversationId": 5, "requestId": "other"}),
            )
            .unwrap();
        database
            .put(
                "submissions",
                &serde_json::json!(4),
                &serde_json::json!({"id": 4, "conversationId": 5}),
            )
            .unwrap();
        let rows = database
            .get_all_by_index(
                "submissions",
                "submissions_by_request",
                &serde_json::json!([5, "r"]),
            )
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], 1);
        assert_eq!(rows[1]["id"], 2);

        database
            .put(
                "tasks",
                &serde_json::json!(1),
                &serde_json::json!({"id": 1, "status": "open"}),
            )
            .unwrap();
        database
            .put(
                "tasks",
                &serde_json::json!(1),
                &serde_json::json!({"id": 1, "status": "done"}),
            )
            .unwrap();
        assert!(database
            .get_all_by_index("tasks", "tasks_by_status", &serde_json::json!(["open", 1]))
            .unwrap()
            .is_empty());
        assert_eq!(
            database
                .get_all_by_index("tasks", "tasks_by_status", &serde_json::json!(["done", 1]))
                .unwrap()
                .len(),
            1
        );

        let revision_key = serde_json::json!([4, 9]);
        database
            .put(
                "document_revisions",
                &revision_key,
                &serde_json::json!({"documentId": 4, "seq": 9, "kind": "base"}),
            )
            .unwrap();
        let revisions = database
            .get_all_by_index(
                "document_revisions",
                "document_revisions_by_kind",
                &serde_json::json!([4, "base", 9]),
            )
            .unwrap();
        assert_eq!(revisions.len(), 1);
        assert_eq!(
            database.get("document_revisions", &revision_key).unwrap().unwrap()["kind"],
            "base"
        );

        assert_eq!(
            database
                .get_all_by_index("tasks", "submissions_by_status", &serde_json::json!(["done", 1]))
                .unwrap_err(),
            "unknown index: submissions_by_status"
        );
        assert_eq!(
            database
                .get_all_by_index("nope", "tasks_by_status", &serde_json::json!(["done", 1]))
                .unwrap_err(),
            "unknown object store: nope"
        );
        database.delete("tasks", &serde_json::json!(1)).unwrap();
        assert!(database
            .get_all_by_index("tasks", "tasks_by_status", &serde_json::json!(["done", 1]))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn commit_reads_the_conversation_back() {
        use crate::{ConversationRecord, Storage, StorageWrite, ROOT_CONVERSATION_ID};

        let storage = IdbStorage::open("read-own-write").unwrap();
        let written = ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        };
        assert_eq!(storage.conversation(written.id).unwrap(), None);
        let committed = storage
            .commit(
                &[vec![StorageWrite::Conversation {
                    value: written.clone(),
                }]],
                &mut |_index, _seq| {},
            )
            .unwrap();
        assert_eq!(committed.stored, 1);
        assert_eq!(committed.refused, None);
        assert_eq!(storage.conversation(written.id).unwrap(), Some(written));
    }
}
