// Portions derived from pi-durable 1.5.0 (packages/durable/src/testing/storage-conformance.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Every pi-durable storage conformance case, on memory and on SQLite.
//! Lone UTF-16 surrogates are not Rust scalars; the lossless case uses distinct escaped scalars instead.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use ez4a_store::{
    CommitResult, ConversationOwner, ConversationParent, ConversationQuery, ConversationRecord,
    Cursor, DocumentAddress, DocumentContent, DocumentCopySource, DocumentCreate, DocumentFork,
    DocumentHistory, DocumentPoint, DocumentQuery, DocumentScope, EntryHit, EntryQuery,
    EntryRecord, Error, JoinPolicy, MemoryStorage, Op, Page, ScanOrder, Seg, SqliteStorage,
    Storage, StorageWrite, StoredDocument, SubmissionQuery, SubmissionRecord, SubmissionStatus,
    SubmissionType, TaskOutcome, TaskOutcomeError, TaskQuery, TaskRecord, TaskState, TaskStatus,
    MAX_SAFE_INTEGER, ROOT_CONVERSATION_ID,
};
use serde_json::{json, Value as Json};

fn conversation(id: i64) -> ConversationRecord {
    ConversationRecord {
        id,
        parent: None,
        owner: None,
    }
}

fn entry(id: i64, conversation_id: i64, kind: &str) -> EntryRecord {
    EntryRecord {
        id,
        conversation_id,
        kind: kind.to_string(),
        model: None,
        data: None,
        head: None,
        edits: None,
        by_task_id: None,
    }
}

fn with_data(mut record: EntryRecord, data: Json) -> EntryRecord {
    record.data = Some(data);
    record
}

fn with_head(mut record: EntryRecord, head: i64) -> EntryRecord {
    record.head = Some(head);
    record
}

fn pending(id: i64, conversation_id: i64) -> TaskRecord {
    TaskRecord {
        trace: None,
        links: None,
        id,
        conversation_id,
        kind: "test.task".into(),
        version: 1,
        input: json!({ "value": id }),
        owner: None,
        background: false,
        abort_requested: false,
        abort_reason: None,
        abandon_on_restart: None,
        started_at: None,
        ended_at: None,
        restarts: None,
        state: TaskState::Pending {
            checkpoint: json!({ "phase": "ready" }),
        },
        memos: None,
    }
}

fn submission(
    id: i64,
    conversation_id: i64,
    request_id: Option<&str>,
    ty: SubmissionType,
    status: SubmissionStatus,
) -> SubmissionRecord {
    SubmissionRecord {
        id,
        conversation_id,
        request_id: request_id.map(str::to_string),
        trace: None,
        ty,
        status,
        entry: None,
        answer: None,
        reason: None,
        detail: None,
    }
}

fn base(version: i64, value: Json) -> DocumentContent {
    DocumentContent::Base { version, value }
}

fn delta(version: i64, ops: Vec<Op>) -> DocumentContent {
    DocumentContent::Delta { version, ops }
}

fn key(name: &str) -> Seg {
    Seg::Key(name.to_string())
}

fn op_set(path: Vec<Seg>, value: Json) -> Op {
    Op::Set(path, value)
}

fn op_splice(path: Vec<Seg>, index: i64, remove: i64, items: Vec<Json>) -> Op {
    Op::Splice {
        path,
        index,
        remove,
        items,
    }
}

struct Checking<'a> {
    inner: &'a dyn Storage,
    seqs: RefCell<Vec<i64>>,
    settled: Cell<bool>,
}

impl<'a> Checking<'a> {
    fn new(inner: &'a dyn Storage) -> Self {
        Self {
            inner,
            seqs: RefCell::new(Vec::new()),
            settled: Cell::new(false),
        }
    }
}

struct Settle<'a>(&'a Cell<bool>);

impl Drop for Settle<'_> {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

impl Storage for Checking<'_> {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error> {
        self.seqs.borrow_mut().clear();
        self.settled.set(false);
        let _settle = Settle(&self.settled);
        let result = self.inner.commit(batches, &mut |index, seq| {
            assert!(
                !self.settled.get(),
                "Storage.commit() received a batch after it settled"
            );
            {
                let mut seqs = self.seqs.borrow_mut();
                assert_eq!(index, seqs.len());
                if let Some(previous) = seqs.last().copied() {
                    assert!(seq > previous);
                }
                seqs.push(seq);
            }
            receive(index, seq);
        })?;
        assert_eq!(self.seqs.borrow().len(), result.stored);
        Ok(result)
    }

    fn mint_id(&self) -> Result<i64, Error> {
        self.inner.mint_id()
    }

    fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        self.inner.conversation(id)
    }

    fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error> {
        self.inner.scan_conversations(query, limit, cursor)
    }

    fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error> {
        self.inner.entry(id)
    }

    fn entry_in(&self, conversation_id: i64, id: i64) -> Result<Option<EntryHit>, Error> {
        self.inner.entry_in(conversation_id, id)
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error> {
        self.inner
            .find_latest_head_marker(conversation_id, at_or_before)
    }

    fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error> {
        self.inner.scan_entries(query, limit, cursor)
    }

    fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.inner.task(id)
    }

    fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error> {
        self.inner.scan_tasks(query, limit, cursor)
    }

    fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        self.inner.submission(id)
    }

    fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error> {
        self.inner.scan_submissions(query, limit, cursor)
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        self.inner
            .submission_by_request(conversation_id, request_id)
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<ez4a_store::DocumentRecord>, Error> {
        self.inner.find_document(address, at)
    }

    fn document(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error> {
        self.inner.document(id, at)
    }

    fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ez4a_store::DocumentRecord>, Error> {
        self.inner.scan_documents(query, limit, cursor)
    }

    fn delete_conversation(&self, id: i64) -> Result<(), Error> {
        self.inner.delete_conversation(id)
    }

    fn close(&self) -> Result<(), Error> {
        self.inner.close()
    }
}

fn commit_writes(storage: &dyn Storage, writes: Vec<StorageWrite>) -> Result<i64, Error> {
    let batches = vec![writes];
    let mut seq = None;
    let result = storage.commit(&batches, &mut |_index, received| seq = Some(received))?;
    if result.stored == 0 {
        return Err(Error::message(
            result.refused.unwrap_or_else(|| "refused".into()),
        ));
    }
    Ok(seq.expect("stored batch"))
}

fn create_root(storage: &dyn Storage) -> i64 {
    commit_writes(
        storage,
        vec![StorageWrite::Conversation {
            value: conversation(ROOT_CONVERSATION_ID),
        }],
    )
    .unwrap();
    ROOT_CONVERSATION_ID
}

fn assert_err<T: std::fmt::Debug>(result: Result<T, Error>, needle: &str) {
    match result {
        Ok(value) => panic!("expected error containing {needle}, got {value:?}"),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains(needle),
                "error `{message}` does not contain `{needle}`"
            );
        }
    }
}

fn ids<T>(page: &Page<T>, id: impl Fn(&T) -> i64) -> Vec<i64> {
    page.items.iter().map(id).collect()
}

fn roundtrip(cursor: &Cursor) -> Cursor {
    serde_json::from_str(&serde_json::to_string(cursor).unwrap()).unwrap()
}

struct Bomb {
    case: &'static str,
    backend: &'static str,
}

impl Drop for Bomb {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("conformance case {} failed on {}", self.case, self.backend);
        }
    }
}

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ez4a-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(sibling(&self.path, "-wal"));
        let _ = std::fs::remove_file(sibling(&self.path, "-shm"));
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Closes and reopens the same file after every commit, so reads hit a cache loaded from disk.
struct Reopening {
    path: PathBuf,
    inner: RefCell<Option<Rc<SqliteStorage>>>,
    closed: Cell<bool>,
}

impl Reopening {
    fn open(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            inner: RefCell::new(Some(Rc::new(SqliteStorage::open(path).unwrap()))),
            closed: Cell::new(false),
        }
    }

    fn current(&self) -> Result<Rc<SqliteStorage>, Error> {
        if self.closed.get() {
            return Err(Error::message("SqliteStorage is closed"));
        }
        self.inner
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| Error::message("SqliteStorage is closed"))
    }

    fn with_live<T>(
        &self,
        body: impl FnOnce(&SqliteStorage) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let storage = self.current()?;
        body(&storage)
    }
}

impl Storage for Reopening {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error> {
        let storage = self.current()?;
        let result = storage.commit(batches, receive)?;
        storage.close()?;
        *self.inner.borrow_mut() = Some(Rc::new(SqliteStorage::open(&self.path)?));
        Ok(result)
    }

    fn mint_id(&self) -> Result<i64, Error> {
        self.with_live(|storage| storage.mint_id())
    }

    fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        self.with_live(|storage| storage.conversation(id))
    }

    fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error> {
        self.with_live(|storage| storage.scan_conversations(query, limit, cursor))
    }

    fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error> {
        self.with_live(|storage| storage.entry(id))
    }

    fn entry_in(&self, conversation_id: i64, id: i64) -> Result<Option<EntryHit>, Error> {
        self.with_live(|storage| storage.entry_in(conversation_id, id))
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error> {
        self.with_live(|storage| storage.find_latest_head_marker(conversation_id, at_or_before))
    }

    fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error> {
        self.with_live(|storage| storage.scan_entries(query, limit, cursor))
    }

    fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.with_live(|storage| storage.task(id))
    }

    fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error> {
        self.with_live(|storage| storage.scan_tasks(query, limit, cursor))
    }

    fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        self.with_live(|storage| storage.submission(id))
    }

    fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error> {
        self.with_live(|storage| storage.scan_submissions(query, limit, cursor))
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        self.with_live(|storage| storage.submission_by_request(conversation_id, request_id))
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<ez4a_store::DocumentRecord>, Error> {
        self.with_live(|storage| storage.find_document(address, at))
    }

    fn document(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error> {
        self.with_live(|storage| storage.document(id, at))
    }

    fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ez4a_store::DocumentRecord>, Error> {
        self.with_live(|storage| storage.scan_documents(query, limit, cursor))
    }

    fn delete_conversation(&self, id: i64) -> Result<(), Error> {
        self.with_live(|storage| storage.delete_conversation(id))
    }

    fn close(&self) -> Result<(), Error> {
        if self.closed.get() {
            return Ok(());
        }
        self.closed.set(true);
        if let Some(storage) = self.inner.borrow().as_ref().cloned() {
            storage.close()?;
        }
        Ok(())
    }
}

fn run_both(case: &'static str, body: fn(&dyn Storage)) {
    let memory = MemoryStorage::new();
    let bomb = Bomb {
        case,
        backend: "memory",
    };
    body(&Checking::new(&memory));
    drop(bomb);
    let temp = TempDb::new();
    let sqlite = SqliteStorage::open(&temp.path).unwrap();
    let bomb = Bomb {
        case,
        backend: "sqlite",
    };
    body(&Checking::new(&sqlite));
    drop(bomb);
    let temp_reopen = TempDb::new();
    let reopening = Reopening::open(&temp_reopen.path);
    let bomb = Bomb {
        case,
        backend: "sqlite-reopen",
    };
    body(&Checking::new(&reopening));
    drop(bomb);
    reopening.close().unwrap();
}

fn reserves_id_1(storage: &dyn Storage) {
    assert_eq!(storage.mint_id().unwrap(), 2);
    assert_eq!(create_root(storage), ROOT_CONVERSATION_ID);
    assert_eq!(
        storage.conversation(ROOT_CONVERSATION_ID).unwrap(),
        Some(conversation(ROOT_CONVERSATION_ID))
    );
    assert_err(
        commit_writes(
            storage,
            vec![StorageWrite::Conversation {
                value: conversation(ROOT_CONVERSATION_ID),
            }],
        ),
        &format!("ID {ROOT_CONVERSATION_ID} already belongs to conversation"),
    );
}

fn receives_durable_commit(storage: &dyn Storage) {
    let root = create_root(storage);
    let mut events = Vec::new();
    let entry_id = storage.mint_id().unwrap();
    let mut read = None;
    let mut seq = None;
    let batch = vec![vec![StorageWrite::Entry {
        value: entry(entry_id, root, "message"),
    }]];
    let result = storage
        .commit(&batch, &mut |_index, received| {
            seq = Some(received);
            events.push(format!("receive {received}"));
            read = Some(storage.entry(entry_id));
        })
        .unwrap();
    events.push(format!("resolve {}", result.stored));
    let seq = seq.unwrap();
    assert_eq!(
        events,
        vec![format!("receive {seq}"), "resolve 1".to_string()]
    );
    assert_eq!(read.unwrap().unwrap().unwrap().commit_seq, seq);
    events.clear();
    let refused = storage
        .commit(
            &[vec![StorageWrite::Conversation {
                value: conversation(root),
            }]],
            &mut |_index, received| events.push(format!("receive {received}")),
        )
        .unwrap();
    assert_eq!(refused.stored, 0);
    assert_eq!(
        refused.refused.as_deref(),
        Some(format!("Error: ID {root} already belongs to conversation").as_str())
    );
    assert!(events.is_empty());
    let next = commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(storage.mint_id().unwrap(), root, "message"),
        }],
    )
    .unwrap();
    assert!(next > seq);
}

