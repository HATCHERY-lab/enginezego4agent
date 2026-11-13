// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/session.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use e4a_store::{MemoryStorage, SqliteStorage, Storage};
use e4a_types::{
    ConversationRecord, DocumentAddress, DocumentQuery, DocumentScope, EntryQuery, EntryRecord,
    Json, ScanOrder, TaskQuery, ROOT_CONVERSATION_ID,
};
use tokio::sync::{watch, Mutex as AsyncMutex};

use crate::docs::{address_id, DocDef};
use crate::keys::{conversation_key, document_key, entry_key, submission_key, task_key};
use crate::store::StoreHandle;
use crate::tx::{empty_scope, Adopted, AdoptedDocument, Tx};
use crate::view::{window_entries, ConversationView, ConversationWindow};
use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEnd {
    Closed,
    Failed { error: String },
}

enum Status {
    Open,
    Failed(String),
    Closed,
}

struct Published {
    conversations: HashMap<i64, ConversationRecord>,
    entries: HashMap<i64, Vec<EntryRecord>>,
    documents: HashMap<String, AdoptedDocument>,
}

struct Inner {
    store: StoreHandle,
    line: AsyncMutex<()>,
    status: Mutex<Status>,
    published: Mutex<Published>,
    views: Mutex<HashMap<i64, watch::Sender<ConversationView>>>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    pub async fn memory() -> Result<Self, Error> {
        Self::open(|| Ok(MemoryStorage::new())).await
    }

    pub async fn sqlite(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = PathBuf::from(path.as_ref());
        Self::open(move || SqliteStorage::open(&path)).await
    }

    async fn open<S, F>(factory: F) -> Result<Self, Error>
    where
        S: Storage + Send + 'static,
        F: FnOnce() -> Result<S, e4a_store::Error> + Send + 'static,
    {
        let store = StoreHandle::spawn(factory)?;
        let session = Self {
            inner: Arc::new(Inner {
                store,
                line: AsyncMutex::new(()),
                status: Mutex::new(Status::Open),
                published: Mutex::new(Published {
                    conversations: HashMap::new(),
                    entries: HashMap::new(),
                    documents: HashMap::new(),
                }),
                views: Mutex::new(HashMap::new()),
                now: Arc::new(wall_clock),
            }),
        };
        session.ensure_root().await?;
        Ok(session)
    }

    pub async fn commit<T>(
        &self,
        change: impl FnOnce(&mut Tx) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let (value, reply) = {
            let _line = self.inner.line.lock().await;
            self.assert_usable()?;
            let mut tx = Tx::new(
                self.inner.store.clone(),
                Arc::clone(&self.inner.now),
                empty_scope(),
            );
            let value = match change(&mut tx) {
                Ok(value) => value,
                Err(error) => return Err(error),
            };
            let adopted = tx.settle()?;
            if adopted.writes.is_empty() {
                return Ok(value);
            }
            let reply = self.admit(adopted)?;
            (value, Some(reply))
        };
        if let Some(reply) = reply {
            match reply.await {
                Ok(Ok(_seq)) => {}
                Ok(Err(error)) => {
                    self.fail(&error);
                    return Err(error);
                }
                Err(_) => {
                    let error = Error::message("commit publication dropped");
                    self.fail(&error);
                    return Err(error);
                }
            }
        }
        self.assert_usable()?;
        Ok(value)
    }

    pub async fn watch(
        &self,
        conversation_id: i64,
    ) -> Result<watch::Receiver<ConversationView>, Error> {
        self.watch_window(conversation_id, ConversationWindow::default())
            .await
    }

