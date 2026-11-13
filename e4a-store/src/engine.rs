// Portions derived from pi-durable 1.5.0 (packages/durable/src/storage/memory.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! In-memory durable engine. Reads return owned clones. A refused batch changes nothing.

use std::collections::{HashMap, HashSet};

use e4a_types::{
    apply, is_safe_integer, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentCreate, DocumentHistory, DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope,
    EntryHit, EntryQuery, EntryRecord, Json, Page, ScanOrder, StorageWrite, StoredDocument,
    SubmissionQuery, SubmissionRecord, SubmissionStatus, TaskQuery, TaskRecord, TaskStatus,
    ROOT_CONVERSATION_ID,
};

use crate::Error;

#[derive(Clone, Debug)]
pub(crate) struct Revision {
    pub seq: i64,
    pub content: DocumentContent,
}

#[derive(Clone, Debug)]
pub(crate) struct StoredDoc {
    pub record: DocumentRecord,
    pub revisions: Vec<Revision>,
}

#[derive(Clone, Debug, Default)]
struct Address {
    ids: Vec<i64>,
    current_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum ScopeKey {
    Session,
    Conversation(i64),
    Task(i64),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AddrKey {
    kind: String,
    scope: ScopeKey,
    key: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordType {
    Conversation,
    Entry,
    Task,
    Submission,
    Document,
}

impl RecordType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RecordType::Conversation => "conversation",
            RecordType::Entry => "entry",
            RecordType::Task => "task",
            RecordType::Submission => "submission",
            RecordType::Document => "document",
        }
    }
}

/// One document row the batch inserted or updated, plus the revision rows to write.
#[derive(Debug)]
pub(crate) struct DocumentWrite {
    pub inserted: bool,
    pub record: DocumentRecord,
    /// Delete stored revision rows before inserting `revisions`.
    pub replace_revisions: bool,
    pub revisions: Vec<Revision>,
}

/// Rows one accepted batch inserts or updates. Rows the batch did not touch are absent.
#[derive(Debug)]
pub(crate) struct BatchWrite {
    pub seq: i64,
    pub conversations: Vec<ConversationRecord>,
    pub entries: Vec<EntryRecord>,
    pub tasks: Vec<TaskRecord>,
    pub submissions: Vec<SubmissionRecord>,
    pub documents: Vec<DocumentWrite>,
}