fn commits_mixed_atomically(storage: &dyn Storage) {
    let root = create_root(storage);
    let entry_id = storage.mint_id().unwrap();
    let task_id = storage.mint_id().unwrap();
    let submission_id = storage.mint_id().unwrap();
    let task = pending(task_id, root);
    let input = submission(
        submission_id,
        root,
        Some("request-1"),
        SubmissionType::Input,
        SubmissionStatus::Placed,
    );
    let mut input = input;
    input.entry = Some(entry_id);
    let stored_entry = with_data(entry(entry_id, root, "user"), json!({ "text": "hello" }));
    let initial_seq = commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: stored_entry.clone(),
            },
            StorageWrite::Task {
                value: task.clone(),
            },
            StorageWrite::Submission {
                value: input.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        storage.entry(entry_id).unwrap(),
        Some(EntryHit {
            entry: stored_entry,
            commit_seq: initial_seq,
        })
    );
    assert_eq!(storage.task(task_id).unwrap(), Some(task.clone()));
    assert_eq!(
        storage.submission(submission_id).unwrap(),
        Some(input.clone())
    );
    let transient_entry_id = storage.mint_id().unwrap();
    let mut running = task.clone();
    running.state = TaskState::Running {
        checkpoint: json!({ "phase": "effect" }),
    };
    let mut done_input = input.clone();
    done_input.status = SubmissionStatus::Done;
    done_input.answer = Some(transient_entry_id);
    assert_err(
        commit_writes(
            storage,
            vec![
                StorageWrite::Task { value: running },
                StorageWrite::Submission { value: done_input },
                StorageWrite::Entry {
                    value: entry(transient_entry_id, root, "assistant"),
                },
                StorageWrite::Conversation {
                    value: conversation(root),
                },
            ],
        ),
        &format!("ID {root} already belongs to conversation"),
    );
    assert_eq!(storage.task(task_id).unwrap(), Some(task));
    assert_eq!(storage.submission(submission_id).unwrap(), Some(input));
    assert_eq!(storage.entry(transient_entry_id).unwrap(), None);
    let after = commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(storage.mint_id().unwrap(), root, "after-rollback"),
        }],
    )
    .unwrap();
    assert!(after > initial_seq);
}

fn detaches_retained_writes(storage: &dyn Storage) {
    let root = create_root(storage);
    let entry_id = storage.mint_id().unwrap();
    let task_id = storage.mint_id().unwrap();
    let submission_id = storage.mint_id().unwrap();
    let mut entry_data = json!({ "nested": [1, 2] });
    let mut checkpoint = json!({ "phase": "ready", "nested": { "count": 1 } });
    let mut detail = json!({ "codes": ["initial"] });
    let mut task = pending(task_id, root);
    task.state = TaskState::Pending {
        checkpoint: checkpoint.clone(),
    };
    let mut input = submission(
        submission_id,
        root,
        None,
        SubmissionType::Input,
        SubmissionStatus::Unanswered,
    );
    input.reason = Some("failed".into());
    input.detail = Some(detail.clone());
    commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: with_data(entry(entry_id, root, "note"), entry_data.clone()),
            },
            StorageWrite::Task { value: task },
            StorageWrite::Submission { value: input },
        ],
    )
    .unwrap();
    entry_data["nested"].as_array_mut().unwrap().push(json!(3));
    checkpoint["nested"]["count"] = json!(2);
    detail["codes"]
        .as_array_mut()
        .unwrap()
        .push(json!("mutated"));
    assert_eq!(
        storage.entry(entry_id).unwrap().unwrap().entry.data,
        Some(json!({ "nested": [1, 2] }))
    );
    match storage.task(task_id).unwrap().unwrap().state {
        TaskState::Pending { checkpoint } => {
            assert_eq!(
                checkpoint,
                json!({ "phase": "ready", "nested": { "count": 1 } })
            );
        }
        other => panic!("unexpected state {other:?}"),
    }
    assert_eq!(
        storage.submission(submission_id).unwrap().unwrap().detail,
        Some(json!({ "codes": ["initial"] }))
    );
    let mut read_entry = storage.entry(entry_id).unwrap().unwrap().entry;
    read_entry.data.as_mut().unwrap()["nested"]
        .as_array_mut()
        .unwrap()
        .push(json!(9));
    let mut read_task = storage.task(task_id).unwrap().unwrap();
    if let TaskState::Pending { checkpoint } = &mut read_task.state {
        checkpoint["nested"]["count"] = json!(9);
    }
    let mut read_input = storage.submission(submission_id).unwrap().unwrap();
    read_input.detail.as_mut().unwrap()["codes"]
        .as_array_mut()
        .unwrap()
        .push(json!("read mutation"));
    assert_eq!(
        storage.entry(entry_id).unwrap().unwrap().entry.data,
        Some(json!({ "nested": [1, 2] }))
    );
    match storage.task(task_id).unwrap().unwrap().state {
        TaskState::Pending { checkpoint } => {
            assert_eq!(
                checkpoint,
                json!({ "phase": "ready", "nested": { "count": 1 } })
            );
        }
        other => panic!("unexpected state {other:?}"),
    }
    assert_eq!(
        storage.submission(submission_id).unwrap().unwrap().detail,
        Some(json!({ "codes": ["initial"] }))
    );
}

fn detaches_prototype_keys(storage: &dyn Storage) {
    let root = create_root(storage);
    let entry_id = storage.mint_id().unwrap();
    let mut data = json!({
        "__proto__": { "polluted": false },
        "constructor": { "label": "stored" },
        "toString": "value"
    });
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: with_data(entry(entry_id, root, "note"), data.clone()),
        }],
    )
    .unwrap();
    data["__proto__"]["polluted"] = json!(true);
    data["constructor"]["label"] = json!("mutated");
    let mut first = storage
        .entry(entry_id)
        .unwrap()
        .unwrap()
        .entry
        .data
        .unwrap();
    assert_eq!(first["__proto__"], json!({ "polluted": false }));
    assert_eq!(first["constructor"], json!({ "label": "stored" }));
    assert_eq!(first["toString"], json!("value"));
    assert!(json!({}).get("polluted").is_none());
    first["__proto__"]["polluted"] = json!(true);
    let second = storage
        .entry(entry_id)
        .unwrap()
        .unwrap()
        .entry
        .data
        .unwrap();
    assert_eq!(second["__proto__"], json!({ "polluted": false }));
    assert_eq!(second["constructor"], json!({ "label": "stored" }));
    assert_eq!(second["toString"], json!("value"));
}

fn indexes_entries_out_of_order(storage: &dyn Storage) {
    let root = create_root(storage);
    commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: entry(30, root, "message"),
            },
            StorageWrite::Entry {
                value: entry(10, root, "message"),
            },
            StorageWrite::Entry {
                value: with_head(entry(20, root, "marker"), 10),
            },
        ],
    )
    .unwrap();
    let page = storage
        .scan_entries(
            &EntryQuery {
                conversation_id: root,
                min_entry_id: None,
                max_entry_id: None,
                order: None,
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(ids(&page, |record| record.id), vec![30, 20, 10]);
    assert_eq!(
        storage
            .find_latest_head_marker(root, None)
            .unwrap()
            .unwrap()
            .id,
        20
    );
}

fn continues_entry_cursor(storage: &dyn Storage) {
    let root = create_root(storage);
    let oldest = storage.mint_id().unwrap();
    let middle = storage.mint_id().unwrap();
    let newest = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: entry(oldest, root, "message"),
            },
            StorageWrite::Entry {
                value: entry(middle, root, "message"),
            },
            StorageWrite::Entry {
                value: entry(newest, root, "message"),
            },
        ],
    )
    .unwrap();
    let query = EntryQuery {
        conversation_id: root,
        min_entry_id: None,
        max_entry_id: None,
        order: None,
    };
    let first = storage.scan_entries(&query, 2, None).unwrap();
    assert_eq!(ids(&first, |record| record.id), vec![newest, middle]);
    let appended = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(appended, root, "message"),
        }],
    )
    .unwrap();
    let second = storage
        .scan_entries(&query, 2, first.next.as_ref())
        .unwrap();
    assert_eq!(ids(&second, |record| record.id), vec![oldest]);
    assert!(second.next.is_none());
}

fn paginates_conversations(storage: &dyn Storage) {
    let root = create_root(storage);
    let second = storage.mint_id().unwrap();
    let third = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![
            StorageWrite::Conversation {
                value: conversation(third),
            },
            StorageWrite::Conversation {
                value: conversation(second),
            },
        ],
    )
    .unwrap();
    let first = storage
        .scan_conversations(&ConversationQuery::default(), 2, None)
        .unwrap();
    assert_eq!(ids(&first, |record| record.id), vec![root, second]);
    assert!(first.next.is_some());
    let cursor = roundtrip(first.next.as_ref().unwrap());
    let next = storage
        .scan_conversations(&ConversationQuery::default(), 2, Some(&cursor))
        .unwrap();
    assert_eq!(ids(&next, |record| record.id), vec![third]);
    assert!(next.next.is_none());
}

fn filters_conversations_by_owner(storage: &dyn Storage) {
    let root = create_root(storage);
    let other_owner = storage.mint_id().unwrap();
    let first_task = storage.mint_id().unwrap();
    let second_task = storage.mint_id().unwrap();
    let first = storage.mint_id().unwrap();
    let second = storage.mint_id().unwrap();
    let third = storage.mint_id().unwrap();
    let owned = |id, conversation_id, task_id| {
        let mut record = conversation(id);
        record.owner = Some(ConversationOwner {
            conversation_id,
            task_id,
        });
        StorageWrite::Conversation { value: record }
    };
    commit_writes(
        storage,
        vec![
            StorageWrite::Conversation {
                value: conversation(other_owner),
            },
            owned(first, root, first_task),
            owned(second, root, second_task),
            owned(third, other_owner, first_task),
        ],
    )
    .unwrap();
    let by_owner = ConversationQuery {
        owner_conversation_id: Some(root),
        ..ConversationQuery::default()
    };
    let page = storage.scan_conversations(&by_owner, 1, None).unwrap();
    assert_eq!(ids(&page, |record| record.id), vec![first]);
    assert!(page.next.is_some());
    let next = storage
        .scan_conversations(&by_owner, 1, page.next.as_ref())
        .unwrap();
    assert_eq!(ids(&next, |record| record.id), vec![second]);
    assert!(next.next.is_none());
    let by_task = ConversationQuery {
        owner_task_id: Some(first_task),
        ..ConversationQuery::default()
    };
    let page = storage.scan_conversations(&by_task, 10, None).unwrap();
    assert_eq!(ids(&page, |record| record.id), vec![first, third]);
    let both = ConversationQuery {
        owner_conversation_id: Some(root),
        owner_task_id: Some(first_task),
        ..ConversationQuery::default()
    };
    let page = storage.scan_conversations(&both, 10, None).unwrap();
    assert_eq!(ids(&page, |record| record.id), vec![first]);
}

