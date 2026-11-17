// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/forks.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use std::collections::HashSet;

use ez4a_types::{
    Cursor, DocumentAddress, DocumentCopySource, DocumentCreate, DocumentFork, DocumentPoint,
    DocumentQuery, DocumentRecord, DocumentScope, Json, StorageWrite,
};

use crate::docs::address_id;
use crate::store::StoreHandle;
use crate::Error;

const SCAN_PAGE_SIZE: usize = 256;

pub(crate) struct ForkDocumentCopy {
    pub record: DocumentCreate,
    pub source: DocumentCopySource,
    pub value: Json,
    pub version: i64,
}

pub(crate) fn prepare_fork_document_copies(
    store: &StoreHandle,
    parent_conversation_id: i64,
    at: i64,
    child_conversation_id: i64,
) -> Result<Vec<ForkDocumentCopy>, Error> {
    let hit = store.call(move |storage| {
        storage
            .entry_in(parent_conversation_id, at)
            .map_err(Error::from)
    })?;
    let Some(hit) = hit else {
        return Err(Error::message(format!(
            "Entry {at} is not visible from conversation {parent_conversation_id}"
        )));
    };
    let mut copies = Vec::new();
    let mut copied_addresses = HashSet::new();
    collect_copies(
        store,
        DocumentScope::Conversation {
            conversation_id: hit.entry.conversation_id,
        },
        DocumentPoint::Seq(hit.commit_seq),
        DocumentFork::AsOf,
        child_conversation_id,
        &mut copies,
        &mut copied_addresses,
    )?;
    collect_copies(
        store,
        DocumentScope::Conversation {
            conversation_id: parent_conversation_id,
        },
        DocumentPoint::Current,
        DocumentFork::Current,
        child_conversation_id,
        &mut copies,
        &mut copied_addresses,
    )?;
    Ok(copies)
}

fn collect_copies(
    store: &StoreHandle,
    scope: DocumentScope,
    at: DocumentPoint,
    policy: DocumentFork,
    child_conversation_id: i64,
    copies: &mut Vec<ForkDocumentCopy>,
    copied_addresses: &mut HashSet<String>,
) -> Result<(), Error> {
    let mut cursor: Option<Cursor> = None;
    loop {
        let query = DocumentQuery {
            scope,
            at,
            kind: None,
        };
        let page_cursor = cursor.clone();
        let page = store.call(move |storage| {
            storage
                .scan_documents(&query, SCAN_PAGE_SIZE, page_cursor.as_ref())
                .map_err(Error::from)
        })?;
        for source in page.items {
            if !matches!(source.scope, DocumentScope::Conversation { .. })
                || source.fork != Some(policy)
            {
                continue;
            }
            let id = store.call(|storage| storage.mint_id().map_err(Error::from))?;
            let record = child_record(id, child_conversation_id, &source);
            let copy_address = address_id(&DocumentAddress {
                kind: record.kind.clone(),
                scope: record.scope,
                key: record.key.clone(),
            });
            if !copied_addresses.insert(copy_address) {
                let member = match &record.key {
                    Some(key) => format!("{}/{}", record.kind, key),
                    None => record.kind.clone(),
                };
                return Err(Error::message(format!(
                    "Fork selects multiple source documents for {member}"
                )));
            }
            let source_id = source.id;
            let stored = store.call(move |storage| {
                storage
                    .document(source_id, at)
                    .map_err(Error::from)
            })?;
            let Some(stored) = stored else {
                return Err(Error::message(format!(
                    "Fork source document {source_id} cannot be read"
                )));
            };
            copies.push(ForkDocumentCopy {
                record,
                source: DocumentCopySource { id: source.id, at },
                value: stored.value,
                version: stored.version,
            });
        }
        match page.next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(())
}

fn child_record(id: i64, child_conversation_id: i64, source: &DocumentRecord) -> DocumentCreate {
    DocumentCreate {
        id,
        kind: source.kind.clone(),
        key: source.key.clone(),
        scope: DocumentScope::Conversation {
            conversation_id: child_conversation_id,
        },
        history: source.history,
        fork: source.fork,
    }
}

pub(crate) fn copy_write(copy: &ForkDocumentCopy) -> StorageWrite {
    StorageWrite::DocumentCopy {
        record: copy.record.clone(),
        source: copy.source.clone(),
    }
}