    pub async fn watch_window(
        &self,
        conversation_id: i64,
        window: ConversationWindow,
    ) -> Result<watch::Receiver<ConversationView>, Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        self.inner.store.drained()?;
        self.load_conversation(conversation_id)?;
        let view = self.conversation_view(conversation_id, window)?;
        let mut views = lock_std(&self.inner.views)?;
        if let Some(sender) = views.get(&conversation_id) {
            let _ = sender.send(view);
            return Ok(sender.subscribe());
        }
        let (sender, receiver) = watch::channel(view);
        views.insert(conversation_id, sender);
        Ok(receiver)
    }

    pub async fn snapshot_doc(
        &self,
        def: &DocDef,
        owner: Option<i64>,
    ) -> Result<Option<Json>, Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        let address = def.address(owner)?;
        self.inner.store.touch(&document_key(&address_id(&address)))?;
        let published = lock_std(&self.inner.published)?;
        Ok(published
            .documents
            .get(&address_id(&address))
            .map(|document| document.value.clone()))
    }

    pub async fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        self.inner.store.touch(&conversation_key(id))?;
        self.inner
            .store
            .call(move |storage| storage.conversation(id).map_err(Error::from))
    }

    pub async fn entry(&self, id: i64) -> Result<Option<EntryRecord>, Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        self.inner.store.touch(&entry_key(id))?;
        self.inner.store.call(move |storage| {
            Ok(storage.entry(id).map_err(Error::from)?.map(|hit| hit.entry))
        })
    }

    pub async fn task(&self, id: i64) -> Result<Option<e4a_types::TaskRecord>, Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        self.inner.store.touch(&task_key(id))?;
        self.inner
            .store
            .call(move |storage| storage.task(id).map_err(Error::from))
    }

    pub async fn submission(&self, id: i64) -> Result<Option<e4a_types::SubmissionRecord>, Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        self.inner.store.touch(&submission_key(id))?;
        self.inner
            .store
            .call(move |storage| storage.submission(id).map_err(Error::from))
    }

    /// Permanently delete a conversation on the store writer thread.
    ///
    /// Root conversation 1 is refused. Children that point at `id` as parent stay;
    /// this does not recurse.
    pub async fn delete_conversation(&self, id: i64) -> Result<(), Error> {
        let _line = self.inner.line.lock().await;
        self.assert_usable()?;
        self.inner.store.drained()?;
        let query = TaskQuery {
            conversation_id: Some(id),
            ..TaskQuery::default()
        };
        let task_ids: HashSet<i64> = self.inner.store.call(move |storage| {
            let page = storage
                .scan_tasks(&query, usize::MAX, None)
                .map_err(Error::from)?;
            Ok(page.items.into_iter().map(|task| task.id).collect())
        })?;
        self.inner
            .store
            .call(move |storage| storage.delete_conversation(id).map_err(Error::from))?;
        {
            let mut published = lock_std(&self.inner.published)?;
            published.conversations.remove(&id);
            published.entries.remove(&id);
            published.documents.retain(|_, document| match document.record.scope {
                DocumentScope::Conversation { conversation_id } => conversation_id != id,
                DocumentScope::Task { task_id } => !task_ids.contains(&task_id),
                DocumentScope::Session => true,
            });
        }
        {
            let mut views = lock_std(&self.inner.views)?;
            views.remove(&id);
        }
        refresh_views(&self.inner)
    }

    pub async fn close(&self) -> Result<SessionEnd, Error> {
        {
            let _line = self.inner.line.lock().await;
            let mut status = lock_std(&self.inner.status)?;
            if matches!(*status, Status::Closed) {
                return Ok(SessionEnd::Closed);
            }
            if let Status::Failed(error) = &*status {
                let error = error.clone();
                *status = Status::Closed;
                drop(status);
                let _ = self.inner.store.close().await;
                return Ok(SessionEnd::Failed { error });
            }
            *status = Status::Closed;
        }
        self.inner.store.drained()?;
        self.inner.store.close().await?;
        Ok(SessionEnd::Closed)
    }

    async fn ensure_root(&self) -> Result<(), Error> {
        let existing = {
            let _line = self.inner.line.lock().await;
            self.inner
                .store
                .call(|storage| storage.conversation(ROOT_CONVERSATION_ID).map_err(Error::from))?
        };
        if existing.is_some() {
            let _line = self.inner.line.lock().await;
            self.load_conversation(ROOT_CONVERSATION_ID)?;
            return Ok(());
        }
        self.commit(|tx| {
            tx.create_root_conversation()?;
            Ok(())
        })
        .await
    }

    fn admit(
        &self,
        adopted: Adopted,
    ) -> Result<tokio::sync::oneshot::Receiver<Result<i64, Error>>, Error> {
        let keys = adopted.keys.clone();
        let inner = Arc::clone(&self.inner);
        let (_id, reply) = self.inner.store.admit(
            adopted.writes.clone(),
            keys,
            Box::new(move |seq| publish(&inner, seq, adopted)),
        )?;
        Ok(reply)
    }

    fn load_conversation(&self, conversation_id: i64) -> Result<(), Error> {
        if !self.publish_conversation(conversation_id)? {
            return Err(Error::message(format!(
                "Conversation {conversation_id} does not exist"
            )));
        }
        Ok(())
    }

    fn publish_conversation(&self, conversation_id: i64) -> Result<bool, Error> {
        let conversation = self
            .inner
            .store
            .call(move |storage| storage.conversation(conversation_id).map_err(Error::from))?;
        let Some(conversation) = conversation else {
            return Ok(false);
        };
        if let Some(parent) = conversation.parent {
            // Children that point at a deleted parent stay; do not require that row.
            self.publish_conversation(parent.conversation_id)?;
        }
        let query = EntryQuery {
            conversation_id,
            min_entry_id: None,
            max_entry_id: None,
            order: Some(ScanOrder::Ascending),
        };
        let page = self.inner.store.call(move |storage| {
            storage
                .scan_entries(&query, usize::MAX, None)
                .map_err(Error::from)
        })?;
        // Storage.scan_entries walks the parent chain; publication keeps each
        // conversation's own rows so collect_visible_entries can walk once.
        let own: Vec<EntryRecord> = page
            .items
            .into_iter()
            .filter(|entry| entry.conversation_id == conversation_id)
            .collect();
        let documents = conversation_documents(&self.inner.store, conversation_id)?;
        let mut published = lock_std(&self.inner.published)?;
        published
            .conversations
            .insert(conversation_id, conversation);
        published.entries.insert(conversation_id, own);
        for document in documents {
            published
                .documents
                .insert(document.address_id.clone(), document);
        }
        Ok(true)
    }

    fn conversation_view(
        &self,
        conversation_id: i64,
        window: ConversationWindow,
    ) -> Result<ConversationView, Error> {
        let published = lock_std(&self.inner.published)?;
        view_from_published(&published, conversation_id, window)
    }

    fn assert_usable(&self) -> Result<(), Error> {
        match &*lock_std(&self.inner.status)? {
            Status::Open => Ok(()),
            Status::Failed(error) => Err(Error::message(format!("Session failed: {error}"))),
            Status::Closed => Err(Error::message("Session is closed")),
        }
    }

    fn fail(&self, error: &Error) {
        if let Ok(mut status) = lock_std(&self.inner.status) {
            if matches!(*status, Status::Open) {
                *status = Status::Failed(error.to_string());
            }
        }
    }
}