fn scans_deep_fork(storage: &dyn Storage) {
    let root = create_root(storage);
    let root_first = storage.mint_id().unwrap();
    let root_fork = storage.mint_id().unwrap();
    let root_excluded_same = storage.mint_id().unwrap();
    let root_seq = commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: entry(root_first, root, "message"),
            },
            StorageWrite::Entry {
                value: with_head(entry(root_fork, root, "marker"), root_first),
            },
            StorageWrite::Entry {
                value: entry(root_excluded_same, root, "message"),
            },
        ],
    )
    .unwrap();
    let child = storage.mint_id().unwrap();
    let mut child_record = conversation(child);
    child_record.parent = Some(ConversationParent {
        conversation_id: root,
        at: root_fork,
    });
    commit_writes(
        storage,
        vec![StorageWrite::Conversation {
            value: child_record,
        }],
    )
    .unwrap();
    let child_fork = storage.mint_id().unwrap();
    let child_excluded = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: entry(child_fork, child, "note"),
            },
            StorageWrite::Entry {
                value: entry(child_excluded, child, "message"),
            },
        ],
    )
    .unwrap();
    let root_excluded_later = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(root_excluded_later, root, "message"),
        }],
    )
    .unwrap();
    let grandchild = storage.mint_id().unwrap();
    let mut grandchild_record = conversation(grandchild);
    grandchild_record.parent = Some(ConversationParent {
        conversation_id: child,
        at: child_fork,
    });
    commit_writes(
        storage,
        vec![StorageWrite::Conversation {
            value: grandchild_record,
        }],
    )
    .unwrap();
    let grandchild_head = storage.mint_id().unwrap();
    let grandchild_tail = storage.mint_id().unwrap();
    let grandchild_seq = commit_writes(
        storage,
        vec![
            StorageWrite::Entry {
                value: with_head(
                    entry(grandchild_head, grandchild, "marker"),
                    grandchild_head,
                ),
            },
            StorageWrite::Entry {
                value: entry(grandchild_tail, grandchild, "message"),
            },
        ],
    )
    .unwrap();
    let child_excluded_later = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(child_excluded_later, child, "message"),
        }],
    )
    .unwrap();
    let query = |order| EntryQuery {
        conversation_id: grandchild,
        min_entry_id: None,
        max_entry_id: None,
        order,
    };
    let first = storage.scan_entries(&query(None), 2, None).unwrap();
    assert_eq!(
        ids(&first, |record| record.id),
        vec![grandchild_tail, grandchild_head]
    );
    let second = storage
        .scan_entries(&query(None), 2, first.next.as_ref())
        .unwrap();
    assert_eq!(
        ids(&second, |record| record.id),
        vec![child_fork, root_fork]
    );
    let third = storage
        .scan_entries(&query(None), 2, second.next.as_ref())
        .unwrap();
    assert_eq!(ids(&third, |record| record.id), vec![root_first]);
    assert!(third.next.is_none());
    let ascending = query(Some(ScanOrder::Ascending));
    let first_up = storage.scan_entries(&ascending, 2, None).unwrap();
    assert_eq!(
        ids(&first_up, |record| record.id),
        vec![root_first, root_fork]
    );
    let second_up = storage
        .scan_entries(&ascending, 2, first_up.next.as_ref())
        .unwrap();
    assert_eq!(
        ids(&second_up, |record| record.id),
        vec![child_fork, grandchild_head]
    );
    let third_up = storage
        .scan_entries(&query(None), 2, second_up.next.as_ref())
        .unwrap();
    assert_eq!(ids(&third_up, |record| record.id), vec![grandchild_tail]);
    assert!(third_up.next.is_none());
    let window = EntryQuery {
        conversation_id: grandchild,
        min_entry_id: Some(root_fork),
        max_entry_id: Some(child_fork),
        order: Some(ScanOrder::Ascending),
    };
    assert_eq!(
        ids(
            &storage.scan_entries(&window, 10, None).unwrap(),
            |record| record.id
        ),
        vec![root_fork, child_fork]
    );
    assert_err(
        storage.scan_entries(
            &query(Some(ScanOrder::Descending)),
            2,
            first_up.next.as_ref(),
        ),
        "cursor",
    );
    let current = storage
        .find_latest_head_marker(grandchild, None)
        .unwrap()
        .unwrap();
    assert_eq!(current.id, grandchild_head);
    assert_eq!(current.head, Some(grandchild_head));
    let historical = storage
        .find_latest_head_marker(grandchild, Some(child_fork))
        .unwrap()
        .unwrap();
    assert_eq!(historical.id, root_fork);
    assert_eq!(historical.head, Some(root_first));
    assert!(storage
        .find_latest_head_marker(grandchild, Some(root_first))
        .unwrap()
        .is_none());
    let active = EntryQuery {
        conversation_id: grandchild,
        min_entry_id: current.head,
        max_entry_id: None,
        order: None,
    };
    let active_first = storage.scan_entries(&active, 1, None).unwrap();
    assert_eq!(
        ids(&active_first, |record| record.id),
        vec![grandchild_tail]
    );
    assert!(active_first.next.is_some());
    let active_second = storage
        .scan_entries(&active, 1, active_first.next.as_ref())
        .unwrap();
    assert_eq!(
        ids(&active_second, |record| record.id),
        vec![grandchild_head]
    );
    assert!(active_second.next.is_none());
    let historical_window = EntryQuery {
        conversation_id: grandchild,
        min_entry_id: historical.head,
        max_entry_id: Some(child_fork),
        order: None,
    };
    assert_eq!(
        ids(
            &storage.scan_entries(&historical_window, 10, None).unwrap(),
            |record| record.id
        ),
        vec![child_fork, root_fork, root_first]
    );
    assert_eq!(
        storage.entry(root_first).unwrap(),
        Some(EntryHit {
            entry: entry(root_first, root, "message"),
            commit_seq: root_seq
        })
    );
    assert_eq!(
        storage.entry(root_fork).unwrap().unwrap().commit_seq,
        root_seq
    );
    assert_eq!(
        storage.entry(grandchild_head).unwrap().unwrap().commit_seq,
        grandchild_seq
    );
    assert_eq!(
        storage.entry(grandchild_tail).unwrap().unwrap().commit_seq,
        grandchild_seq
    );
    assert!(storage.entry(999_999).unwrap().is_none());
    assert_eq!(
        storage.entry_in(grandchild, root_first).unwrap(),
        Some(EntryHit {
            entry: entry(root_first, root, "message"),
            commit_seq: root_seq
        })
    );
    assert_eq!(
        storage
            .entry_in(grandchild, child_fork)
            .unwrap()
            .unwrap()
            .entry
            .conversation_id,
        child
    );
    assert_eq!(
        storage
            .entry_in(grandchild, grandchild_tail)
            .unwrap()
            .unwrap()
            .commit_seq,
        grandchild_seq
    );
    assert!(storage
        .entry_in(grandchild, root_excluded_same)
        .unwrap()
        .is_none());
    assert!(storage
        .entry_in(grandchild, root_excluded_later)
        .unwrap()
        .is_none());
    assert!(storage
        .entry_in(grandchild, child_excluded)
        .unwrap()
        .is_none());
    assert!(storage
        .entry_in(grandchild, child_excluded_later)
        .unwrap()
        .is_none());
    assert!(storage.entry_in(root, grandchild_head).unwrap().is_none());
    assert!(storage.entry_in(grandchild, 999_999).unwrap().is_none());
    assert_err(
        storage.entry_in(999_999, root_first),
        "Unknown conversation",
    );
    assert_err(
        storage.scan_entries(
            &EntryQuery {
                conversation_id: 999_999,
                min_entry_id: None,
                max_entry_id: None,
                order: None,
            },
            10,
            None,
        ),
        "Unknown conversation",
    );
}

fn scans_either_order(storage: &dyn Storage) {
    let root = create_root(storage);
    let mut conversation_ids = vec![root];
    let mut task_ids = Vec::new();
    let mut submission_ids = Vec::new();
    for _ in 0..3 {
        let conversation_id = storage.mint_id().unwrap();
        let task_id = storage.mint_id().unwrap();
        let submission_id = storage.mint_id().unwrap();
        commit_writes(
            storage,
            vec![
                StorageWrite::Conversation {
                    value: conversation(conversation_id),
                },
                StorageWrite::Task {
                    value: pending(task_id, root),
                },
                StorageWrite::Submission {
                    value: submission(
                        submission_id,
                        root,
                        None,
                        SubmissionType::Input,
                        SubmissionStatus::Queued,
                    ),
                },
            ],
        )
        .unwrap();
        conversation_ids.push(conversation_id);
        task_ids.push(task_id);
        submission_ids.push(submission_id);
    }
    let all_conversations = |order: Option<ScanOrder>| -> Vec<i64> {
        let mut found = Vec::new();
        let mut cursor = None;
        loop {
            let requested = if found.is_empty() { order } else { None };
            let page = storage
                .scan_conversations(
                    &ConversationQuery {
                        order: requested,
                        ..ConversationQuery::default()
                    },
                    2,
                    cursor.as_ref(),
                )
                .unwrap();
            found.extend(page.items.iter().map(|record| record.id));
            match page.next {
                Some(next) => cursor = Some(roundtrip(&next)),
                None => break,
            }
        }
        found
    };
    let all_tasks = |order: Option<ScanOrder>| -> Vec<i64> {
        let mut found = Vec::new();
        let mut cursor = None;
        loop {
            let requested = if found.is_empty() { order } else { None };
            let page = storage
                .scan_tasks(
                    &TaskQuery {
                        order: requested,
                        ..TaskQuery::default()
                    },
                    2,
                    cursor.as_ref(),
                )
                .unwrap();
            found.extend(page.items.iter().map(|record| record.id));
            match page.next {
                Some(next) => cursor = Some(roundtrip(&next)),
                None => break,
            }
        }
        found
    };
    let all_submissions = |order: Option<ScanOrder>| -> Vec<i64> {
        let mut found = Vec::new();
        let mut cursor = None;
        loop {
            let requested = if found.is_empty() { order } else { None };
            let page = storage
                .scan_submissions(
                    &SubmissionQuery {
                        order: requested,
                        ..SubmissionQuery::default()
                    },
                    2,
                    cursor.as_ref(),
                )
                .unwrap();
            found.extend(page.items.iter().map(|record| record.id));
            match page.next {
                Some(next) => cursor = Some(roundtrip(&next)),
                None => break,
            }
        }
        found
    };
    for (found_default, found_up, found_down, ids) in [
        (
            all_conversations(None),
            all_conversations(Some(ScanOrder::Ascending)),
            all_conversations(Some(ScanOrder::Descending)),
            conversation_ids.clone(),
        ),
        (
            all_tasks(None),
            all_tasks(Some(ScanOrder::Ascending)),
            all_tasks(Some(ScanOrder::Descending)),
            task_ids.clone(),
        ),
        (
            all_submissions(None),
            all_submissions(Some(ScanOrder::Ascending)),
            all_submissions(Some(ScanOrder::Descending)),
            submission_ids.clone(),
        ),
    ] {
        let reversed: Vec<i64> = ids.iter().rev().copied().collect();
        assert_eq!(found_default, ids);
        assert_eq!(found_up, ids);
        assert_eq!(found_down, reversed);
    }
    let reversed: Vec<i64> = conversation_ids.iter().rev().copied().collect();
    let descending = storage
        .scan_conversations(
            &ConversationQuery {
                order: Some(ScanOrder::Descending),
                ..ConversationQuery::default()
            },
            2,
            None,
        )
        .unwrap();
    let next = storage
        .scan_conversations(
            &ConversationQuery {
                order: Some(ScanOrder::Descending),
                ..ConversationQuery::default()
            },
            2,
            descending.next.as_ref(),
        )
        .unwrap();
    assert_eq!(
        ids(&next, |record| record.id),
        reversed.iter().skip(2).take(2).copied().collect::<Vec<_>>()
    );
    assert_err(
        storage.scan_conversations(
            &ConversationQuery {
                order: Some(ScanOrder::Ascending),
                ..ConversationQuery::default()
            },
            2,
            descending.next.as_ref(),
        ),
        "cursor",
    );
    let reversed: Vec<i64> = task_ids.iter().rev().copied().collect();
    let descending = storage
        .scan_tasks(
            &TaskQuery {
                order: Some(ScanOrder::Descending),
                ..TaskQuery::default()
            },
            2,
            None,
        )
        .unwrap();
    let next = storage
        .scan_tasks(
            &TaskQuery {
                order: Some(ScanOrder::Descending),
                ..TaskQuery::default()
            },
            2,
            descending.next.as_ref(),
        )
        .unwrap();
    assert_eq!(
        ids(&next, |record| record.id),
        reversed.iter().skip(2).take(2).copied().collect::<Vec<_>>()
    );
    assert_err(
        storage.scan_tasks(
            &TaskQuery {
                order: Some(ScanOrder::Ascending),
                ..TaskQuery::default()
            },
            2,
            descending.next.as_ref(),
        ),
        "cursor",
    );
    let reversed: Vec<i64> = submission_ids.iter().rev().copied().collect();
    let descending = storage
        .scan_submissions(
            &SubmissionQuery {
                order: Some(ScanOrder::Descending),
                ..SubmissionQuery::default()
            },
            2,
            None,
        )
        .unwrap();
    let next = storage
        .scan_submissions(
            &SubmissionQuery {
                order: Some(ScanOrder::Descending),
                ..SubmissionQuery::default()
            },
            2,
            descending.next.as_ref(),
        )
        .unwrap();
    assert_eq!(
        ids(&next, |record| record.id),
        reversed.iter().skip(2).take(2).copied().collect::<Vec<_>>()
    );
    assert_err(
        storage.scan_submissions(
            &SubmissionQuery {
                order: Some(ScanOrder::Ascending),
                ..SubmissionQuery::default()
            },
            2,
            descending.next.as_ref(),
        ),
        "cursor",
    );
}