/// Accepted batches from one `commit`, plus the first refusal if a later batch failed its checks.
#[derive(Debug)]
pub(crate) struct ApplyOutcome {
    pub seqs: Vec<i64>,
    pub writes: Vec<BatchWrite>,
    pub refused: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Action {
    create: Option<DocumentCreate>,
    content: Option<DocumentContent>,
    retire: bool,
}

type Prepared = (Vec<StorageWrite>, Vec<(i64, Action)>);

#[derive(Clone, Debug, Default)]
struct TaskBuckets {
    pending: Vec<i64>,
    running: Vec<i64>,
    waiting: Vec<i64>,
    sleeping: Vec<i64>,
    completing: Vec<i64>,
    terminal: Vec<i64>,
}

#[derive(Clone, Debug, Default)]
struct SubmissionBuckets {
    queued: Vec<i64>,
    placed: Vec<i64>,
    done: Vec<i64>,
    unanswered: Vec<i64>,
}

#[derive(Debug)]
pub(crate) struct Engine {
    next_id: i64,
    next_seq: i64,
    record_types: HashMap<i64, RecordType>,
    conversations: HashMap<i64, ConversationRecord>,
    conversation_ids: Vec<i64>,
    by_owner_conversation: HashMap<i64, Vec<i64>>,
    by_owner_task: HashMap<i64, Vec<i64>>,
    entries: HashMap<i64, EntryRecord>,
    entry_commit_seqs: HashMap<i64, i64>,
    entry_ids: HashMap<i64, Vec<i64>>,
    head_entry_ids: HashMap<i64, Vec<i64>>,
    tasks: HashMap<i64, TaskRecord>,
    task_ids: Vec<i64>,
    task_buckets: TaskBuckets,
    submissions: HashMap<i64, SubmissionRecord>,
    submission_ids: Vec<i64>,
    submission_buckets: SubmissionBuckets,
    submission_by_request: HashMap<i64, HashMap<String, i64>>,
    documents: HashMap<i64, StoredDoc>,
    document_addresses: HashMap<AddrKey, Address>,
    document_ids_by_scope: HashMap<ScopeKey, Vec<i64>>,
}

impl Engine {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 2,
            next_seq: 1,
            record_types: HashMap::new(),
            conversations: HashMap::new(),
            conversation_ids: Vec::new(),
            by_owner_conversation: HashMap::new(),
            by_owner_task: HashMap::new(),
            entries: HashMap::new(),
            entry_commit_seqs: HashMap::new(),
            entry_ids: HashMap::new(),
            head_entry_ids: HashMap::new(),
            tasks: HashMap::new(),
            task_ids: Vec::new(),
            task_buckets: TaskBuckets::default(),
            submissions: HashMap::new(),
            submission_ids: Vec::new(),
            submission_buckets: SubmissionBuckets::default(),
            submission_by_request: HashMap::new(),
            documents: HashMap::new(),
            document_addresses: HashMap::new(),
            document_ids_by_scope: HashMap::new(),
        }
    }

    pub(crate) fn next_id(&self) -> i64 {
        self.next_id
    }

    pub(crate) fn next_seq(&self) -> i64 {
        self.next_seq
    }

    /// Apply every batch that passes checks. A refusal keeps earlier batches and drops the rest.
    ///
    /// `writes` lists only the rows those batches changed.
    pub(crate) fn apply_batches(
        &mut self,
        batches: &[Vec<StorageWrite>],
    ) -> Result<ApplyOutcome, Error> {
        let mut seqs = Vec::new();
        let mut writes = Vec::new();
        for batch in batches {
            let prepared = match self.prepare(batch) {
                Ok(prepared) => prepared,
                Err(message) => {
                    return Ok(ApplyOutcome {
                        seqs,
                        writes,
                        refused: Some(format!("Error: {message}")),
                    })
                }
            };
            let seq = self.next_seq;
            writes.push(self.apply_prepared(prepared, seq));
            seqs.push(seq);
        }
        Ok(ApplyOutcome {
            seqs,
            writes,
            refused: None,
        })
    }

    pub(crate) fn mint_id(&mut self) -> Result<i64, Error> {
        if !is_safe_integer(self.next_id) {
            return Err(Error::message("ID space is exhausted"));
        }
        let id = self.next_id;
        self.next_id += 1;
        Ok(id)
    }

    pub(crate) fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        Ok(self.conversations.get(&id).cloned())
    }

    pub(crate) fn scan_conversations(
        &self,
        query_owner_conversation: Option<i64>,
        query_owner_task: Option<i64>,
        order: Option<ScanOrder>,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error> {
        let ids = if let Some(task_id) = query_owner_task {
            self.by_owner_task
                .get(&task_id)
                .cloned()
                .unwrap_or_default()
        } else if let Some(conversation_id) = query_owner_conversation {
            self.by_owner_conversation
                .get(&conversation_id)
                .cloned()
                .unwrap_or_default()
        } else {
            self.conversation_ids.clone()
        };
        let (order, after) = scan_start(order, cursor, ScanOrder::Ascending)?;
        let scanned = scan_ids(&ids, order, after);
        let mut values = Vec::new();
        for id in scanned {
            if values.len() > limit {
                break;
            }
            let Some(value) = self.conversations.get(&id) else {
                continue;
            };
            if let Some(owner_conversation) = query_owner_conversation {
                if value.owner.map(|owner| owner.conversation_id) != Some(owner_conversation) {
                    continue;
                }
            }
            values.push(value.clone());
        }
        Ok(page(values, limit, order, |record| record.id))
    }

    pub(crate) fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error> {
        let Some(entry) = self.entries.get(&id) else {
            return Ok(None);
        };
        Ok(Some(EntryHit {
            commit_seq: self.entry_commit_seqs[&id],
            entry: entry.clone(),
        }))
    }

    pub(crate) fn entry_in(
        &self,
        conversation_id: i64,
        id: i64,
    ) -> Result<Option<EntryHit>, Error> {
        let mut found = None;
        self.for_each_visible(conversation_id, Some(id), Some(id), false, |entry| {
            found = Some(entry.clone());
            false
        })?;
        Ok(found.map(|entry| EntryHit {
            commit_seq: self.entry_commit_seqs[&entry.id],
            entry,
        }))
    }

    pub(crate) fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error> {
        if !self.conversations.contains_key(&conversation_id) {
            return Err(unknown_conversation(conversation_id));
        }
        let mut current = conversation_id;
        let mut upper = at_or_before;
        loop {
            let ids = self
                .head_entry_ids
                .get(&current)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let index = upper_bound_opt(ids, upper) as isize - 1;
            if index >= 0 {
                let id = ids[index as usize];
                return Ok(self.entries.get(&id).cloned());
            }
            let parent = self
                .conversations
                .get(&current)
                .and_then(|conversation| conversation.parent);
            let Some(parent) = parent else {
                return Ok(None);
            };
            upper = cap_max(upper, parent.at);
            current = parent.conversation_id;
            // Children that point at a deleted parent stay; that history is gone.
            if !self.conversations.contains_key(&current) {
                break;
            }
        }
        Ok(None)
    }

    pub(crate) fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error> {
        let (order, after) = scan_start(query.order, cursor, ScanOrder::Descending)?;
        let mut min_entry_id = query.min_entry_id;
        let mut max_entry_id = query.max_entry_id;
        if let Some(after) = after {
            if order == ScanOrder::Descending {
                let cap = after.saturating_sub(1);
                max_entry_id = Some(max_entry_id.map(|max| max.min(cap)).unwrap_or(cap));
            } else {
                let cap = after.saturating_add(1);
                min_entry_id = Some(min_entry_id.map(|min| min.max(cap)).unwrap_or(cap));
            }
        }
        let mut values = Vec::new();
        let ascending = order == ScanOrder::Ascending;
        self.for_each_visible(
            query.conversation_id,
            min_entry_id,
            max_entry_id,
            ascending,
            |entry| {
                values.push(entry.clone());
                values.len() <= limit
            },
        )?;
        Ok(page(values, limit, order, |record| record.id))
    }

    pub(crate) fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        Ok(self.tasks.get(&id).cloned())
    }

    pub(crate) fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error> {
        let (order, after) = scan_start(query.order, cursor, ScanOrder::Ascending)?;
        let ids = match query.status {
            Some(status) => task_bucket(&self.task_buckets, status).clone(),
            None => self.task_ids.clone(),
        };
        let scanned = scan_ids(&ids, order, after);
        let mut values = Vec::new();
        for id in scanned {
            if values.len() > limit {
                break;
            }
            let Some(value) = self.tasks.get(&id) else {
                continue;
            };
            if query
                .conversation_id
                .is_some_and(|conversation_id| value.conversation_id != conversation_id)
            {
                continue;
            }
            if query.kind.as_ref().is_some_and(|kind| &value.kind != kind) {
                continue;
            }
            if query
                .abort_requested
                .is_some_and(|abort| value.abort_requested != abort)
            {
                continue;
            }
            if query
                .background
                .is_some_and(|background| value.background != background)
            {
                continue;
            }
            values.push(value.clone());
        }
        Ok(page(values, limit, order, |record| record.id))
    }

    pub(crate) fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        Ok(self.submissions.get(&id).cloned())
    }

    pub(crate) fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error> {
        let (order, after) = scan_start(query.order, cursor, ScanOrder::Ascending)?;
        let ids = match query.status {
            Some(status) => submission_bucket(&self.submission_buckets, status).clone(),
            None => self.submission_ids.clone(),
        };
        let scanned = scan_ids(&ids, order, after);
        let mut values = Vec::new();
        for id in scanned {
            if values.len() > limit {
                break;
            }
            let Some(value) = self.submissions.get(&id) else {
                continue;
            };
            if query
                .conversation_id
                .is_some_and(|conversation_id| value.conversation_id != conversation_id)
            {
                continue;
            }
            values.push(value.clone());
        }
        Ok(page(values, limit, order, |record| record.id))
    }

    pub(crate) fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        let Some(id) = self
            .submission_by_request
            .get(&conversation_id)
            .and_then(|requests| requests.get(request_id))
        else {
            return Ok(None);
        };
        Ok(self.submissions.get(id).cloned())
    }

    pub(crate) fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<DocumentRecord>, Error> {
        let Some(index) =
            self.document_addresses
                .get(&addr_key(&address.kind, &address.scope, &address.key))
        else {
            return Ok(None);
        };
        match at {
            DocumentPoint::Current => Ok(index
                .current_id
                .and_then(|id| self.documents.get(&id).map(|doc| doc.record.clone()))),
            DocumentPoint::Seq(_) => {
                for id in &index.ids {
                    let record = &self.documents[id].record;
                    if is_alive(record, at) {
                        return Ok(Some(record.clone()));
                    }
                }
                Ok(None)
            }
        }
    }

    pub(crate) fn document(
        &self,
        id: i64,
        at: DocumentPoint,
    ) -> Result<Option<StoredDocument>, Error> {
        self.materialize(id, at)
    }

    pub(crate) fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<DocumentRecord>, Error> {
        let ids = self
            .document_ids_by_scope
            .get(&scope_key(&query.scope))
            .cloned()
            .unwrap_or_default();
        let after = document_after(cursor)?;
        let start = match after {
            Some(after) => upper_bound(&ids, after),
            None => 0,
        };
        let mut values = Vec::new();
        for id in ids.iter().skip(start) {
            if values.len() > limit {
                break;
            }
            let record = &self.documents[id].record;
            if query.kind.as_ref().is_some_and(|kind| &record.kind != kind) {
                continue;
            }
            if is_alive(record, query.at) {
                values.push(record.clone());
            }
        }
        Ok(page(values, limit, ScanOrder::Ascending, |record| {
            record.id
        }))
    }

    pub(crate) fn load_conversation(&mut self, record: ConversationRecord) {
        self.put_conversation(record);
    }

    pub(crate) fn load_entry(&mut self, record: EntryRecord, commit_seq: i64) {
        self.note_seq(commit_seq);
        self.put_entry(record, commit_seq);
    }

    pub(crate) fn load_task(&mut self, record: TaskRecord) {
        self.put_task(record);
    }

    pub(crate) fn load_submission(&mut self, record: SubmissionRecord) {
        self.put_submission(record);
    }

    pub(crate) fn load_document(&mut self, record: DocumentRecord, revisions: Vec<Revision>) {
        let id = record.id;
        self.note_seq(record.created_at);
        if let Some(retired_at) = record.retired_at {
            self.note_seq(retired_at);
        }
        for revision in &revisions {
            self.note_seq(revision.seq);
        }
        let key = addr_key_record(&record);
        let scope = scope_key(&record.scope);
        let alive = record.retired_at.is_none();
        {
            let address = self.document_addresses.entry(key).or_default();
            insert_sorted(&mut address.ids, id);
            if alive {
                address.current_id = Some(id);
            }
        }
        insert_sorted(self.document_ids_by_scope.entry(scope).or_default(), id);
        self.record_types.insert(id, RecordType::Document);
        self.bump(id);
        self.documents.insert(id, StoredDoc { record, revisions });
    }

    pub(crate) fn finish_load(&mut self, next_id: i64, next_seq: i64) {
        self.next_id = self.next_id.max(next_id);
        self.next_seq = self.next_seq.max(next_seq);
    }

    fn prepare(&self, writes: &[StorageWrite]) -> Result<Prepared, String> {
        let mut writes = writes.to_vec();
        self.resolve_copies(&mut writes)?;
        self.check_ids(&writes)?;
        let actions = prepare_actions(&writes)?;
        self.check_actions(&actions)?;
        Ok((writes, actions))
    }

    fn resolve_copies(&self, writes: &mut [StorageWrite]) -> Result<(), String> {
        if !writes
            .iter()
            .any(|write| matches!(write, StorageWrite::DocumentCopy { .. }))
        {
            return Ok(());
        }
        let mut changed = HashSet::new();
        for write in writes.iter() {
            match write {
                StorageWrite::DocumentCreate { record, .. }
                | StorageWrite::DocumentCopy { record, .. } => {
                    changed.insert(record.id);
                }
                StorageWrite::DocumentChange { id, .. } | StorageWrite::DocumentRetire { id } => {
                    changed.insert(*id);
                }
                _ => {}
            }
        }
        for write in writes.iter_mut() {
            let StorageWrite::DocumentCopy { record, source } = write else {
                continue;
            };
            let copy_id = record.id;
            let resolved = (|| -> Result<StorageWrite, String> {
                if changed.contains(&source.id) {
                    return Err(format!(
                        "Fork source document {} is changed in the copy batch",
                        source.id
                    ));
                }
                let stored = self
                    .materialize(source.id, source.at)
                    .map_err(|error| error.to_string())?;
                let Some(stored) = stored else {
                    return Err(format!("Fork source document {} cannot be read", source.id));
                };
                if !copy_matches(&stored.record, record) {
                    return Err(format!(
                        "Fork source document {} does not match the copied record",
                        source.id
                    ));
                }
                Ok(StorageWrite::DocumentCreate {
                    record: record.clone(),
                    content: DocumentContent::Base {
                        version: stored.version,
                        value: stored.value,
                    },
                })
            })();
            match resolved {
                Ok(create) => *write = create,
                Err(_) => return Err(format!("Document copy {copy_id} was rejected")),
            }
        }
        Ok(())
    }

    fn check_ids(&self, writes: &[StorageWrite]) -> Result<(), String> {
        let mut claimed: HashMap<i64, &'static str> = HashMap::new();
        for write in writes {
            let (table, id) = match write {
                StorageWrite::Conversation { value } => ("conversation", value.id),
                StorageWrite::Entry { value } => ("entry", value.id),
                StorageWrite::Task { value } => ("task", value.id),
                StorageWrite::Submission { value } => ("submission", value.id),
                StorageWrite::DocumentCreate { record, .. } => ("document", record.id),
                StorageWrite::DocumentCopy { .. } => {
                    return Err("Prepared document copy was not resolved".into());
                }
                StorageWrite::DocumentChange { .. } | StorageWrite::DocumentRetire { .. } => {
                    continue
                }
            };
            let existing = self.record_types.get(&id).map(|kind| kind.as_str());
            let earlier = claimed.get(&id).copied();
            if table == "conversation" || table == "entry" || table == "document" {
                if let Some(existing) = existing {
                    return Err(format!("ID {id} already belongs to {existing}"));
                }
                if earlier.is_some() {
                    return Err(format!("ID {id} is written more than once"));
                }
            } else {
                if let Some(existing) = existing {
                    if existing != table {
                        return Err(format!("ID {id} already belongs to {existing}"));
                    }
                }
                if let Some(earlier) = earlier {
                    if earlier != table {
                        return Err(format!("ID {id} is written as two record types"));
                    }
                }
            }
            claimed.insert(id, table);
        }
        Ok(())
    }

    fn check_actions(&self, actions: &[(i64, Action)]) -> Result<(), String> {
        let mut live_counts: HashMap<AddrKey, i64> = HashMap::new();
        for (id, action) in actions {
            let existing = self.documents.get(id);
            if action.create.is_none() && existing.is_none() {
                return Err(format!("Unknown document: {id}"));
            }
            if action.create.is_some() && existing.is_some() {
                return Err(format!("Document {id} already exists"));
            }
            if existing.and_then(|doc| doc.record.retired_at).is_some() {
                return Err(format!("Document {id} is retired"));
            }
            if let Some(DocumentContent::Delta { version, .. }) = &action.content {
                let Some(previous) = existing.and_then(|doc| doc.revisions.last()) else {
                    return Err(format!("Document {id} delta has no base"));
                };
                let previous_version = match &previous.content {
                    DocumentContent::Base { version, .. }
                    | DocumentContent::Delta { version, .. } => *version,
                };
                if previous_version != *version {
                    return Err(format!("Document {id} version transition requires a base"));
                }
            }
            let key = if let Some(create) = &action.create {
                addr_key(&create.kind, &create.scope, &create.key)
            } else {
                addr_key_record(&existing.expect("existing document").record)
            };
            let current_id = self
                .document_addresses
                .get(&key)
                .and_then(|address| address.current_id);
            let live = live_counts
                .entry(key)
                .or_insert(if current_id.is_some() { 1 } else { 0 });
            if action.retire && current_id == Some(*id) {
                *live -= 1;
            }
            if action.create.is_some() && !action.retire {
                *live += 1;
            }
        }
        if live_counts.values().any(|live| *live > 1) {
            return Err("Document address already has a current incarnation".into());
        }
        Ok(())
    }

    fn apply_prepared(
        &mut self,
        prepared: (Vec<StorageWrite>, Vec<(i64, Action)>),
        seq: i64,
    ) -> BatchWrite {
        let (writes, actions) = prepared;
        let mut applied = BatchWrite {
            seq,
            conversations: Vec::new(),
            entries: Vec::new(),
            tasks: Vec::new(),
            submissions: Vec::new(),
            documents: Vec::new(),
        };
        for write in writes {
            match write {
                StorageWrite::Conversation { value } => {
                    applied.conversations.push(value.clone());
                    self.put_conversation(value);
                }
                StorageWrite::Entry { value } => {
                    applied.entries.push(value.clone());
                    self.put_entry(value, seq);
                }
                StorageWrite::Task { value } => {
                    applied.tasks.push(value.clone());
                    self.put_task(value);
                }
                StorageWrite::Submission { value } => {
                    applied.submissions.push(value.clone());
                    self.put_submission(value);
                }
                StorageWrite::DocumentCopy { .. } => {
                    unreachable!("document copy is resolved before apply")
                }
                StorageWrite::DocumentCreate { .. }
                | StorageWrite::DocumentChange { .. }
                | StorageWrite::DocumentRetire { .. } => {}
            }
        }
        applied.documents = self.apply_actions(actions, seq);
        self.next_seq = seq + 1;
        applied
    }

    fn apply_actions(&mut self, actions: Vec<(i64, Action)>, seq: i64) -> Vec<DocumentWrite> {
        let mut writes = Vec::new();
        for (id, action) in actions {
            if let Some(create) = action.create {
                let record = record_from_create(&create, seq, action.retire);
                let content = action.content.expect("document create carries content");
                let mut revisions = vec![Revision { seq, content }];
                if action.retire && is_current_only(&record) {
                    revisions.clear();
                }
                let key = addr_key_record(&record);
                let scope = scope_key(&record.scope);
                {
                    let address = self.document_addresses.entry(key).or_default();
                    insert_sorted(&mut address.ids, id);
                    if action.retire && address.current_id == Some(id) {
                        address.current_id = None;
                    }
                    if !action.retire {
                        address.current_id = Some(id);
                    }
                }
                insert_sorted(self.document_ids_by_scope.entry(scope).or_default(), id);
                self.record_types.insert(id, RecordType::Document);
                self.bump(id);
                writes.push(DocumentWrite {
                    inserted: true,
                    record: record.clone(),
                    replace_revisions: false,
                    revisions: revisions.clone(),
                });
                self.documents.insert(id, StoredDoc { record, revisions });
            } else {
                let key = {
                    let stored = self.documents.get_mut(&id).expect("checked document");
                    let before_len = stored.revisions.len();
                    let before_seqs: Vec<i64> = stored
                        .revisions
                        .iter()
                        .map(|revision| revision.seq)
                        .collect();
                    if let Some(content) = action.content {
                        let revision = Revision { seq, content };
                        if matches!(revision.content, DocumentContent::Base { .. })
                            && is_current_only(&stored.record)
                        {
                            stored.revisions = vec![revision];
                        } else {
                            stored.revisions.push(revision);
                        }
                    }
                    if action.retire {
                        stored.record.retired_at = Some(seq);
                        if is_current_only(&stored.record) {
                            stored.revisions.clear();
                        }
                    }
                    let replace_revisions = stored.revisions.len() < before_len
                        || stored
                            .revisions
                            .iter()
                            .zip(&before_seqs)
                            .any(|(revision, previous)| revision.seq != *previous);
                    let revisions = if replace_revisions {
                        stored.revisions.clone()
                    } else {
                        stored.revisions[before_len..].to_vec()
                    };
                    writes.push(DocumentWrite {
                        inserted: false,
                        record: stored.record.clone(),
                        replace_revisions,
                        revisions,
                    });
                    addr_key_record(&stored.record)
                };
                if action.retire {
                    if let Some(address) = self.document_addresses.get_mut(&key) {
                        if address.current_id == Some(id) {
                            address.current_id = None;
                        }
                    }
                }
            }
        }
        writes
    }

    fn put_conversation(&mut self, record: ConversationRecord) {
        let id = record.id;
        self.record_types.insert(id, RecordType::Conversation);
        insert_sorted(&mut self.conversation_ids, id);
        if let Some(owner) = record.owner {
            insert_map_id(&mut self.by_owner_conversation, owner.conversation_id, id);
            insert_map_id(&mut self.by_owner_task, owner.task_id, id);
        }
        self.bump(id);
        self.conversations.insert(id, record);
    }

    fn put_entry(&mut self, record: EntryRecord, seq: i64) {
        let id = record.id;
        let conversation_id = record.conversation_id;
        let head = record.head;
        self.record_types.insert(id, RecordType::Entry);
        self.entry_commit_seqs.insert(id, seq);
        insert_sorted(self.entry_ids.entry(conversation_id).or_default(), id);
        if head.is_some() {
            insert_sorted(self.head_entry_ids.entry(conversation_id).or_default(), id);
        }
        self.bump(id);
        self.entries.insert(id, record);
    }

    fn put_task(&mut self, record: TaskRecord) {
        let id = record.id;
        let status = record.state.status();
        let previous = self.tasks.get(&id).cloned();
        if let Some(previous) = previous {
            let previous_status = previous.state.status();
            if previous_status != status {
                remove_sorted(task_bucket_mut(&mut self.task_buckets, previous_status), id);
                insert_sorted(task_bucket_mut(&mut self.task_buckets, status), id);
            }
        } else {
            insert_sorted(&mut self.task_ids, id);
            insert_sorted(task_bucket_mut(&mut self.task_buckets, status), id);
        }
        self.record_types.insert(id, RecordType::Task);
        self.bump(id);
        self.tasks.insert(id, record);
    }

    fn put_submission(&mut self, record: SubmissionRecord) {
        let id = record.id;
        let status = record.status;
        let previous = self.submissions.get(&id).cloned();
        if let Some(previous) = previous {
            if previous.status != status {
                remove_sorted(
                    submission_bucket_mut(&mut self.submission_buckets, previous.status),
                    id,
                );
                insert_sorted(
                    submission_bucket_mut(&mut self.submission_buckets, status),
                    id,
                );
            }
            if let Some(request_id) = previous.request_id {
                let mut empty = false;
                if let Some(requests) = self
                    .submission_by_request
                    .get_mut(&previous.conversation_id)
                {
                    if requests.get(&request_id) == Some(&id) {
                        requests.remove(&request_id);
                    }
                    empty = requests.is_empty();
                }
                if empty {
                    self.submission_by_request.remove(&previous.conversation_id);
                }
            }
        } else {
            insert_sorted(&mut self.submission_ids, id);
            insert_sorted(
                submission_bucket_mut(&mut self.submission_buckets, status),
                id,
            );
        }
        if let Some(request_id) = record.request_id.clone() {
            self.submission_by_request
                .entry(record.conversation_id)
                .or_default()
                .insert(request_id, id);
        }
        self.record_types.insert(id, RecordType::Submission);
        self.bump(id);
        self.submissions.insert(id, record);
    }

    fn materialize(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error> {
        let Some(stored) = self.documents.get(&id) else {
            return Ok(None);
        };
        if !matches!(at, DocumentPoint::Current) && is_current_only(&stored.record) {
            return Err(Error::message(format!(
                "Document {id} does not retain historical content"
            )));
        }
        if !is_alive(&stored.record, at) {
            return Ok(None);
        }
        let revisions: Vec<&Revision> = match at {
            DocumentPoint::Current => stored.revisions.iter().collect(),
            DocumentPoint::Seq(seq) => stored
                .revisions
                .iter()
                .filter(|revision| revision.seq <= seq)
                .collect(),
        };
        let Some(base_index) = revisions
            .iter()
            .rposition(|revision| matches!(revision.content, DocumentContent::Base { .. }))
        else {
            return Err(Error::message(format!(
                "Document {id} is missing a required base"
            )));
        };
        let (version, mut value) = match &revisions[base_index].content {
            DocumentContent::Base { version, value } => (*version, value.clone()),
            DocumentContent::Delta { .. } => unreachable!("base index points at a base"),
        };
        let mut deltas_since_base = 0;
        for revision in revisions.iter().skip(base_index + 1) {
            match &revision.content {
                DocumentContent::Delta {
                    version: delta_version,
                    ops,
                } if *delta_version == version => {
                    value =
                        apply(&value, ops).map_err(|error| Error::message(error.to_string()))?;
                    deltas_since_base += 1;
                }
                _ => {
                    return Err(Error::message(format!(
                        "Document {id} crosses a stored version boundary without a base"
                    )));
                }
            }
        }
        Ok(Some(StoredDocument {
            record: stored.record.clone(),
            version,
            value,
            deltas_since_base,
        }))
    }

    fn for_each_visible(
        &self,
        conversation_id: i64,
        min_entry_id: Option<i64>,
        max_entry_id: Option<i64>,
        ascending: bool,
        mut visit: impl FnMut(&EntryRecord) -> bool,
    ) -> Result<(), Error> {
        if !self.conversations.contains_key(&conversation_id) {
            return Err(unknown_conversation(conversation_id));
        }
        if !ascending {
            let mut current = conversation_id;
            let mut upper = max_entry_id;
            loop {
                let ids = self
                    .entry_ids
                    .get(&current)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let mut index = upper_bound_opt(ids, upper) as isize - 1;
                while index >= 0 {
                    let id = ids[index as usize];
                    if below_min(id, min_entry_id) {
                        break;
                    }
                    if !visit(&self.entries[&id]) {
                        return Ok(());
                    }
                    index -= 1;
                }
                let parent = self.conversations[&current].parent;
                let Some(parent) = parent else {
                    break;
                };
                upper = cap_max(upper, parent.at);
                if let (Some(upper), Some(min_entry_id)) = (upper, min_entry_id) {
                    if upper < min_entry_id {
                        break;
                    }
                }
                current = parent.conversation_id;
                // Children that point at a deleted parent stay; that history is gone.
                if !self.conversations.contains_key(&current) {
                    break;
                }
            }
            return Ok(());
        }
        let mut segments = Vec::new();
        let mut current = conversation_id;
        let mut upper = max_entry_id;
        loop {
            segments.push((current, upper));
            let parent = self.conversations[&current].parent;
            let Some(parent) = parent else {
                break;
            };
            upper = cap_max(upper, parent.at);
            if let (Some(upper), Some(min_entry_id)) = (upper, min_entry_id) {
                if upper < min_entry_id {
                    break;
                }
            }
            current = parent.conversation_id;
            // Children that point at a deleted parent stay; that history is gone.
            if !self.conversations.contains_key(&current) {
                break;
            }
        }
        for (conversation_id, upper) in segments.into_iter().rev() {
            let ids = self
                .entry_ids
                .get(&conversation_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            for id in ids.iter().copied().skip(lower_bound_opt(ids, min_entry_id)) {
                if above_max(id, upper) {
                    break;
                }
                if !visit(&self.entries[&id]) {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn ensure_deletable(&self, id: i64) -> Result<(), Error> {
        if id == ROOT_CONVERSATION_ID {
            return Err(Error::message("The root cannot be deleted"));
        }
        if !self.conversations.contains_key(&id) {
            return Err(unknown_conversation(id));
        }
        Ok(())
    }

    /// Remove one conversation and the rows that belong to it.
    ///
    /// Children that point at `id` as parent stay; do not recurse.
    pub(crate) fn delete_conversation(&mut self, id: i64) -> Result<(), Error> {
        self.ensure_deletable(id)?;
        let record = match self.conversations.get(&id).cloned() {
            Some(record) => record,
            None => return Err(unknown_conversation(id)),
        };
        let entry_ids: Vec<i64> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.conversation_id == id)
            .map(|(entry_id, _)| *entry_id)
            .collect();
        for entry_id in entry_ids {
            self.entries.remove(&entry_id);
            self.entry_commit_seqs.remove(&entry_id);
            self.record_types.remove(&entry_id);
        }
        self.entry_ids.remove(&id);
        self.head_entry_ids.remove(&id);

        let task_ids: Vec<i64> = self
            .tasks
            .iter()
            .filter(|(_, task)| task.conversation_id == id)
            .map(|(task_id, _)| *task_id)
            .collect();
        for task_id in task_ids {
            self.remove_task(task_id);
            self.remove_scope_documents(ScopeKey::Task(task_id));
        }

        let submission_ids: Vec<i64> = self
            .submissions
            .iter()
            .filter(|(_, submission)| submission.conversation_id == id)
            .map(|(submission_id, _)| *submission_id)
            .collect();
        for submission_id in submission_ids {
            self.remove_submission(submission_id);
        }
        self.submission_by_request.remove(&id);
        self.remove_scope_documents(ScopeKey::Conversation(id));

        remove_sorted(&mut self.conversation_ids, id);
        if let Some(owner) = record.owner {
            remove_map_id(&mut self.by_owner_conversation, owner.conversation_id, id);
            remove_map_id(&mut self.by_owner_task, owner.task_id, id);
        }
        self.record_types.remove(&id);
        self.conversations.remove(&id);
        Ok(())
    }

    fn remove_task(&mut self, id: i64) {
        let Some(task) = self.tasks.remove(&id) else {
            return;
        };
        remove_sorted(&mut self.task_ids, id);
        remove_sorted(
            task_bucket_mut(&mut self.task_buckets, task.state.status()),
            id,
        );
        self.record_types.remove(&id);
    }

    fn remove_submission(&mut self, id: i64) {
        let Some(record) = self.submissions.remove(&id) else {
            return;
        };
        remove_sorted(&mut self.submission_ids, id);
        remove_sorted(
            submission_bucket_mut(&mut self.submission_buckets, record.status),
            id,
        );
        if let Some(request_id) = record.request_id {
            let mut empty = false;
            if let Some(requests) = self.submission_by_request.get_mut(&record.conversation_id) {
                if requests.get(&request_id) == Some(&id) {
                    requests.remove(&request_id);
                }
                empty = requests.is_empty();
            }
            if empty {
                self.submission_by_request.remove(&record.conversation_id);
            }
        }
        self.record_types.remove(&id);
    }

    fn remove_scope_documents(&mut self, scope: ScopeKey) {
        let ids: Vec<i64> = self
            .documents
            .iter()
            .filter(|(_, stored)| scope_key(&stored.record.scope) == scope)
            .map(|(id, _)| *id)
            .collect();
        self.document_ids_by_scope.remove(&scope);
        for id in ids {
            let Some(stored) = self.documents.remove(&id) else {
                self.record_types.remove(&id);
                continue;
            };
            let key = addr_key_record(&stored.record);
            let empty = if let Some(address) = self.document_addresses.get_mut(&key) {
                remove_sorted(&mut address.ids, id);
                if address.current_id == Some(id) {
                    address.current_id = None;
                }
                address.ids.is_empty()
            } else {
                false
            };
            if empty {
                self.document_addresses.remove(&key);
            }
            self.record_types.remove(&id);
        }
    }

    fn bump(&mut self, id: i64) {
        if let Some(next) = id.checked_add(1) {
            if next > self.next_id {
                self.next_id = next;
            }
        }
    }

    fn note_seq(&mut self, seq: i64) {
        if let Some(next) = seq.checked_add(1) {
            if next > self.next_seq {
                self.next_seq = next;
            }
        }
    }
}

fn prepare_actions(writes: &[StorageWrite]) -> Result<Vec<(i64, Action)>, String> {
    let mut order = Vec::new();
    let mut actions: HashMap<i64, Action> = HashMap::new();
    for write in writes {
        let id = match write {
            StorageWrite::DocumentCreate { record, .. } => record.id,
            StorageWrite::DocumentChange { id, .. } | StorageWrite::DocumentRetire { id } => *id,
            _ => continue,
        };
        let action = actions.entry(id).or_insert_with(|| {
            order.push(id);
            Action::default()
        });
        match write {
            StorageWrite::DocumentCreate { record, content } => {
                if action.create.is_some() || action.content.is_some() {
                    return Err(format!("Document {id} has more than one content command"));
                }
                action.create = Some(record.clone());
                action.content = Some(content.clone());
            }
            StorageWrite::DocumentChange { content, .. } => {
                if action.content.is_some() {
                    return Err(format!("Document {id} has more than one content command"));
                }
                action.content = Some(content.clone());
            }
            StorageWrite::DocumentRetire { .. } => {
                if action.retire {
                    return Err(format!("Document {id} is retired more than once"));
                }
                action.retire = true;
            }
            _ => {}
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|id| actions.remove(&id).map(|action| (id, action)))
        .collect())
}

fn copy_matches(stored: &DocumentRecord, record: &DocumentCreate) -> bool {
    let conversation = matches!(stored.scope, DocumentScope::Conversation { .. })
        && matches!(record.scope, DocumentScope::Conversation { .. });
    conversation
        && stored.kind == record.kind
        && stored.key == record.key
        && stored.history == record.history
        && stored.fork == record.fork
}

fn record_from_create(create: &DocumentCreate, seq: i64, retire: bool) -> DocumentRecord {
    DocumentRecord {
        id: create.id,
        kind: create.kind.clone(),
        key: create.key.clone(),
        created_at: seq,
        retired_at: retire.then_some(seq),
        scope: create.scope,
        history: create.history,
        fork: create.fork,
    }
}

fn is_current_only(record: &DocumentRecord) -> bool {
    !matches!(record.scope, DocumentScope::Conversation { .. })
        || record.history == Some(DocumentHistory::Latest)
}

fn is_alive(record: &DocumentRecord, at: DocumentPoint) -> bool {
    match at {
        DocumentPoint::Current => record.retired_at.is_none(),
        DocumentPoint::Seq(at) => {
            record.created_at <= at && record.retired_at.is_none_or(|retired| at < retired)
        }
    }
}

fn scope_key(scope: &DocumentScope) -> ScopeKey {
    match scope {
        DocumentScope::Session => ScopeKey::Session,
        DocumentScope::Conversation { conversation_id } => ScopeKey::Conversation(*conversation_id),
        DocumentScope::Task { task_id } => ScopeKey::Task(*task_id),
    }
}

fn addr_key(kind: &str, scope: &DocumentScope, key: &Option<String>) -> AddrKey {
    AddrKey {
        kind: kind.to_string(),
        scope: scope_key(scope),
        key: key.clone(),
    }
}

fn addr_key_record(record: &DocumentRecord) -> AddrKey {
    addr_key(&record.kind, &record.scope, &record.key)
}

fn unknown_conversation(id: i64) -> Error {
    Error::message(format!("Unknown conversation: {id}"))
}

fn scan_start(
    requested: Option<ScanOrder>,
    cursor: Option<&Cursor>,
    fallback: ScanOrder,
) -> Result<(ScanOrder, Option<i64>), Error> {
    let Some(cursor) = cursor else {
        return Ok((requested.unwrap_or(fallback), None));
    };
    let after = match cursor.get("after") {
        Some(Json::Number(number)) => number.as_i64().filter(|id| is_safe_integer(*id)),
        _ => None,
    };
    let Some(after) = after else {
        return Err(Error::message("Invalid storage cursor"));
    };
    let stored = match cursor.get("order") {
        None | Some(Json::Null) => None,
        Some(Json::String(value)) if value == "ascending" => Some(ScanOrder::Ascending),
        Some(Json::String(value)) if value == "descending" => Some(ScanOrder::Descending),
        _ => return Err(Error::message("Invalid storage cursor")),
    };
    let order = stored.unwrap_or(fallback);
    if let Some(requested) = requested {
        if requested != order {
            return Err(Error::message(format!(
                "The cursor continues a {order} scan; the query asks for {requested}"
            )));
        }
    }
    Ok((order, Some(after)))
}

fn document_after(cursor: Option<&Cursor>) -> Result<Option<i64>, Error> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    match cursor.get("after") {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Number(number)) => match number.as_i64().filter(|id| is_safe_integer(*id)) {
            Some(id) => Ok(Some(id)),
            None => Err(Error::message("Invalid storage cursor")),
        },
        Some(_) => Err(Error::message("Invalid storage cursor")),
    }
}

fn scan_ids(ids: &[i64], order: ScanOrder, after: Option<i64>) -> Vec<i64> {
    if order == ScanOrder::Ascending {
        let first = match after {
            Some(after) => upper_bound(ids, after),
            None => 0,
        };
        ids[first..].to_vec()
    } else {
        let end = match after {
            Some(after) => lower_bound(ids, after),
            None => ids.len(),
        };
        ids[..end].iter().rev().copied().collect()
    }
}

fn page<T>(
    mut values: Vec<T>,
    limit: usize,
    order: ScanOrder,
    id_of: impl Fn(&T) -> i64,
) -> Page<T> {
    if values.len() <= limit {
        return Page {
            items: values,
            next: None,
        };
    }
    values.truncate(limit);
    let Some(last) = values.last() else {
        return Page {
            items: values,
            next: None,
        };
    };
    let after = id_of(last);
    Page {
        items: values,
        next: Some(serde_json::json!({ "after": after, "order": order })),
    }
}

fn lower_bound(ids: &[i64], target: i64) -> usize {
    ids.partition_point(|id| *id < target)
}

fn upper_bound(ids: &[i64], target: i64) -> usize {
    ids.partition_point(|id| *id <= target)
}

fn upper_bound_opt(ids: &[i64], target: Option<i64>) -> usize {
    match target {
        Some(target) => upper_bound(ids, target),
        None => ids.len(),
    }
}

fn lower_bound_opt(ids: &[i64], target: Option<i64>) -> usize {
    match target {
        Some(target) => lower_bound(ids, target),
        None => 0,
    }
}

fn insert_sorted(ids: &mut Vec<i64>, id: i64) {
    if ids.last().is_some_and(|last| *last < id) {
        ids.push(id);
    } else {
        ids.insert(lower_bound(ids, id), id);
    }
}

fn remove_sorted(ids: &mut Vec<i64>, id: i64) {
    let index = lower_bound(ids, id);
    if ids.get(index) == Some(&id) {
        ids.remove(index);
    }
}

fn insert_map_id(index: &mut HashMap<i64, Vec<i64>>, key: i64, id: i64) {
    insert_sorted(index.entry(key).or_default(), id);
}

fn remove_map_id(index: &mut HashMap<i64, Vec<i64>>, key: i64, id: i64) {
    let empty = if let Some(ids) = index.get_mut(&key) {
        remove_sorted(ids, id);
        ids.is_empty()
    } else {
        false
    };
    if empty {
        index.remove(&key);
    }
}

fn below_min(id: i64, min: Option<i64>) -> bool {
    min.is_some_and(|min| id < min)
}

fn above_max(id: i64, max: Option<i64>) -> bool {
    max.is_some_and(|max| id > max)
}

fn cap_max(max: Option<i64>, at: i64) -> Option<i64> {
    Some(match max {
        Some(max) => max.min(at),
        None => at,
    })
}

fn task_bucket(buckets: &TaskBuckets, status: TaskStatus) -> &Vec<i64> {
    match status {
        TaskStatus::Pending => &buckets.pending,
        TaskStatus::Running => &buckets.running,
        TaskStatus::Waiting => &buckets.waiting,
        TaskStatus::Sleeping => &buckets.sleeping,
        TaskStatus::Completing => &buckets.completing,
        TaskStatus::Terminal => &buckets.terminal,
    }
}

fn task_bucket_mut(buckets: &mut TaskBuckets, status: TaskStatus) -> &mut Vec<i64> {
    match status {
        TaskStatus::Pending => &mut buckets.pending,
        TaskStatus::Running => &mut buckets.running,
        TaskStatus::Waiting => &mut buckets.waiting,
        TaskStatus::Sleeping => &mut buckets.sleeping,
        TaskStatus::Completing => &mut buckets.completing,
        TaskStatus::Terminal => &mut buckets.terminal,
    }
}

fn submission_bucket(buckets: &SubmissionBuckets, status: SubmissionStatus) -> &Vec<i64> {
    match status {
        SubmissionStatus::Queued => &buckets.queued,
        SubmissionStatus::Placed => &buckets.placed,
        SubmissionStatus::Done => &buckets.done,
        SubmissionStatus::Unanswered => &buckets.unanswered,
    }
}

fn submission_bucket_mut(
    buckets: &mut SubmissionBuckets,
    status: SubmissionStatus,
) -> &mut Vec<i64> {
    match status {
        SubmissionStatus::Queued => &mut buckets.queued,
        SubmissionStatus::Placed => &mut buckets.placed,
        SubmissionStatus::Done => &mut buckets.done,
        SubmissionStatus::Unanswered => &mut buckets.unanswered,
    }
}