fn publish(inner: &Arc<Inner>, seq: i64, mut adopted: Adopted) -> Result<(), Error> {
    adopted.stamp_seq(seq);
    {
        let mut published = lock_std(&inner.published)?;
        for conversation in adopted.conversations {
            published
                .conversations
                .insert(conversation.id, conversation);
        }
        for entry in adopted.entries {
            published
                .entries
                .entry(entry.conversation_id)
                .or_default()
                .push(entry);
        }
        for document in adopted.documents {
            published
                .documents
                .insert(document.address_id.clone(), document);
        }
        for address_id in adopted.retired {
            published.documents.remove(&address_id);
        }
    }
    refresh_views(inner)
}

fn refresh_views(inner: &Arc<Inner>) -> Result<(), Error> {
    let published = lock_std(&inner.published)?;
    let views = lock_std(&inner.views)?;
    for (conversation_id, sender) in views.iter() {
        if let Ok(view) = view_from_published(&published, *conversation_id, ConversationWindow::default())
        {
            let _ = sender.send(view);
        }
    }
    Ok(())
}

fn view_from_published(
    published: &Published,
    conversation_id: i64,
    window: ConversationWindow,
) -> Result<ConversationView, Error> {
    let conversation = published
        .conversations
        .get(&conversation_id)
        .cloned()
        .ok_or_else(|| Error::message(format!("Conversation {conversation_id} does not exist")))?;
    let mut entries = Vec::new();
    collect_visible_entries(published, conversation_id, None, &mut entries, &mut HashMap::new())?;
    let (entries, before) = window_entries(&entries, window);
    let mut docs = BTreeMap::new();
    for document in published.documents.values() {
        if let DocumentScope::Conversation {
            conversation_id: owner,
        } = document.record.scope
        {
            if owner == conversation_id {
                docs.insert(document.record.kind.clone(), document.value.clone());
            }
        }
    }
    Ok(ConversationView {
        conversation,
        entries,
        before,
        docs,
    })
}

fn collect_visible_entries(
    published: &Published,
    conversation_id: i64,
    until: Option<i64>,
    out: &mut Vec<EntryRecord>,
    seen: &mut HashMap<i64, ()>,
) -> Result<(), Error> {
    if seen.insert(conversation_id, ()).is_some() {
        return Err(Error::message(format!(
            "Conversation {conversation_id} has a cyclic parent"
        )));
    }
    let conversation = published
        .conversations
        .get(&conversation_id)
        .ok_or_else(|| Error::message(format!("Conversation {conversation_id} does not exist")))?;
    if let Some(parent) = conversation.parent {
        if published
            .conversations
            .contains_key(&parent.conversation_id)
        {
            let parent_until = match until {
                Some(at) if at < parent.at => at,
                _ => parent.at,
            };
            collect_visible_entries(
                published,
                parent.conversation_id,
                Some(parent_until),
                out,
                seen,
            )?;
        }
    }
    if let Some(entries) = published.entries.get(&conversation_id) {
        for entry in entries {
            if until.map(|at| entry.id <= at).unwrap_or(true) {
                out.push(entry.clone());
            }
        }
    }
    Ok(())
}

fn conversation_documents(
    store: &StoreHandle,
    conversation_id: i64,
) -> Result<Vec<AdoptedDocument>, Error> {
    let query = DocumentQuery {
        scope: DocumentScope::Conversation { conversation_id },
        at: e4a_types::DocumentPoint::Current,
        kind: None,
    };
    let page = store.call(move |storage| {
        storage
            .scan_documents(&query, 256, None)
            .map_err(Error::from)
    })?;
    let mut documents = Vec::new();
    for record in page.items {
        let id = record.id;
        let stored = store.call(move |storage| {
            storage
                .document(id, e4a_types::DocumentPoint::Current)
                .map_err(Error::from)
        })?;
        if let Some(stored) = stored {
            let address = DocumentAddress {
                kind: stored.record.kind.clone(),
                scope: stored.record.scope,
                key: stored.record.key.clone(),
            };
            documents.push(AdoptedDocument {
                address_id: address_id(&address),
                record: stored.record,
                value: stored.value,
            });
        }
    }
    Ok(documents)
}

fn wall_clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn lock_std<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, Error> {
    mutex
        .lock()
        .map_err(|_| Error::message("session state poisoned"))
}