fn replaces_tasks(storage: &dyn Storage) {
    let root = create_root(storage);
    let first_id = storage.mint_id().unwrap();
    let second_id = storage.mint_id().unwrap();
    let third_id = storage.mint_id().unwrap();
    let mut first = pending(first_id, root);
    first.memos = Some(json!({ "winner": "first" }));
    let mut second = pending(second_id, root);
    second.background = true;
    let mut third = pending(third_id, root);
    third.abort_requested = true;
    commit_writes(
        storage,
        vec![
            StorageWrite::Task {
                value: first.clone(),
            },
            StorageWrite::Task {
                value: second.clone(),
            },
            StorageWrite::Task { value: third },
        ],
    )
    .unwrap();
    let mut running = first.clone();
    running.state = TaskState::Running {
        checkpoint: json!({ "phase": "effect", "attempt": 1 }),
    };
    running.abort_requested = true;
    commit_writes(storage, vec![StorageWrite::Task { value: running }]).unwrap();
    let terminal = TaskRecord {
        trace: None,
        links: None,
        id: first_id,
        conversation_id: root,
        kind: first.kind.clone(),
        version: first.version,
        input: first.input.clone(),
        owner: None,
        background: false,
        abort_requested: true,
        abort_reason: None,
        abandon_on_restart: None,
        started_at: None,
        ended_at: None,
        restarts: None,
        state: TaskState::Terminal {
            outcome: TaskOutcome::Completed {
                result: json!({ "entryId": 99 }),
            },
        },
        memos: None,
    };
    commit_writes(
        storage,
        vec![StorageWrite::Task {
            value: terminal.clone(),
        }],
    )
    .unwrap();
    assert_eq!(storage.task(first_id).unwrap(), Some(terminal.clone()));
    let pending_page = storage
        .scan_tasks(
            &TaskQuery {
                status: Some(TaskStatus::Pending),
                ..TaskQuery::default()
            },
            1,
            None,
        )
        .unwrap();
    assert_eq!(ids(&pending_page, |record| record.id), vec![second_id]);
    assert!(pending_page.next.is_some());
    let rest = storage
        .scan_tasks(
            &TaskQuery {
                status: Some(TaskStatus::Pending),
                ..TaskQuery::default()
            },
            1,
            pending_page.next.as_ref(),
        )
        .unwrap();
    assert_eq!(ids(&rest, |record| record.id), vec![third_id]);
    let terminal_page = storage
        .scan_tasks(
            &TaskQuery {
                status: Some(TaskStatus::Terminal),
                abort_requested: Some(true),
                ..TaskQuery::default()
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(terminal_page.items, vec![terminal]);
    let background = storage
        .scan_tasks(
            &TaskQuery {
                background: Some(true),
                ..TaskQuery::default()
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(ids(&background, |record| record.id), vec![second_id]);
}

fn waiting_and_completing(storage: &dyn Storage) {
    let root = create_root(storage);
    let owner_id = storage.mint_id().unwrap();
    let waiting_id = storage.mint_id().unwrap();
    let completing_id = storage.mint_id().unwrap();
    let owner = pending(owner_id, root);
    let mut waiting = pending(waiting_id, root);
    waiting.owner = Some(owner_id);
    waiting.state = TaskState::Waiting {
        checkpoint: json!({ "phase": "next" }),
        on: vec![owner_id],
        policy: JoinPolicy::AllSettled,
    };
    waiting.memos = Some(json!({ "kept": true }));
    let outcome = TaskOutcome::Failed {
        error: TaskOutcomeError {
            message: "held".into(),
            detail: None,
        },
        result: None,
    };
    let mut completing = pending(completing_id, root);
    completing.owner = Some(owner_id);
    completing.state = TaskState::Completing {
        outcome: outcome.clone(),
    };
    commit_writes(
        storage,
        vec![
            StorageWrite::Task { value: owner },
            StorageWrite::Task {
                value: waiting.clone(),
            },
            StorageWrite::Task {
                value: completing.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(storage.task(waiting_id).unwrap(), Some(waiting.clone()));
    assert_eq!(
        storage.task(completing_id).unwrap(),
        Some(completing.clone())
    );
    let scan = |status| {
        storage
            .scan_tasks(
                &TaskQuery {
                    status: Some(status),
                    ..TaskQuery::default()
                },
                10,
                None,
            )
            .unwrap()
            .items
    };
    assert_eq!(scan(TaskStatus::Waiting), vec![waiting]);
    assert_eq!(scan(TaskStatus::Completing), vec![completing.clone()]);
    assert_eq!(
        scan(TaskStatus::Pending)
            .into_iter()
            .map(|task| task.id)
            .collect::<Vec<_>>(),
        vec![owner_id]
    );
    let mut terminal = completing;
    terminal.state = TaskState::Terminal { outcome };
    commit_writes(
        storage,
        vec![StorageWrite::Task {
            value: terminal.clone(),
        }],
    )
    .unwrap();
    assert!(scan(TaskStatus::Completing).is_empty());
    assert_eq!(scan(TaskStatus::Terminal), vec![terminal]);
}

fn sleeping_tasks(storage: &dyn Storage) {
    let root = create_root(storage);
    let sleeping_id = storage.mint_id().unwrap();
    let mut sleeping = pending(sleeping_id, root);
    sleeping.state = TaskState::Sleeping {
        checkpoint: json!({ "phase": "next" }),
        until: 61_000,
    };
    sleeping.memos = Some(json!({ "kept": true }));
    commit_writes(
        storage,
        vec![StorageWrite::Task {
            value: sleeping.clone(),
        }],
    )
    .unwrap();
    assert_eq!(storage.task(sleeping_id).unwrap(), Some(sleeping.clone()));
    let scan = |status| {
        storage
            .scan_tasks(
                &TaskQuery {
                    status: Some(status),
                    ..TaskQuery::default()
                },
                10,
                None,
            )
            .unwrap()
            .items
    };
    assert_eq!(scan(TaskStatus::Sleeping), vec![sleeping.clone()]);
    let mut running = sleeping;
    running.state = TaskState::Running {
        checkpoint: json!({ "phase": "next" }),
    };
    commit_writes(
        storage,
        vec![StorageWrite::Task {
            value: running.clone(),
        }],
    )
    .unwrap();
    assert!(scan(TaskStatus::Sleeping).is_empty());
    assert_eq!(scan(TaskStatus::Running), vec![running]);
}

fn indexes_request_ids(storage: &dyn Storage) {
    let root = create_root(storage);
    let second_conversation = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![StorageWrite::Conversation {
            value: conversation(second_conversation),
        }],
    )
    .unwrap();
    let first_id = storage.mint_id().unwrap();
    let second_id = storage.mint_id().unwrap();
    let other_id = storage.mint_id().unwrap();
    let first = submission(
        first_id,
        root,
        Some("same"),
        SubmissionType::Input,
        SubmissionStatus::Queued,
    );
    let second = submission(
        second_id,
        root,
        Some("other"),
        SubmissionType::Input,
        SubmissionStatus::Queued,
    );
    let other = submission(
        other_id,
        second_conversation,
        Some("same"),
        SubmissionType::Input,
        SubmissionStatus::Queued,
    );
    commit_writes(
        storage,
        vec![
            StorageWrite::Submission {
                value: first.clone(),
            },
            StorageWrite::Submission {
                value: second.clone(),
            },
            StorageWrite::Submission {
                value: other.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        storage.submission_by_request(root, "same").unwrap(),
        Some(first)
    );
    assert_eq!(
        storage
            .submission_by_request(second_conversation, "same")
            .unwrap(),
        Some(other.clone())
    );
    let mut placed = second;
    placed.status = SubmissionStatus::Placed;
    placed.entry = Some(storage.mint_id().unwrap());
    commit_writes(
        storage,
        vec![StorageWrite::Submission {
            value: placed.clone(),
        }],
    )
    .unwrap();
    assert_eq!(storage.submission(second_id).unwrap(), Some(placed.clone()));
    assert_eq!(
        storage.submission_by_request(root, "other").unwrap(),
        Some(placed.clone())
    );
    let collect = |query: SubmissionQuery| -> Vec<i64> {
        let mut found = Vec::new();
        let mut cursor = None;
        loop {
            let page = storage
                .scan_submissions(&query, 1, cursor.as_ref())
                .unwrap();
            found.extend(page.items.iter().map(|record| record.id));
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        found
    };
    assert_eq!(
        collect(SubmissionQuery::default()),
        vec![first_id, second_id, other_id]
    );
    assert_eq!(
        collect(SubmissionQuery {
            conversation_id: Some(root),
            ..SubmissionQuery::default()
        }),
        vec![first_id, second_id]
    );
    assert_eq!(
        collect(SubmissionQuery {
            status: Some(SubmissionStatus::Queued),
            ..SubmissionQuery::default()
        }),
        vec![first_id, other_id]
    );
    assert_eq!(
        collect(SubmissionQuery {
            status: Some(SubmissionStatus::Placed),
            ..SubmissionQuery::default()
        }),
        vec![second_id]
    );
    assert_eq!(
        collect(SubmissionQuery {
            conversation_id: Some(second_conversation),
            status: Some(SubmissionStatus::Queued),
            ..SubmissionQuery::default()
        }),
        vec![other_id]
    );
    assert!(collect(SubmissionQuery {
        conversation_id: Some(second_conversation),
        status: Some(SubmissionStatus::Placed),
        ..SubmissionQuery::default()
    })
    .is_empty());
    let placed_page = storage
        .scan_submissions(
            &SubmissionQuery {
                status: Some(SubmissionStatus::Placed),
                ..SubmissionQuery::default()
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(placed_page.items, vec![placed]);
    let _ = other;
}

fn passive_writes(storage: &dyn Storage) {
    let root = create_root(storage);
    let done_id = storage.mint_id().unwrap();
    let failed_id = storage.mint_id().unwrap();
    let queued_done = submission(
        done_id,
        root,
        Some("passive-done"),
        SubmissionType::Write,
        SubmissionStatus::Queued,
    );
    let queued_failed = submission(
        failed_id,
        root,
        Some("passive-failed"),
        SubmissionType::Write,
        SubmissionStatus::Queued,
    );
    commit_writes(
        storage,
        vec![
            StorageWrite::Submission {
                value: queued_done.clone(),
            },
            StorageWrite::Submission {
                value: queued_failed.clone(),
            },
        ],
    )
    .unwrap();
    let mut done = queued_done;
    done.status = SubmissionStatus::Done;
    done.entry = Some(storage.mint_id().unwrap());
    let mut unanswered = queued_failed;
    unanswered.status = SubmissionStatus::Unanswered;
    unanswered.reason = Some("closed".into());
    unanswered.detail = Some(json!({ "retryable": false }));
    commit_writes(
        storage,
        vec![
            StorageWrite::Submission {
                value: done.clone(),
            },
            StorageWrite::Submission {
                value: unanswered.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(storage.submission(done_id).unwrap(), Some(done.clone()));
    assert_eq!(
        storage.submission_by_request(root, "passive-done").unwrap(),
        Some(done)
    );
    assert_eq!(
        storage.submission(failed_id).unwrap(),
        Some(unanswered.clone())
    );
    assert_eq!(
        storage
            .submission_by_request(root, "passive-failed")
            .unwrap(),
        Some(unanswered)
    );
}

fn continue_submissions(storage: &dyn Storage) {
    let root = create_root(storage);
    let done_id = storage.mint_id().unwrap();
    let failed_id = storage.mint_id().unwrap();
    let placed_done = submission(
        done_id,
        root,
        Some("continue-done"),
        SubmissionType::Continue,
        SubmissionStatus::Placed,
    );
    let placed_failed = submission(
        failed_id,
        root,
        Some("continue-failed"),
        SubmissionType::Continue,
        SubmissionStatus::Placed,
    );
    commit_writes(
        storage,
        vec![
            StorageWrite::Submission {
                value: placed_done.clone(),
            },
            StorageWrite::Submission {
                value: placed_failed.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(
        storage.submission(done_id).unwrap(),
        Some(placed_done.clone())
    );
    let placed = storage
        .scan_submissions(
            &SubmissionQuery {
                status: Some(SubmissionStatus::Placed),
                ..SubmissionQuery::default()
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(
        placed.items,
        vec![placed_done.clone(), placed_failed.clone()]
    );
    let mut done = placed_done;
    done.status = SubmissionStatus::Done;
    done.answer = Some(storage.mint_id().unwrap());
    let mut unanswered = placed_failed;
    unanswered.status = SubmissionStatus::Unanswered;
    unanswered.reason = Some("nothing_to_answer".into());
    commit_writes(
        storage,
        vec![
            StorageWrite::Submission {
                value: done.clone(),
            },
            StorageWrite::Submission {
                value: unanswered.clone(),
            },
        ],
    )
    .unwrap();
    assert_eq!(storage.submission(done_id).unwrap(), Some(done.clone()));
    assert_eq!(
        storage
            .submission_by_request(root, "continue-done")
            .unwrap(),
        Some(done)
    );
    assert_eq!(
        storage.submission(failed_id).unwrap(),
        Some(unanswered.clone())
    );
    assert_eq!(
        storage
            .submission_by_request(root, "continue-failed")
            .unwrap(),
        Some(unanswered)
    );
}

fn doc_create(record: DocumentCreate, content: DocumentContent) -> StorageWrite {
    StorageWrite::DocumentCreate { record, content }
}

fn rewindable_documents(storage: &dyn Storage) {
    let root = create_root(storage);
    let first_id = storage.mint_id().unwrap();
    let record = DocumentCreate {
        id: first_id,
        kind: "conversation.notes".into(),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: root,
        },
        history: Some(DocumentHistory::Rewindable),
        fork: Some(DocumentFork::AsOf),
    };
    let mut initial = json!({ "items": ["a"], "nested": { "count": 1 } });
    let created_at = commit_writes(
        storage,
        vec![doc_create(record.clone(), base(1, initial.clone()))],
    )
    .unwrap();
    let mut appended = vec![json!("b")];
    let ops = vec![
        op_splice(vec![key("items")], 1, 0, appended.clone()),
        op_set(vec![key("nested"), key("count")], json!(2)),
    ];
    let changed_at = commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id: first_id,
            content: delta(1, ops),
        }],
    )
    .unwrap();
    initial["items"]
        .as_array_mut()
        .unwrap()
        .push(json!("caller mutation"));
    appended.push(json!("caller mutation"));
    let created = storage
        .document(first_id, DocumentPoint::Seq(created_at))
        .unwrap()
        .unwrap();
    assert_eq!(created.version, 1);
    assert_eq!(
        created.value,
        json!({ "items": ["a"], "nested": { "count": 1 } })
    );
    assert_eq!(created.deltas_since_base, 0);
    let mut changed = storage
        .document(first_id, DocumentPoint::Seq(changed_at))
        .unwrap()
        .unwrap();
    assert_eq!(
        changed.value,
        json!({ "items": ["a", "b"], "nested": { "count": 2 } })
    );
    assert_eq!(changed.deltas_since_base, 1);
    changed.value["items"]
        .as_array_mut()
        .unwrap()
        .push(json!("read mutation"));
    assert_eq!(
        storage
            .document(first_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "items": ["a", "b"], "nested": { "count": 2 } })
    );
    let checkpoint_at = commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id: first_id,
            content: base(
                2,
                json!({ "items": ["checkpoint"], "nested": { "count": 3 } }),
            ),
        }],
    )
    .unwrap();
    let replaced_at = commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id: first_id,
            content: delta(
                2,
                vec![Op::Replace(
                    json!({ "items": ["replacement"], "nested": { "count": 4 } }),
                )],
            ),
        }],
    )
    .unwrap();
    let at_change = storage
        .document(first_id, DocumentPoint::Seq(changed_at))
        .unwrap()
        .unwrap();
    assert_eq!(at_change.version, 1);
    assert_eq!(
        at_change.value,
        json!({ "items": ["a", "b"], "nested": { "count": 2 } })
    );
    let at_checkpoint = storage
        .document(first_id, DocumentPoint::Seq(checkpoint_at))
        .unwrap()
        .unwrap();
    assert_eq!(at_checkpoint.version, 2);
    assert_eq!(
        at_checkpoint.value,
        json!({ "items": ["checkpoint"], "nested": { "count": 3 } })
    );
    assert_eq!(at_checkpoint.deltas_since_base, 0);
    let at_replaced = storage
        .document(first_id, DocumentPoint::Seq(replaced_at))
        .unwrap()
        .unwrap();
    assert_eq!(
        at_replaced.value,
        json!({ "items": ["replacement"], "nested": { "count": 4 } })
    );
    assert_eq!(at_replaced.deltas_since_base, 1);
    assert_eq!(
        storage
            .document(first_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .deltas_since_base,
        1
    );
    let second_id = storage.mint_id().unwrap();
    let mut second = record.clone();
    second.id = second_id;
    let retired_at = commit_writes(
        storage,
        vec![
            doc_create(second, base(1, json!({ "items": ["new"] }))),
            StorageWrite::DocumentRetire { id: first_id },
            StorageWrite::DocumentChange {
                id: first_id,
                content: delta(2, vec![op_set(vec![key("retiring")], json!(true))]),
            },
        ],
    )
    .unwrap();
    let address = DocumentAddress {
        kind: record.kind.clone(),
        scope: record.scope,
        key: None,
    };
    assert_eq!(
        storage
            .find_document(&address, DocumentPoint::Seq(changed_at))
            .unwrap()
            .unwrap()
            .id,
        first_id
    );
    let found = storage
        .find_document(&address, DocumentPoint::Seq(retired_at))
        .unwrap()
        .unwrap();
    assert_eq!(found.id, second_id);
    assert_eq!(found.created_at, retired_at);
    let scope_at = |at| DocumentQuery {
        scope: record.scope,
        at,
        kind: None,
    };
    assert_eq!(
        ids(
            &storage
                .scan_documents(&scope_at(DocumentPoint::Seq(changed_at)), 10, None)
                .unwrap(),
            |record| record.id
        ),
        vec![first_id]
    );
    assert_eq!(
        ids(
            &storage
                .scan_documents(&scope_at(DocumentPoint::Seq(retired_at)), 10, None)
                .unwrap(),
            |record| record.id
        ),
        vec![second_id]
    );
    assert!(storage
        .document(first_id, DocumentPoint::Seq(retired_at))
        .unwrap()
        .is_none());
    assert_eq!(
        storage
            .document(second_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "items": ["new"] })
    );
}

fn long_rows(prefix: &str, start: i64) -> Vec<Json> {
    (0..512)
        .map(|value| json!({ "value": start + value as i64, "stable": format!("{prefix}{value}") }))
        .collect()
}

fn streams_long_tails(storage: &dyn Storage) {
    let root = create_root(storage);
    let id = storage.mint_id().unwrap();
    let record = DocumentCreate {
        id,
        kind: "conversation.long-tail".into(),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: root,
        },
        history: Some(DocumentHistory::Rewindable),
        fork: Some(DocumentFork::AsOf),
    };
    let initial = json!({ "revision": 0, "rows": long_rows("row-", 0) });
    let created_at =
        commit_writes(storage, vec![doc_create(record, base(1, initial.clone()))]).unwrap();
    let mut before = initial.clone();
    let mut before_at = created_at;
    for revision in 1..=24 {
        let revision = revision as i64;
        let index = (revision as usize * 17) % 512;
        before["rows"][index]["value"] = json!(-revision);
        before["revision"] = json!(revision);
        before_at = commit_writes(
            storage,
            vec![StorageWrite::DocumentChange {
                id,
                content: delta(
                    1,
                    vec![
                        op_set(
                            vec![key("rows"), Seg::Index(index as i64), key("value")],
                            json!(-revision),
                        ),
                        op_set(vec![key("revision")], json!(revision)),
                    ],
                ),
            }],
        )
        .unwrap();
    }
    let replacement = json!({ "revision": 100, "rows": long_rows("new-", 10_000) });
    let snapshot = replacement.clone();
    let replacement_at = commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id,
            content: delta(1, vec![Op::Replace(replacement)]),
        }],
    )
    .unwrap();
    let mut current = snapshot.clone();
    for revision in 101..=124 {
        let revision = revision as i64;
        let index = (revision as usize * 19) % 512;
        current["rows"][index]["value"] = json!(-revision);
        current["revision"] = json!(revision);
        commit_writes(
            storage,
            vec![StorageWrite::DocumentChange {
                id,
                content: delta(
                    1,
                    vec![
                        op_set(
                            vec![key("rows"), Seg::Index(index as i64), key("value")],
                            json!(-revision),
                        ),
                        op_set(vec![key("revision")], json!(revision)),
                    ],
                ),
            }],
        )
        .unwrap();
    }
    assert_eq!(
        storage
            .document(id, DocumentPoint::Seq(created_at))
            .unwrap()
            .unwrap()
            .value,
        initial
    );
    assert_eq!(
        storage
            .document(id, DocumentPoint::Seq(before_at))
            .unwrap()
            .unwrap()
            .value,
        before
    );
    assert_eq!(
        storage
            .document(id, DocumentPoint::Seq(replacement_at))
            .unwrap()
            .unwrap()
            .value,
        snapshot
    );
    let mut read = storage
        .document(id, DocumentPoint::Current)
        .unwrap()
        .unwrap();
    assert_eq!(read.value, current);
    read.value["rows"][0]["value"] = json!(-1_000);
    assert_eq!(
        storage
            .document(id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        current
    );
}

fn copies_bases(storage: &dyn Storage) {
    let root = create_root(storage);
    let child = storage.mint_id().unwrap();
    let second_child = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![
            StorageWrite::Conversation {
                value: conversation(child),
            },
            StorageWrite::Conversation {
                value: conversation(second_child),
            },
        ],
    )
    .unwrap();
    let source_id = storage.mint_id().unwrap();
    let source = DocumentCreate {
        id: source_id,
        kind: "copy.source".into(),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: root,
        },
        history: Some(DocumentHistory::Rewindable),
        fork: Some(DocumentFork::AsOf),
    };
    let created_at = commit_writes(
        storage,
        vec![doc_create(
            source.clone(),
            base(2, json!({ "count": 1, "rows": [{ "value": "base" }] })),
        )],
    )
    .unwrap();
    commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id: source_id,
            content: delta(
                2,
                vec![
                    op_set(vec![key("count")], json!(2)),
                    op_splice(vec![key("rows")], 1, 0, vec![json!({ "value": "current" })]),
                ],
            ),
        }],
    )
    .unwrap();
    let historical_id = storage.mint_id().unwrap();
    let current_id = storage.mint_id().unwrap();
    let retired_id = storage.mint_id().unwrap();
    let child_record = |id, conversation_id| DocumentCreate {
        id,
        kind: source.kind.clone(),
        key: None,
        scope: DocumentScope::Conversation { conversation_id },
        history: Some(DocumentHistory::Rewindable),
        fork: Some(DocumentFork::AsOf),
    };
    commit_writes(
        storage,
        vec![
            StorageWrite::DocumentCopy {
                record: child_record(historical_id, child),
                source: DocumentCopySource {
                    id: source_id,
                    at: DocumentPoint::Seq(created_at),
                },
            },
            StorageWrite::DocumentCopy {
                record: child_record(current_id, second_child),
                source: DocumentCopySource {
                    id: source_id,
                    at: DocumentPoint::Current,
                },
            },
            StorageWrite::DocumentCopy {
                record: child_record(retired_id, root),
                source: DocumentCopySource {
                    id: source_id,
                    at: DocumentPoint::Current,
                },
            },
            StorageWrite::DocumentRetire { id: retired_id },
        ],
    )
    .unwrap();
    let historical = storage
        .document(historical_id, DocumentPoint::Current)
        .unwrap()
        .unwrap();
    assert_eq!(historical.version, 2);
    assert_eq!(
        historical.value,
        json!({ "count": 1, "rows": [{ "value": "base" }] })
    );
    let current = storage
        .document(current_id, DocumentPoint::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.version, 2);
    assert_eq!(
        current.value,
        json!({ "count": 2, "rows": [{ "value": "base" }, { "value": "current" }] })
    );
    assert!(storage
        .document(retired_id, DocumentPoint::Current)
        .unwrap()
        .is_none());
    commit_writes(
        storage,
        vec![
            StorageWrite::DocumentChange {
                id: source_id,
                content: base(2, json!({ "count": 99, "rows": [] })),
            },
            StorageWrite::DocumentRetire { id: source_id },
        ],
    )
    .unwrap();
    assert_eq!(
        storage
            .document(current_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 2, "rows": [{ "value": "base" }, { "value": "current" }] })
    );
    let latest_source_id = storage.mint_id().unwrap();
    let latest_copy_id = storage.mint_id().unwrap();
    let latest_source = DocumentCreate {
        id: latest_source_id,
        kind: "copy.latest".into(),
        key: None,
        scope: DocumentScope::Conversation {
            conversation_id: root,
        },
        history: Some(DocumentHistory::Latest),
        fork: Some(DocumentFork::Current),
    };
    commit_writes(
        storage,
        vec![doc_create(
            latest_source.clone(),
            base(4, json!({ "retained": "copy" })),
        )],
    )
    .unwrap();
    let mut latest_copy = latest_source.clone();
    latest_copy.id = latest_copy_id;
    latest_copy.scope = DocumentScope::Conversation {
        conversation_id: child,
    };
    commit_writes(
        storage,
        vec![StorageWrite::DocumentCopy {
            record: latest_copy,
            source: DocumentCopySource {
                id: latest_source_id,
                at: DocumentPoint::Current,
            },
        }],
    )
    .unwrap();
    commit_writes(
        storage,
        vec![
            StorageWrite::DocumentChange {
                id: latest_source_id,
                content: base(4, json!({ "retained": "source-only" })),
            },
            StorageWrite::DocumentRetire {
                id: latest_source_id,
            },
        ],
    )
    .unwrap();
    let copied = storage
        .document(latest_copy_id, DocumentPoint::Current)
        .unwrap()
        .unwrap();
    assert_eq!(copied.version, 4);
    assert_eq!(copied.value, json!({ "retained": "copy" }));
    let conflict_id = storage.mint_id().unwrap();
    let conflict = commit_writes(
        storage,
        vec![
            StorageWrite::DocumentCopy {
                record: child_record(conflict_id, child),
                source: DocumentCopySource {
                    id: current_id,
                    at: DocumentPoint::Current,
                },
            },
            StorageWrite::DocumentRetire { id: current_id },
        ],
    );
    assert!(conflict.is_err());
    assert!(storage
        .document(conflict_id, DocumentPoint::Current)
        .unwrap()
        .is_none());
    assert_eq!(
        storage
            .document(current_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 2, "rows": [{ "value": "base" }, { "value": "current" }] })
    );
    let mismatch_id = storage.mint_id().unwrap();
    let mut mismatch = child_record(mismatch_id, child);
    mismatch.kind = "copy.mismatch".into();
    let mismatch = commit_writes(
        storage,
        vec![StorageWrite::DocumentCopy {
            record: mismatch,
            source: DocumentCopySource {
                id: current_id,
                at: DocumentPoint::Current,
            },
        }],
    );
    assert!(mismatch.is_err());
    assert!(storage
        .document(mismatch_id, DocumentPoint::Current)
        .unwrap()
        .is_none());
}

