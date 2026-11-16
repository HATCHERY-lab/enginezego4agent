// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/keys.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use ez4a_types::{DocumentScope, StorageWrite};

pub fn task_key(id: i64) -> String {
    format!("task:{id}")
}

pub fn conversation_key(id: i64) -> String {
    format!("conv:{id}")
}

pub fn entry_key(id: i64) -> String {
    format!("entry:{id}")
}

pub fn append_key(id: i64) -> String {
    format!("entry-append:{id}")
}

pub fn submission_key(id: i64) -> String {
    format!("sub:{id}")
}

pub fn request_key(conversation_id: i64, request_id: &str) -> String {
    format!("req:{conversation_id}:{request_id}")
}

pub fn document_key(address_id: &str) -> String {
    format!("doc:{address_id}")
}

pub fn incarnation_key(id: i64) -> String {
    format!("docid:{id}")
}

pub fn scope_key(scope: &DocumentScope) -> String {
    match scope {
        DocumentScope::Session => "docscope:session".into(),
        DocumentScope::Conversation { conversation_id } => {
            format!("docscope:c{conversation_id}")
        }
        DocumentScope::Task { task_id } => format!("docscope:t{task_id}"),
    }
}

pub const ALL_KEYS: &str = "*";

pub fn table_keys(writes: &[StorageWrite]) -> Vec<String> {
    let mut keys = Vec::new();
    for write in writes {
        match write {
            StorageWrite::Conversation { value } => keys.push(conversation_key(value.id)),
            StorageWrite::Entry { value } => {
                keys.push(entry_key(value.id));
                keys.push(append_key(value.conversation_id));
            }
            StorageWrite::Task { value } => keys.push(task_key(value.id)),
            StorageWrite::Submission { value } => {
                keys.push(submission_key(value.id));
                if let Some(request_id) = &value.request_id {
                    keys.push(request_key(value.conversation_id, request_id));
                }
            }
            _ => {}
        }
    }
    keys
}
