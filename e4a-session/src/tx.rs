// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/transaction.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use e4a_types::{
    apply, ConversationOwner, ConversationParent, ConversationRecord,
    DocumentAddress, DocumentContent, DocumentCreate, DocumentPoint, DocumentRecord, DocumentScope,
    EntryQuery, EntryRecord, Json, Op, ScanOrder, StorageWrite, SubmissionRecord, SubmissionStatus,
    SubmissionType, TaskRecord, TaskState, ROOT_CONVERSATION_ID,
};

use crate::docs::{address_id, agent_doc, provider_doc, DocDef, DocRef};
use crate::forks::{copy_write, prepare_fork_document_copies, ForkDocumentCopy};
use crate::keys::{
    append_key, conversation_key, document_key, entry_key, incarnation_key, request_key, scope_key,
    submission_key, table_keys, task_key, ALL_KEYS,
};
use crate::store::StoreHandle;
use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationOwnership {
    Ownerless,
    Task { task_id: i64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntryDraft {
    pub kind: String,
    pub model: Option<Json>,
    pub data: Option<Json>,
    pub head: Option<i64>,
    pub edits: Option<Vec<e4a_types::ContextEdit>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskCreate {
    pub kind: String,
    pub version: i64,
    pub conversation_id: i64,
    pub input: Json,
    pub owner: Option<i64>,
    pub background: bool,
    pub checkpoint: Json,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubmissionCreate {
    pub conversation_id: i64,
    pub request_id: Option<String>,
    pub ty: SubmissionType,
    pub status: SubmissionStatus,
    pub entry: Option<i64>,
    pub answer: Option<i64>,
    pub reason: Option<String>,
    pub detail: Option<Json>,
    pub trace: Option<Json>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SubmissionSettlement {
    Done { answer: i64 },
    Unanswered { reason: String, detail: Option<Json> },
}

enum SubmissionChange {
    Place { entry: i64 },
    Settle(SubmissionSettlement),
}

enum TaskWriteKind {
    Create,
    Replace,
}

struct TaskWrite {
    kind: TaskWriteKind,
    record: TaskRecord,
}

enum DocTarget {
    Created {
        record: DocumentCreate,
    },
    ForkCopy {
        record: DocumentCreate,
        source: e4a_types::DocumentCopySource,
        version: i64,
    },
    Loaded {
        record: DocumentRecord,
        deltas_since_base: usize,
        stored_version: i64,
    },
}

struct DocEntry {
    address_id: String,
    address: DocumentAddress,
    def: DocDef,
    target: DocTarget,
    ops: Vec<Op>,
    value: Json,
    retire: bool,
}

pub(crate) struct AdoptedDocument {
    pub address_id: String,
    pub record: DocumentRecord,
    pub value: Json,
}

pub(crate) struct Adopted {
    pub writes: Vec<StorageWrite>,
    pub conversations: Vec<ConversationRecord>,
    pub entries: Vec<EntryRecord>,
    pub documents: Vec<AdoptedDocument>,
    pub retired: Vec<String>,
    pub keys: Vec<String>,
}

pub(crate) struct TransactionScope {
    pub task_id: Option<i64>,
}

pub struct Tx {
    store: StoreHandle,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    scope: TransactionScope,
    sealed: bool,
    has_table_write: bool,
    writes: Vec<StorageWrite>,
    created_conversation_ids: HashSet<i64>,
    fork_source_conversation_ids: HashSet<i64>,
    fork_source_document_ids: HashSet<i64>,
    tasks: HashMap<i64, TaskWrite>,
    submissions: HashMap<i64, SubmissionRecord>,
    submission_changes: Vec<(i64, SubmissionChange)>,
    documents: Vec<DocEntry>,
    latest_document: HashMap<String, usize>,
}

impl Tx {
    pub(crate) fn new(
        store: StoreHandle,
        now: Arc<dyn Fn() -> i64 + Send + Sync>,
        scope: TransactionScope,
    ) -> Self {
        Self {
            store,
            now,
            scope,
            sealed: false,
            has_table_write: false,
            writes: Vec::new(),
            created_conversation_ids: HashSet::new(),
            fork_source_conversation_ids: HashSet::new(),
            fork_source_document_ids: HashSet::new(),
            tasks: HashMap::new(),
            submissions: HashMap::new(),
            submission_changes: Vec::new(),
            documents: Vec::new(),
            latest_document: HashMap::new(),
        }
    }

    pub fn conversation(&mut self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        self.assert_readable("conversation")?;
        self.store.touch(&conversation_key(id))?;
        self.store
            .call(move |storage| storage.conversation(id).map_err(Error::from))
    }

    pub fn entry(&mut self, id: i64) -> Result<Option<EntryRecord>, Error> {
        self.assert_readable("entry")?;
        self.store.touch(&entry_key(id))?;
        self.store.call(move |storage| {
            Ok(storage.entry(id).map_err(Error::from)?.map(|hit| hit.entry))
        })
    }

    pub fn task(&mut self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.assert_readable("task")?;
        self.committed_task(id)
    }

    pub fn submission(&mut self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        self.assert_readable("submission")?;
        self.store.touch(&submission_key(id))?;
        self.store
            .call(move |storage| storage.submission(id).map_err(Error::from))
    }

    pub fn submission_by_request(
        &mut self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        self.assert_readable("submissionByRequest")?;
        self.store
            .touch(&request_key(conversation_id, request_id))?;
        let request_id = request_id.to_string();
        self.store.call(move |storage| {
            storage
                .submission_by_request(conversation_id, &request_id)
                .map_err(Error::from)
        })
    }

    pub fn scan_entries(
        &mut self,
        query: &EntryQuery,
        limit: usize,
    ) -> Result<Vec<EntryRecord>, Error> {
        self.assert_readable("scanEntries")?;
        self.store.touch(ALL_KEYS)?;
        let query = query.clone();
        self.store.call(move |storage| {
            storage
                .scan_entries(&query, limit, None)
                .map(|page| page.items)
                .map_err(Error::from)
        })
    }

    pub fn latest_entry_id(&mut self, conversation_id: i64) -> Result<Option<i64>, Error> {
        self.assert_readable("latestEntryId")?;
        self.store.touch(&append_key(conversation_id))?;
        let query = EntryQuery {
            conversation_id,
            min_entry_id: None,
            max_entry_id: None,
            order: Some(ScanOrder::Descending),
        };
        self.store.call(move |storage| {
            Ok(storage
                .scan_entries(&query, 1, None)
                .map_err(Error::from)?
                .items
                .into_iter()
                .next()
                .map(|entry| entry.id))
        })
    }

    pub fn create_conversation(
        &mut self,
        ownership: ConversationOwnership,
    ) -> Result<ConversationRecord, Error> {
        self.stage_conversation(None, ownership, None)
    }

    pub(crate) fn create_root_conversation(&mut self) -> Result<ConversationRecord, Error> {
        self.stage_conversation(None, ConversationOwnership::Ownerless, Some(ROOT_CONVERSATION_ID))
    }

    pub fn fork_conversation(
        &mut self,
        parent_conversation_id: i64,
        at: i64,
        ownership: ConversationOwnership,
    ) -> Result<ConversationRecord, Error> {
        self.stage_conversation(
            Some(ConversationParent {
                conversation_id: parent_conversation_id,
                at,
            }),
            ownership,
            None,
        )
    }

    pub fn append_entry(
        &mut self,
        conversation_id: i64,
        draft: EntryDraft,
    ) -> Result<EntryRecord, Error> {
        self.assert_open()?;
        self.require_conversation(conversation_id)?;
        let id = self.mint()?;
        let record = EntryRecord {
            id,
            conversation_id,
            kind: draft.kind,
            model: draft.model,
            data: draft.data,
            head: draft.head,
            edits: draft.edits,
            by_task_id: self.scope.task_id,
        };
        self.has_table_write = true;
        self.writes.push(StorageWrite::Entry {
            value: record.clone(),
        });
        Ok(record)
    }

    pub fn create_task(&mut self, create: TaskCreate) -> Result<TaskRecord, Error> {
        self.assert_open()?;
        if let Some(owner_id) = create.owner {
            let owner = self.current_task(owner_id)?.ok_or_else(|| {
                Error::message(format!("Task owner {owner_id} does not exist"))
            })?;
            if create.background {
                return Err(Error::message("A child task cannot be background"));
            }
            if owner.conversation_id != create.conversation_id {
                return Err(Error::message(format!(
                    "A child task lives in its owner's conversation {}",
                    owner.conversation_id
                )));
            }
        }
        self.require_conversation(create.conversation_id)?;
        let id = self.mint()?;
        let record = TaskRecord {
            trace: None,
            links: None,
            id,
            conversation_id: create.conversation_id,
            kind: create.kind,
            version: create.version,
            input: create.input,
            owner: create.owner,
            background: create.background,
            abort_requested: false,
            abort_reason: None,
            abandon_on_restart: None,
            started_at: None,
            ended_at: None,
            restarts: None,
            state: TaskState::Pending {
                checkpoint: create.checkpoint,
            },
            memos: None,
        };
        self.has_table_write = true;
        self.tasks.insert(
            id,
            TaskWrite {
                kind: TaskWriteKind::Create,
                record: record.clone(),
            },
        );
        Ok(record)
    }

    pub fn set_task(&mut self, value: TaskRecord) -> Result<(), Error> {
        self.assert_open()?;
        self.has_table_write = true;
        let candidate = self.tasks.get(&value.id).map(|write| write.record.clone());
        if matches!(
            candidate.as_ref().map(|record| &record.state),
            Some(TaskState::Terminal { .. })
        ) {
            return Err(Error::message(format!(
                "Task {} already has a terminal candidate",
                value.id
            )));
        }
        if let Some(candidate) = &candidate {
            if candidate.conversation_id != value.conversation_id {
                return Err(Error::message(format!(
                    "Task {} cannot change conversations",
                    value.id
                )));
            }
        }
        let kind = if matches!(
            self.tasks.get(&value.id).map(|write| &write.kind),
            Some(TaskWriteKind::Create)
        ) {
            TaskWriteKind::Create
        } else {
            TaskWriteKind::Replace
        };
        let record = self.stamp_times(value, candidate.as_ref());
        self.tasks.insert(
            record.id,
            TaskWrite {
                kind,
                record,
            },
        );
        Ok(())
    }

    pub fn create_submission(
        &mut self,
        create: SubmissionCreate,
    ) -> Result<SubmissionRecord, Error> {
        self.assert_open()?;
        self.require_conversation(create.conversation_id)?;
        let id = self.mint()?;
        let record = SubmissionRecord {
            id,
            conversation_id: create.conversation_id,
            request_id: create.request_id,
            trace: create.trace,
            ty: create.ty,
            status: create.status,
            entry: create.entry,
            answer: create.answer,
            reason: create.reason,
            detail: create.detail,
        };
        self.has_table_write = true;
        self.submissions.insert(id, record.clone());
        Ok(record)
    }

    pub fn settle_submission(&mut self, id: i64, settlement: SubmissionSettlement) -> Result<(), Error> {
        self.assert_open()?;
        self.has_table_write = true;
        self.submission_changes
            .push((id, SubmissionChange::Settle(settlement)));
        Ok(())
    }

    pub fn place_submission(&mut self, id: i64, entry: i64) -> Result<(), Error> {
        self.assert_open()?;
        self.has_table_write = true;
        self.submission_changes
            .push((id, SubmissionChange::Place { entry }));
        Ok(())
    }

    pub fn doc(&mut self, def: &DocDef, owner: Option<i64>) -> Result<DocRef, Error> {
        self.assert_open()?;
        let address = def.address(owner)?;
        let address_id = address_id(&address);
        if let Some(&index) = self.latest_document.get(&address_id) {
            if !self.documents[index].retire {
                return Ok(DocRef { index });
            }
        }
        self.store.touch(&document_key(&address_id))?;
        let loaded = self.store.call({
            let address = address.clone();
            move |storage| {
                storage
                    .find_document(&address, DocumentPoint::Current)
                    .map_err(Error::from)
            }
        })?;
        if let Some(record) = loaded {
            let stored = self.store.call({
                let id = record.id;
                move |storage| {
                    storage
                        .document(id, DocumentPoint::Current)
                        .map_err(Error::from)
                }
            })?;
            let Some(stored) = stored else {
                return Err(Error::message(format!(
                    "Document {} ({}) cannot be read",
                    record.id, record.kind
                )));
            };
            return Ok(self.push_document(DocEntry {
                address_id,
                address,
                def: def.clone(),
                target: DocTarget::Loaded {
                    record: stored.record,
                    deltas_since_base: stored.deltas_since_base,
                    stored_version: stored.version,
                },
                ops: Vec::new(),
                value: stored.value,
                retire: false,
            }));
        }
        match address.scope {
            DocumentScope::Conversation { conversation_id } => {
                self.require_conversation(conversation_id)?;
            }
            DocumentScope::Task { task_id } => {
                let task = self.current_task(task_id)?.ok_or_else(|| {
                    Error::message(format!("Task {task_id} does not exist"))
                })?;
                if matches!(task.state, TaskState::Terminal { .. }) {
                    return Err(Error::message(format!("Task {task_id} is terminal")));
                }
            }
            DocumentScope::Session => {}
        }
        let id = self.mint()?;
        Ok(self.push_document(DocEntry {
            address_id,
            address: address.clone(),
            def: def.clone(),
            target: DocTarget::Created {
                record: def.create_record(id, &address),
            },
            ops: Vec::new(),
            value: def.initial(),
            retire: false,
        }))
    }

    pub fn apply_ops(&mut self, doc: DocRef, ops: &[Op]) -> Result<Json, Error> {
        self.assert_open()?;
        let entry = self
            .documents
            .get_mut(doc.index)
            .ok_or_else(|| Error::message("Document handle is not from this transaction"))?;
        if ops.is_empty() {
            return Ok(entry.value.clone());
        }
        let value = apply(&entry.value, ops)?;
        entry.value = value.clone();
        entry.ops.extend(ops.iter().cloned());
        if let DocTarget::ForkCopy { record, .. } = &entry.target {
            let record = record.clone();
            entry.target = DocTarget::Created { record };
        }
        Ok(value)
    }

    pub fn doc_value(&self, doc: DocRef) -> Result<&Json, Error> {
        self.documents
            .get(doc.index)
            .map(|entry| &entry.value)
            .ok_or_else(|| Error::message("Document handle is not from this transaction"))
    }

    pub fn retire_doc(&mut self, def: &DocDef, owner: Option<i64>) -> Result<(), Error> {
        self.assert_open()?;
        let address = def.address(owner)?;
        let address_id = address_id(&address);
        if let Some(&index) = self.latest_document.get(&address_id) {
            self.documents[index].retire = true;
            return Ok(());
        }
        let handle = self.doc(def, owner)?;
        self.documents[handle.index].retire = true;
        Ok(())
    }

    pub(crate) fn settle(&mut self) -> Result<Adopted, Error> {
        self.sealed = true;
        self.reject_fork_source_writes()?;
        self.validate_owners()?;
        self.apply_submission_changes()?;
        let mut writes = self.writes.clone();
        let conversations = writes
            .iter()
            .filter_map(|write| match write {
                StorageWrite::Conversation { value } => Some(value.clone()),
                _ => None,
            })
            .collect();
        let entries = writes
            .iter()
            .filter_map(|write| match write {
                StorageWrite::Entry { value } => Some(value.clone()),
                _ => None,
            })
            .collect();
        for record in self.submissions.values() {
            writes.push(StorageWrite::Submission {
                value: record.clone(),
            });
        }
        for write in self.tasks.values() {
            writes.push(StorageWrite::Task {
                value: write.record.clone(),
            });
        }
        let mut documents = Vec::new();
        let mut retired = Vec::new();
        for entry in &self.documents {
            match assemble_document(entry)? {
                Some(Assembled {
                    write,
                    document,
                    retire,
                }) => {
                    if let Some(write) = write {
                        writes.push(write);
                    }
                    if let Some(document) = document {
                        documents.push(document);
                    }
                    if retire {
                        let id = document_id(&entry.target);
                        writes.push(StorageWrite::DocumentRetire { id });
                        retired.push(entry.address_id.clone());
                    }
                }
                None => {}
            }
        }
        if writes.is_empty() {
            return Ok(Adopted {
                writes,
                conversations,
                entries,
                documents,
                retired,
                keys: Vec::new(),
            });
        }
        let mut keys = table_keys(&writes);
        for entry in &self.documents {
            if !document_writes(entry) {
                continue;
            }
            keys.push(document_key(&entry.address_id));
            keys.push(incarnation_key(document_id(&entry.target)));
            if entry.retire
                || matches!(
                    entry.target,
                    DocTarget::Created { .. } | DocTarget::ForkCopy { .. }
                )
            {
                keys.push(scope_key(&entry.address.scope));
            }
        }
        Ok(Adopted {
            writes,
            conversations,
            entries,
            documents,
            retired,
            keys,
        })
    }

    fn stage_conversation(
        &mut self,
        parent: Option<ConversationParent>,
        ownership: ConversationOwnership,
        reserved_id: Option<i64>,
    ) -> Result<ConversationRecord, Error> {
        self.assert_open()?;
        let owner = match ownership {
            ConversationOwnership::Ownerless => None,
            ConversationOwnership::Task { task_id } => {
                let task = self.current_task(task_id)?.ok_or_else(|| {
                    Error::message(format!("Conversation owner task {task_id} does not exist"))
                })?;
                Some(ConversationOwner {
                    conversation_id: task.conversation_id,
                    task_id,
                })
            }
        };
        let id = match reserved_id {
            Some(id) => id,
            None => self.mint()?,
        };
        if let Some(parent) = parent {
            let copies = prepare_fork_document_copies(
                &self.store,
                parent.conversation_id,
                parent.at,
                id,
            )?;
            for copy in copies {
                self.install_fork_copy(copy)?;
            }
            self.fork_source_conversation_ids
                .insert(parent.conversation_id);
        }
        let record = ConversationRecord { id, parent, owner };
        self.has_table_write = true;
        self.created_conversation_ids.insert(id);
        self.writes.push(StorageWrite::Conversation {
            value: record.clone(),
        });
        conversation_created(self, &record)?;
        Ok(record)
    }

    fn install_fork_copy(&mut self, copy: ForkDocumentCopy) -> Result<(), Error> {
        self.fork_source_document_ids.insert(copy.source.id);
        let address = DocumentAddress {
            kind: copy.record.kind.clone(),
            scope: copy.record.scope,
            key: copy.record.key.clone(),
        };
        let address_id = address_id(&address);
        let def = DocDef::conversation(
            copy.record.kind.clone(),
            copy.version,
            copy.record.history.unwrap_or(e4a_types::DocumentHistory::Latest),
            copy.record.fork.unwrap_or(e4a_types::DocumentFork::Current),
            {
                let value = copy.value.clone();
                move || value.clone()
            },
            crate::docs::Checkpoint::Always,
        );
        self.push_document(DocEntry {
            address_id,
            address,
            def,
            target: DocTarget::ForkCopy {
                record: copy.record,
                source: copy.source,
                version: copy.version,
            },
            ops: Vec::new(),
            value: copy.value,
            retire: false,
        });
        Ok(())
    }

    fn push_document(&mut self, entry: DocEntry) -> DocRef {
        let index = self.documents.len();
        self.latest_document
            .insert(entry.address_id.clone(), index);
        self.documents.push(entry);
        DocRef { index }
    }

    fn mint(&self) -> Result<i64, Error> {
        self.store
            .call(|storage| storage.mint_id().map_err(Error::from))
    }

    fn require_conversation(&mut self, id: i64) -> Result<(), Error> {
        if self.created_conversation_ids.contains(&id) {
            return Ok(());
        }
        self.store.touch(&conversation_key(id))?;
        let found = self
            .store
            .call(move |storage| storage.conversation(id).map_err(Error::from))?;
        if found.is_some() {
            Ok(())
        } else {
            Err(Error::message(format!("Conversation {id} does not exist")))
        }
    }

    fn current_task(&mut self, id: i64) -> Result<Option<TaskRecord>, Error> {
        if let Some(write) = self.tasks.get(&id) {
            return Ok(Some(write.record.clone()));
        }
        self.committed_task(id)
    }

    fn committed_task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.store.touch(&task_key(id))?;
        self.store
            .call(move |storage| storage.task(id).map_err(Error::from))
    }

    fn stamp_times(&self, mut value: TaskRecord, candidate: Option<&TaskRecord>) -> TaskRecord {
        let now = (self.now)();
        if value.started_at.is_none() {
            value.started_at = candidate
                .and_then(|record| record.started_at)
                .or_else(|| {
                    if matches!(value.state, TaskState::Running { .. }) {
                        Some(now)
                    } else {
                        None
                    }
                });
        }
        if value.ended_at.is_none() {
            value.ended_at = candidate.and_then(|record| record.ended_at).or_else(|| {
                if matches!(value.state, TaskState::Terminal { .. }) {
                    Some(now)
                } else {
                    None
                }
            });
        }
        value
    }

    fn apply_submission_changes(&mut self) -> Result<(), Error> {
        let changes = std::mem::take(&mut self.submission_changes);
        for (id, change) in changes {
            let current = if let Some(record) = self.submissions.get(&id) {
                record.clone()
            } else {
                self.store.touch(&submission_key(id))?;
                self.store
                    .call(move |storage| storage.submission(id).map_err(Error::from))?
                    .ok_or_else(|| Error::message(format!("Submission {id} does not exist")))?
            };
            let next = apply_submission_change(&current, change)?;
            self.submissions.insert(id, next);
        }
        Ok(())
    }

    fn validate_owners(&mut self) -> Result<(), Error> {
        let mut owners = Vec::new();
        for write in &self.writes {
            if let StorageWrite::Conversation {
                value: ConversationRecord {
                    owner: Some(owner),
                    ..
                },
            } = write
            {
                owners.push(("Conversation owner task", owner.task_id));
            }
        }
        for write in self.tasks.values() {
            if matches!(write.kind, TaskWriteKind::Create) {
                if let Some(owner) = write.record.owner {
                    owners.push(("Task owner", owner));
                }
            }
        }
        for (what, task_id) in owners {
            let task = self.current_task(task_id)?.ok_or_else(|| {
                Error::message(format!("{what} {task_id} does not exist"))
            })?;
            match &task.state {
                TaskState::Terminal { .. } | TaskState::Completing { .. } => {
                    return Err(Error::message(format!(
                        "{what} {task_id} is {}",
                        task.state.status()
                    )));
                }
                _ => {}
            }
            if task.abort_requested {
                return Err(Error::message(format!("{what} {task_id} is abort-marked")));
            }
        }
        Ok(())
    }

    fn reject_fork_source_writes(&self) -> Result<(), Error> {
        for entry in &self.documents {
            if !document_writes(entry) {
                continue;
            }
            let id = document_id(&entry.target);
            if self.fork_source_document_ids.contains(&id) {
                return Err(Error::message(format!(
                    "Cannot change fork source document {id} in the fork transaction"
                )));
            }
            if let DocumentScope::Conversation { conversation_id } = entry.address.scope {
                if self.fork_source_conversation_ids.contains(&conversation_id)
                    && entry.def.fork == Some(e4a_types::DocumentFork::Current)
                    && !matches!(entry.target, DocTarget::ForkCopy { .. } | DocTarget::Created { .. })
                {
                    return Err(Error::message(format!(
                        "Cannot fork conversation {conversation_id} while changing its current-policy documents"
                    )));
                }
            }
        }
        Ok(())
    }

    fn assert_readable(&mut self, what: &str) -> Result<(), Error> {
        self.assert_open()?;
        if self.has_table_write {
            Err(Error::message(format!("Read after write: {what}")))
        } else {
            Ok(())
        }
    }

    fn assert_open(&self) -> Result<(), Error> {
        if self.sealed {
            Err(Error::message("Transaction has settled"))
        } else {
            Ok(())
        }
    }
}

struct Assembled {
    write: Option<StorageWrite>,
    document: Option<AdoptedDocument>,
    retire: bool,
}

fn assemble_document(entry: &DocEntry) -> Result<Option<Assembled>, Error> {
    match &entry.target {
        DocTarget::Created { record } => {
            let content = DocumentContent::Base {
                version: entry.def.version,
                value: entry.value.clone(),
            };
            Ok(Some(Assembled {
                write: Some(StorageWrite::DocumentCreate {
                    record: record.clone(),
                    content,
                }),
                document: Some(AdoptedDocument {
                    address_id: entry.address_id.clone(),
                    record: document_record(record, 0),
                    value: entry.value.clone(),
                }),
                retire: entry.retire,
            }))
        }
        DocTarget::ForkCopy {
            record,
            source,
            version,
        } => Ok(Some(Assembled {
            write: Some(copy_write(&ForkDocumentCopy {
                record: record.clone(),
                source: source.clone(),
                value: entry.value.clone(),
                version: *version,
            })),
            document: Some(AdoptedDocument {
                address_id: entry.address_id.clone(),
                record: document_record(record, 0),
                value: entry.value.clone(),
            }),
            retire: entry.retire,
        })),
        DocTarget::Loaded {
            record,
            deltas_since_base,
            stored_version,
        } => {
            let content = if *stored_version < entry.def.version {
                Some(DocumentContent::Base {
                    version: entry.def.version,
                    value: entry.value.clone(),
                })
            } else if entry.ops.is_empty() {
                None
            } else if entry
                .def
                .checkpoint
                .when(&entry.value, &entry.ops, *deltas_since_base)
            {
                Some(DocumentContent::Base {
                    version: entry.def.version,
                    value: entry.value.clone(),
                })
            } else {
                Some(DocumentContent::Delta {
                    version: entry.def.version,
                    ops: entry.ops.clone(),
                })
            };
            let write = content.map(|content| StorageWrite::DocumentChange {
                id: record.id,
                content,
            });
            let document = if write.is_some() || entry.retire {
                Some(AdoptedDocument {
                    address_id: entry.address_id.clone(),
                    record: record.clone(),
                    value: entry.value.clone(),
                })
            } else {
                None
            };
            if write.is_none() && !entry.retire {
                Ok(None)
            } else {
                Ok(Some(Assembled {
                    write,
                    document,
                    retire: entry.retire,
                }))
            }
        }
    }
}

fn document_record(create: &DocumentCreate, created_at: i64) -> DocumentRecord {
    DocumentRecord {
        id: create.id,
        kind: create.kind.clone(),
        key: create.key.clone(),
        created_at,
        retired_at: None,
        scope: create.scope,
        history: create.history,
        fork: create.fork,
    }
}

fn document_id(target: &DocTarget) -> i64 {
    match target {
        DocTarget::Created { record } | DocTarget::ForkCopy { record, .. } => record.id,
        DocTarget::Loaded { record, .. } => record.id,
    }
}

fn document_writes(entry: &DocEntry) -> bool {
    entry.retire
        || !entry.ops.is_empty()
        || matches!(
            entry.target,
            DocTarget::Created { .. } | DocTarget::ForkCopy { .. }
        )
}

fn apply_submission_change(
    current: &SubmissionRecord,
    change: SubmissionChange,
) -> Result<SubmissionRecord, Error> {
    if matches!(
        current.status,
        SubmissionStatus::Done | SubmissionStatus::Unanswered
    ) {
        return Ok(current.clone());
    }
    match change {
        SubmissionChange::Place { entry } => {
            if current.status != SubmissionStatus::Queued {
                return Err(Error::message(format!(
                    "Submission {} is not queued",
                    current.id
                )));
            }
            let status = if current.ty == SubmissionType::Input {
                SubmissionStatus::Placed
            } else {
                SubmissionStatus::Done
            };
            let mut next = current.clone();
            next.status = status;
            next.entry = Some(entry);
            Ok(next)
        }
        SubmissionChange::Settle(SubmissionSettlement::Done { answer }) => {
            if current.status != SubmissionStatus::Placed {
                return Err(Error::message(format!(
                    "Submission {} is not a placed input or continue",
                    current.id
                )));
            }
            let mut next = current.clone();
            next.status = SubmissionStatus::Done;
            next.answer = Some(answer);
            Ok(next)
        }
        SubmissionChange::Settle(SubmissionSettlement::Unanswered { reason, detail }) => {
            let mut next = current.clone();
            next.status = SubmissionStatus::Unanswered;
            next.reason = Some(reason);
            next.detail = detail;
            Ok(next)
        }
    }
}

fn conversation_created(tx: &mut Tx, record: &ConversationRecord) -> Result<(), Error> {
    tx.doc(&provider_doc(), Some(record.id))?;
    if record.parent.is_none() {
        tx.doc(&agent_doc(), Some(record.id))?;
    }
    Ok(())
}

impl Adopted {
    pub(crate) fn stamp_seq(&mut self, seq: i64) {
        for document in &mut self.documents {
            if document.record.created_at == 0 {
                document.record.created_at = seq;
            }
        }
    }
}

pub(crate) fn empty_scope() -> TransactionScope {
    TransactionScope { task_id: None }
}