fn version_transitions(storage: &dyn Storage) {
    create_root(storage);
    let id = storage.mint_id().unwrap();
    let record = DocumentCreate {
        id,
        kind: "session.settings".into(),
        key: None,
        scope: DocumentScope::Session,
        history: None,
        fork: None,
    };
    commit_writes(
        storage,
        vec![doc_create(record, base(1, json!({ "count": 1 })))],
    )
    .unwrap();
    commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id,
            content: delta(1, vec![op_set(vec![key("count")], json!(2))]),
        }],
    )
    .unwrap();
    let migrated_at = commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id,
            content: base(2, json!({ "count": 3 })),
        }],
    )
    .unwrap();
    let current = storage
        .document(id, DocumentPoint::Current)
        .unwrap()
        .unwrap();
    assert_eq!(current.version, 2);
    assert_eq!(current.value, json!({ "count": 3 }));
    assert_err(
        storage.document(id, DocumentPoint::Seq(migrated_at)),
        "does not retain historical content",
    );
    assert_err(
        commit_writes(
            storage,
            vec![StorageWrite::DocumentChange {
                id,
                content: delta(1, vec![op_set(vec![key("count")], json!(4))]),
            }],
        ),
        "version transition requires a base",
    );
    assert_eq!(
        storage
            .document(id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 3 })
    );
    commit_writes(storage, vec![StorageWrite::DocumentRetire { id }]).unwrap();
    assert!(storage
        .document(id, DocumentPoint::Current)
        .unwrap()
        .is_none());
}

fn logical_addresses(storage: &dyn Storage) {
    let root = create_root(storage);
    let first_id = storage.mint_id().unwrap();
    let second_id = storage.mint_id().unwrap();
    let conversation_id = storage.mint_id().unwrap();
    let task_id = storage.mint_id().unwrap();
    let task_singleton = storage.mint_id().unwrap();
    let task_family = storage.mint_id().unwrap();
    let task_other = storage.mint_id().unwrap();
    let created_at = commit_writes(
        storage,
        vec![
            StorageWrite::Task {
                value: pending(task_id, root),
            },
            doc_create(
                DocumentCreate {
                    id: first_id,
                    kind: "cache".into(),
                    key: Some("__proto__".into()),
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                base(1, json!({ "owner": "first" })),
            ),
            doc_create(
                DocumentCreate {
                    id: second_id,
                    kind: "cache".into(),
                    key: Some("constructor".into()),
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                base(1, json!({ "owner": "second" })),
            ),
            doc_create(
                DocumentCreate {
                    id: conversation_id,
                    kind: "cache".into(),
                    key: Some("__proto__".into()),
                    scope: DocumentScope::Conversation {
                        conversation_id: root,
                    },
                    history: Some(DocumentHistory::Latest),
                    fork: Some(DocumentFork::Current),
                },
                base(1, json!({ "owner": "conversation" })),
            ),
            doc_create(
                DocumentCreate {
                    id: task_singleton,
                    kind: "task.cache".into(),
                    key: None,
                    scope: DocumentScope::Task { task_id },
                    history: None,
                    fork: None,
                },
                base(1, json!({ "owner": "singleton" })),
            ),
            doc_create(
                DocumentCreate {
                    id: task_family,
                    kind: "task.cache".into(),
                    key: Some("member".into()),
                    scope: DocumentScope::Task { task_id },
                    history: None,
                    fork: None,
                },
                base(1, json!({ "owner": "family" })),
            ),
            doc_create(
                DocumentCreate {
                    id: task_other,
                    kind: "task.other".into(),
                    key: None,
                    scope: DocumentScope::Task { task_id },
                    history: None,
                    fork: None,
                },
                base(1, json!({ "owner": "other" })),
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: "cache".into(),
                    scope: DocumentScope::Session,
                    key: Some("__proto__".into())
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        first_id
    );
    let session = DocumentQuery {
        scope: DocumentScope::Session,
        at: DocumentPoint::Current,
        kind: None,
    };
    assert_eq!(
        storage
            .scan_documents(&session, 1, None)
            .unwrap()
            .items
            .len(),
        1
    );
    let first = storage.scan_documents(&session, 1, None).unwrap();
    let second = storage
        .scan_documents(&session, 1, first.next.as_ref())
        .unwrap();
    let mut both = first.items;
    both.extend(second.items);
    assert_eq!(
        both.into_iter().map(|record| record.id).collect::<Vec<_>>(),
        vec![first_id, second_id]
    );
    let conversation_scan = storage
        .scan_documents(
            &DocumentQuery {
                scope: DocumentScope::Conversation {
                    conversation_id: root,
                },
                at: DocumentPoint::Current,
                kind: None,
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(
        ids(&conversation_scan, |record| record.id),
        vec![conversation_id]
    );
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: "task.cache".into(),
                    scope: DocumentScope::Task { task_id },
                    key: None
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        task_singleton
    );
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: "task.cache".into(),
                    scope: DocumentScope::Task { task_id },
                    key: Some("member".into()),
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        task_family
    );
    let task_scan = storage
        .scan_documents(
            &DocumentQuery {
                scope: DocumentScope::Task { task_id },
                at: DocumentPoint::Current,
                kind: Some("task.cache".into()),
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(
        ids(&task_scan, |record| record.id),
        vec![task_singleton, task_family]
    );
    assert_err(
        storage.document(task_singleton, DocumentPoint::Seq(created_at)),
        "does not retain historical content",
    );
    let _ = task_other;
}

fn lifecycle_failures(storage: &dyn Storage) {
    let root = create_root(storage);
    let first_id = storage.mint_id().unwrap();
    let second_id = storage.mint_id().unwrap();
    let record = DocumentCreate {
        id: first_id,
        kind: "singleton".into(),
        key: None,
        scope: DocumentScope::Session,
        history: None,
        fork: None,
    };
    commit_writes(
        storage,
        vec![doc_create(record.clone(), base(1, json!({ "value": 1 })))],
    )
    .unwrap();
    let mut second = record.clone();
    second.id = second_id;
    assert_err(
        commit_writes(
            storage,
            vec![
                doc_create(second, base(1, json!({ "value": 2 }))),
                StorageWrite::DocumentChange {
                    id: first_id,
                    content: delta(1, Vec::new()),
                },
            ],
        ),
        "already has a current incarnation",
    );
    assert_eq!(
        storage
            .document(first_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "value": 1 })
    );
    assert!(storage
        .document(second_id, DocumentPoint::Current)
        .unwrap()
        .is_none());
    let empty_id = storage.mint_id().unwrap();
    let empty_at = commit_writes(
        storage,
        vec![
            doc_create(
                DocumentCreate {
                    id: empty_id,
                    kind: record.kind,
                    key: Some("empty".into()),
                    scope: DocumentScope::Conversation {
                        conversation_id: root,
                    },
                    history: Some(DocumentHistory::Rewindable),
                    fork: Some(DocumentFork::Initial),
                },
                base(1, json!({})),
            ),
            StorageWrite::DocumentRetire { id: empty_id },
        ],
    )
    .unwrap();
    assert!(storage
        .document(empty_id, DocumentPoint::Current)
        .unwrap()
        .is_none());
    assert!(storage
        .document(empty_id, DocumentPoint::Seq(empty_at))
        .unwrap()
        .is_none());
    assert!(storage
        .find_document(
            &DocumentAddress {
                kind: "singleton".into(),
                scope: DocumentScope::Conversation {
                    conversation_id: root
                },
                key: Some("empty".into()),
            },
            DocumentPoint::Seq(empty_at),
        )
        .unwrap()
        .is_none());
}

fn refuses_create_and_change(storage: &dyn Storage) {
    create_root(storage);
    for (order, key_name) in [
        ("create first", "create first"),
        ("change first", "change first"),
    ] {
        let id = storage.mint_id().unwrap();
        let create = doc_create(
            DocumentCreate {
                id,
                kind: "twice".into(),
                key: Some(key_name.into()),
                scope: DocumentScope::Session,
                history: None,
                fork: None,
            },
            base(1, json!({ "x": 1 })),
        );
        let change = StorageWrite::DocumentChange {
            id,
            content: delta(1, vec![op_set(vec![key("x")], json!(2))]),
        };
        let writes = if order == "create first" {
            vec![create, change]
        } else {
            vec![change, create]
        };
        assert_err(
            commit_writes(storage, writes),
            "more than one content command",
        );
        assert!(storage
            .document(id, DocumentPoint::Current)
            .unwrap()
            .is_none());
    }
}

fn rolls_back_document_command(storage: &dyn Storage) {
    let root = create_root(storage);
    let task_id = storage.mint_id().unwrap();
    let submission_id = storage.mint_id().unwrap();
    let document_id = storage.mint_id().unwrap();
    let task = pending(task_id, root);
    let submission = submission(
        submission_id,
        root,
        Some("atomic"),
        SubmissionType::Input,
        SubmissionStatus::Queued,
    );
    let record = DocumentCreate {
        id: document_id,
        kind: "atomic".into(),
        key: None,
        scope: DocumentScope::Session,
        history: None,
        fork: None,
    };
    let baseline = commit_writes(
        storage,
        vec![
            StorageWrite::Task {
                value: task.clone(),
            },
            StorageWrite::Submission {
                value: submission.clone(),
            },
            doc_create(record.clone(), base(1, json!({ "count": 1 }))),
        ],
    )
    .unwrap();
    let entry_id = storage.mint_id().unwrap();
    let conflicting = storage.mint_id().unwrap();
    let mut running = task.clone();
    running.state = TaskState::Running {
        checkpoint: json!({ "phase": "effect" }),
    };
    let mut failed = submission.clone();
    failed.status = SubmissionStatus::Unanswered;
    failed.reason = Some("failed".into());
    let mut conflict_record = record.clone();
    conflict_record.id = conflicting;
    assert_err(
        commit_writes(
            storage,
            vec![
                StorageWrite::Task { value: running },
                StorageWrite::Submission { value: failed },
                StorageWrite::Entry {
                    value: entry(entry_id, root, "transient"),
                },
                doc_create(conflict_record, base(1, json!({ "count": 2 }))),
            ],
        ),
        "already has a current incarnation",
    );
    assert_eq!(storage.task(task_id).unwrap(), Some(task.clone()));
    let pending_page = storage
        .scan_tasks(
            &TaskQuery {
                status: Some(TaskStatus::Pending),
                ..TaskQuery::default()
            },
            10,
            None,
        )
        .unwrap();
    assert_eq!(pending_page.items, vec![task]);
    assert_eq!(
        storage.submission_by_request(root, "atomic").unwrap(),
        Some(submission)
    );
    assert!(storage.entry(entry_id).unwrap().is_none());
    assert!(storage
        .document(conflicting, DocumentPoint::Current)
        .unwrap()
        .is_none());
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: record.kind,
                    scope: record.scope,
                    key: None
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        document_id
    );
    let after = commit_writes(
        storage,
        vec![StorageWrite::DocumentChange {
            id: document_id,
            content: delta(1, vec![op_set(vec![key("count")], json!(3))]),
        }],
    )
    .unwrap();
    assert!(after > baseline);
}

fn lossless_strings(storage: &dyn Storage) {
    // Rust strings cannot hold lone surrogates. These scalars still round-trip through JSON text.
    let first = "\u{D7FF}\"\\\u{0001}";
    let second = "\u{E000}\u{2028}";
    let root = create_root(storage);
    let first_task = storage.mint_id().unwrap();
    let second_task = storage.mint_id().unwrap();
    let first_submission = storage.mint_id().unwrap();
    let second_submission = storage.mint_id().unwrap();
    let first_kind = storage.mint_id().unwrap();
    let second_kind = storage.mint_id().unwrap();
    let first_key = storage.mint_id().unwrap();
    let second_key = storage.mint_id().unwrap();
    let mut task_a = pending(first_task, root);
    task_a.kind = first.into();
    let mut task_b = pending(second_task, root);
    task_b.kind = second.into();
    commit_writes(
        storage,
        vec![
            StorageWrite::Task { value: task_a },
            StorageWrite::Task { value: task_b },
            StorageWrite::Submission {
                value: submission(
                    first_submission,
                    root,
                    Some(first),
                    SubmissionType::Input,
                    SubmissionStatus::Queued,
                ),
            },
            StorageWrite::Submission {
                value: submission(
                    second_submission,
                    root,
                    Some(second),
                    SubmissionType::Input,
                    SubmissionStatus::Queued,
                ),
            },
            doc_create(
                DocumentCreate {
                    id: first_kind,
                    kind: first.into(),
                    key: None,
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                base(1, json!({ "identity": "first kind" })),
            ),
            doc_create(
                DocumentCreate {
                    id: second_kind,
                    kind: second.into(),
                    key: None,
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                base(1, json!({ "identity": "second kind" })),
            ),
            doc_create(
                DocumentCreate {
                    id: first_key,
                    kind: "family".into(),
                    key: Some(first.into()),
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                base(1, json!({ "identity": "first key" })),
            ),
            doc_create(
                DocumentCreate {
                    id: second_key,
                    kind: "family".into(),
                    key: Some(second.into()),
                    scope: DocumentScope::Session,
                    history: None,
                    fork: None,
                },
                base(1, json!({ "identity": "second key" })),
            ),
        ],
    )
    .unwrap();
    assert_eq!(
        ids(
            &storage
                .scan_tasks(
                    &TaskQuery {
                        kind: Some(first.into()),
                        ..TaskQuery::default()
                    },
                    10,
                    None
                )
                .unwrap(),
            |record| record.id
        ),
        vec![first_task]
    );
    assert_eq!(
        ids(
            &storage
                .scan_tasks(
                    &TaskQuery {
                        kind: Some(second.into()),
                        ..TaskQuery::default()
                    },
                    10,
                    None
                )
                .unwrap(),
            |record| record.id
        ),
        vec![second_task]
    );
    assert_eq!(storage.task(first_task).unwrap().unwrap().kind, first);
    assert_eq!(storage.task(second_task).unwrap().unwrap().kind, second);
    assert_eq!(
        storage
            .submission_by_request(root, first)
            .unwrap()
            .unwrap()
            .request_id
            .as_deref(),
        Some(first)
    );
    assert_eq!(
        storage
            .submission_by_request(root, first)
            .unwrap()
            .unwrap()
            .id,
        first_submission
    );
    assert_eq!(
        storage
            .submission_by_request(root, second)
            .unwrap()
            .unwrap()
            .id,
        second_submission
    );
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: first.into(),
                    scope: DocumentScope::Session,
                    key: None
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        first_kind
    );
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: second.into(),
                    scope: DocumentScope::Session,
                    key: None
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        second_kind
    );
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: "family".into(),
                    scope: DocumentScope::Session,
                    key: Some(first.into())
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        first_key
    );
    assert_eq!(
        storage
            .find_document(
                &DocumentAddress {
                    kind: "family".into(),
                    scope: DocumentScope::Session,
                    key: Some(second.into())
                },
                DocumentPoint::Current,
            )
            .unwrap()
            .unwrap()
            .id,
        second_key
    );
    assert_eq!(
        ids(
            &storage
                .scan_documents(
                    &DocumentQuery {
                        scope: DocumentScope::Session,
                        at: DocumentPoint::Current,
                        kind: Some(first.into())
                    },
                    10,
                    None,
                )
                .unwrap(),
            |record| record.id
        ),
        vec![first_kind]
    );
}

fn global_namespace(storage: &dyn Storage) {
    let root = create_root(storage);
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(100, root, "message"),
        }],
    )
    .unwrap();
    assert_eq!(storage.mint_id().unwrap(), 101);
    assert_err(
        commit_writes(
            storage,
            vec![StorageWrite::Task {
                value: pending(100, root),
            }],
        ),
        "ID 100 already belongs to entry",
    );
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(MAX_SAFE_INTEGER, root, "last-id"),
        }],
    )
    .unwrap();
    assert_err(storage.mint_id(), "ID space is exhausted");
    assert_err(storage.mint_id(), "ID space is exhausted");
}

fn commits_batches_in_order(storage: &dyn Storage) {
    let root = create_root(storage);
    let first_id = storage.mint_id().unwrap();
    let last_id = storage.mint_id().unwrap();
    let document_id = storage.mint_id().unwrap();
    let mut receipts = Vec::new();
    let mut read = None;
    let result = storage
        .commit(
            &[
                vec![
                    StorageWrite::Entry {
                        value: entry(first_id, root, "message"),
                    },
                    doc_create(
                        DocumentCreate {
                            id: document_id,
                            kind: "batch.doc".into(),
                            key: None,
                            scope: DocumentScope::Conversation {
                                conversation_id: root,
                            },
                            history: Some(DocumentHistory::Latest),
                            fork: Some(DocumentFork::Initial),
                        },
                        base(1, json!({ "count": 1 })),
                    ),
                ],
                vec![StorageWrite::DocumentChange {
                    id: document_id,
                    content: delta(1, vec![op_set(vec![key("count")], json!(2))]),
                }],
                vec![StorageWrite::Entry {
                    value: entry(last_id, root, "message"),
                }],
            ],
            &mut |index, seq| {
                receipts.push((index, seq));
                if read.is_none() {
                    read = Some(storage.entry(last_id));
                }
            },
        )
        .unwrap();
    assert_eq!(
        result,
        CommitResult {
            stored: 3,
            refused: None
        }
    );
    assert_eq!(
        receipts.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    let first = receipts[0].1;
    let second = receipts[1].1;
    let last = receipts[2].1;
    assert!(second > first);
    assert!(last > second);
    assert_eq!(read.unwrap().unwrap().unwrap().commit_seq, last);
    assert_eq!(storage.entry(first_id).unwrap().unwrap().commit_seq, first);
    assert_eq!(
        storage
            .document(document_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 2 })
    );
    let next = commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(storage.mint_id().unwrap(), root, "message"),
        }],
    )
    .unwrap();
    assert!(next > last);
}

fn stores_batches_before_refusal(storage: &dyn Storage) {
    let root = create_root(storage);
    let first_id = storage.mint_id().unwrap();
    let after_id = storage.mint_id().unwrap();
    let minted_task = storage.mint_id().unwrap();
    let mut receipts = Vec::new();
    let result = storage
        .commit(
            &[
                vec![StorageWrite::Entry {
                    value: entry(first_id, root, "message"),
                }],
                vec![
                    StorageWrite::Task {
                        value: pending(minted_task, root),
                    },
                    StorageWrite::Entry {
                        value: entry(first_id, root, "message"),
                    },
                ],
                vec![StorageWrite::Entry {
                    value: entry(after_id, root, "message"),
                }],
            ],
            &mut |index, _seq| receipts.push(index),
        )
        .unwrap();
    assert_eq!(result.stored, 1);
    assert_eq!(
        result.refused.as_deref(),
        Some(format!("Error: ID {first_id} already belongs to entry").as_str())
    );
    assert_eq!(receipts, vec![0]);
    assert!(storage.entry(first_id).unwrap().is_some());
    assert!(storage.entry(after_id).unwrap().is_none());
    assert!(storage
        .scan_tasks(&TaskQuery::default(), 10, None)
        .unwrap()
        .items
        .is_empty());
    let refused = storage
        .commit(
            &[vec![StorageWrite::Conversation {
                value: conversation(root),
            }]],
            &mut |index, _seq| receipts.push(index),
        )
        .unwrap();
    assert_eq!(refused.stored, 0);
    assert_eq!(
        refused.refused.as_deref(),
        Some(format!("Error: ID {root} already belongs to conversation").as_str())
    );
    assert_eq!(receipts, vec![0]);
    commit_writes(
        storage,
        vec![StorageWrite::Entry {
            value: entry(after_id, root, "message"),
        }],
    )
    .unwrap();
    assert!(storage.entry(after_id).unwrap().is_some());
}

fn refuses_malformed_later_batch(storage: &dyn Storage) {
    let root = create_root(storage);
    let document_id = storage.mint_id().unwrap();
    commit_writes(
        storage,
        vec![doc_create(
            DocumentCreate {
                id: document_id,
                kind: "staged".into(),
                key: None,
                scope: DocumentScope::Session,
                history: None,
                fork: None,
            },
            base(1, json!({ "n": 1 })),
        )],
    )
    .unwrap();
    let first_id = storage.mint_id().unwrap();
    let second_id = storage.mint_id().unwrap();
    let after_id = storage.mint_id().unwrap();
    let mut receipts = Vec::new();
    let result = storage
        .commit(
            &[
                vec![StorageWrite::Entry {
                    value: entry(first_id, root, "message"),
                }],
                vec![
                    StorageWrite::Entry {
                        value: entry(second_id, root, "message"),
                    },
                    StorageWrite::DocumentRetire { id: document_id },
                    StorageWrite::DocumentRetire { id: document_id },
                ],
                vec![StorageWrite::Entry {
                    value: entry(after_id, root, "message"),
                }],
            ],
            &mut |index, _seq| receipts.push(index),
        )
        .unwrap();
    assert_eq!(result.stored, 1);
    assert!(result.refused.is_some());
    assert_eq!(receipts, vec![0]);
    assert!(storage.entry(first_id).unwrap().is_some());
    assert!(storage.entry(second_id).unwrap().is_none());
    assert!(storage.entry(after_id).unwrap().is_none());
    assert_eq!(
        storage
            .document(document_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "n": 1 })
    );
}

fn resolves_batched_copy(storage: &dyn Storage) {
    let root = create_root(storage);
    let child = storage.mint_id().unwrap();
    let source_id = storage.mint_id().unwrap();
    let copy_id = storage.mint_id().unwrap();
    let record = |id, conversation_id| DocumentCreate {
        id,
        kind: "batch.copy".into(),
        key: None,
        scope: DocumentScope::Conversation { conversation_id },
        history: Some(DocumentHistory::Rewindable),
        fork: Some(DocumentFork::AsOf),
    };
    let mut seqs = Vec::new();
    let result = storage
        .commit(
            &[
                vec![doc_create(
                    record(source_id, root),
                    base(1, json!({ "count": 1 })),
                )],
                vec![StorageWrite::DocumentChange {
                    id: source_id,
                    content: delta(1, vec![op_set(vec![key("count")], json!(2))]),
                }],
                vec![
                    StorageWrite::Conversation {
                        value: conversation(child),
                    },
                    StorageWrite::DocumentCopy {
                        record: record(copy_id, child),
                        source: DocumentCopySource {
                            id: source_id,
                            at: DocumentPoint::Current,
                        },
                    },
                ],
            ],
            &mut |_index, seq| seqs.push(seq),
        )
        .unwrap();
    assert_eq!(
        result,
        CommitResult {
            stored: 3,
            refused: None
        }
    );
    assert_eq!(
        storage
            .document(copy_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 2 })
    );
    assert_eq!(
        storage
            .document(source_id, DocumentPoint::Seq(seqs[0]))
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 1 })
    );
    let later_id = storage.mint_id().unwrap();
    let later_copy = storage.mint_id().unwrap();
    let refused = storage
        .commit(
            &[vec![
                StorageWrite::Conversation {
                    value: conversation(later_id),
                },
                StorageWrite::DocumentCopy {
                    record: record(later_copy, later_id),
                    source: DocumentCopySource {
                        id: source_id,
                        at: DocumentPoint::Current,
                    },
                },
                StorageWrite::DocumentChange {
                    id: source_id,
                    content: delta(1, vec![op_set(vec![key("count")], json!(3))]),
                },
            ]],
            &mut |_index, _seq| {},
        )
        .unwrap();
    assert_eq!(refused.stored, 0);
    assert!(refused.refused.is_some());
    assert!(storage.conversation(later_id).unwrap().is_none());
    assert_eq!(
        storage
            .document(source_id, DocumentPoint::Current)
            .unwrap()
            .unwrap()
            .value,
        json!({ "count": 2 })
    );
}

fn rejects_after_close(storage: &dyn Storage) {
    create_root(storage);
    storage.close().unwrap();
    assert_err(storage.conversation(ROOT_CONVERSATION_ID), "closed");
    assert_err(
        commit_writes(
            storage,
            vec![StorageWrite::Conversation {
                value: conversation(ROOT_CONVERSATION_ID),
            }],
        ),
        "closed",
    );
    assert_err(storage.mint_id(), "closed");
}

macro_rules! both {
    ($test:ident, $case:ident, $label:literal) => {
        #[test]
        fn $test() {
            run_both($label, $case);
        }
    };
}

both!(
    reserves_id_1_for_the_immutable_root_conversation,
    reserves_id_1,
    "reserves ID 1 for the immutable root conversation"
);
both!(
    receives_a_durable_commit_once_before_resolving,
    receives_durable_commit,
    "receives a durable commit once before resolving, and never a refused one"
);
both!(
    commits_mixed_table_writes_atomically,
    commits_mixed_atomically,
    "commits mixed table writes atomically and rolls all of them back on failure"
);
both!(
    detaches_retained_writes_and_every_returned_record,
    detaches_retained_writes,
    "detaches retained writes and every returned record"
);
both!(
    detaches_prototype_like_json_keys,
    detaches_prototype_keys,
    "detaches prototype-like JSON keys without changing object prototypes"
);
both!(
    indexes_entries_committed_out_of_id_order,
    indexes_entries_out_of_order,
    "indexes entries committed out of ID order"
);
both!(
    continues_an_entry_cursor_below_its_last_item,
    continues_entry_cursor,
    "continues an entry cursor below its last item after a newer commit"
);
both!(
    paginates_conversations_by_opaque_cursor,
    paginates_conversations,
    "paginates conversations by opaque cursor in ascending ID order"
);
both!(
    filters_and_pages_conversations_by_owner,
    filters_conversations_by_owner,
    "filters and pages conversations by durable owner edges"
);
both!(
    scans_deep_fork_history_newest_first,
    scans_deep_fork,
    "scans deep fork history newest-first through every ancestor cap"
);
both!(
    scans_tables_in_either_id_order,
    scans_either_order,
    "scans tables in either ID order and continues a cursor in its order"
);
both!(
    replaces_complete_task_records,
    replaces_tasks,
    "replaces complete task records and pages filtered task scans"
);
both!(
    stores_owners_and_scans_waiting_and_completing,
    waiting_and_completing,
    "stores owners and scans waiting and completing tasks by status"
);
both!(
    stores_and_scans_sleeping_tasks,
    sleeping_tasks,
    "stores and scans sleeping tasks by status"
);
both!(
    indexes_request_ids_per_conversation,
    indexes_request_ids,
    "indexes request IDs per conversation and replaces complete submission records"
);
both!(
    stores_passive_write_submissions,
    passive_writes,
    "stores passive write submissions without input-only lifecycle states"
);
both!(
    stores_continue_submissions,
    continue_submissions,
    "stores continue submissions, which have no entry and are never queued"
);
both!(
    reconstructs_rewindable_documents,
    rewindable_documents,
    "reconstructs rewindable documents and preserves half-open incarnations"
);
both!(
    streams_long_document_tails,
    streams_long_tails,
    "streams long document tails across root replacement deltas"
);
both!(
    copies_stored_document_bases,
    copies_bases,
    "copies stored document bases independently and rejects ambiguous sources"
);
both!(
    uses_bases_for_version_transitions,
    version_transitions,
    "uses bases for version transitions and rejects historical reads of current-only documents"
);
both!(
    indexes_logical_addresses,
    logical_addresses,
    "indexes logical addresses and exact-scope scans independently"
);
both!(
    keeps_document_lifecycle_failures_atomic,
    lifecycle_failures,
    "keeps document lifecycle failures atomic and gives create-plus-retire an empty lifetime"
);
both!(
    refuses_creation_and_change_together,
    refuses_create_and_change,
    "refuses a creation and a change of one document in one commit, in either order"
);
both!(
    rolls_back_when_a_document_command_fails,
    rolls_back_document_command,
    "rolls back record tables and secondary indexes when a document command fails"
);
both!(
    keeps_indexed_string_identities_lossless,
    lossless_strings,
    "keeps indexed string identities lossless"
);
both!(
    keeps_one_global_record_id_namespace,
    global_namespace,
    "keeps one global record ID namespace and rejects exhausted ID minting"
);
both!(
    commits_batches_in_order_each_with_its_seq,
    commits_batches_in_order,
    "commits batches in order, each with its seq, checked against the earlier ones"
);
both!(
    stores_the_batches_before_the_first_refusal,
    stores_batches_before_refusal,
    "stores the batches before the first one refused, and none after it"
);
both!(
    refuses_a_later_batch_its_own_malformed_actions,
    refuses_malformed_later_batch,
    "refuses a later batch its own malformed actions fail, keeping the batches before it"
);
both!(
    resolves_a_batched_copy_against_earlier_batches,
    resolves_batched_copy,
    "resolves a batched copy against the state the earlier batches left"
);
both!(
    rejects_every_operation_after_close,
    rejects_after_close,
    "rejects every operation after close"
);

#[test]
fn sqlite_reopen_reads_the_committed_entry() {
    let temp = TempDb::new();
    let entry_id;
    let seq;
    {
        let storage = SqliteStorage::open(&temp.path).unwrap();
        commit_writes(
            &storage,
            vec![StorageWrite::Conversation {
                value: conversation(ROOT_CONVERSATION_ID),
            }],
        )
        .unwrap();
        entry_id = storage.mint_id().unwrap();
        seq = commit_writes(
            &storage,
            vec![StorageWrite::Entry {
                value: with_data(
                    entry(entry_id, ROOT_CONVERSATION_ID, "message"),
                    json!({ "text": "kept" }),
                ),
            }],
        )
        .unwrap();
        storage.close().unwrap();
    }
    let storage = SqliteStorage::open(&temp.path).unwrap();
    let hit = storage.entry(entry_id).unwrap().expect("reopened entry");
    assert_eq!(hit.commit_seq, seq);
    assert_eq!(hit.entry.data, Some(json!({ "text": "kept" })));
    assert!(storage.mint_id().unwrap() > entry_id);
    assert_eq!(
        storage
            .conversation(ROOT_CONVERSATION_ID)
            .unwrap()
            .unwrap()
            .id,
        ROOT_CONVERSATION_ID
    );
}
