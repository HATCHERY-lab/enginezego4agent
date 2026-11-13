// Portions derived from pi-durable 1.5.2 (packages/durable/src/storage/indexeddb.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 storage/indexeddb, MIT, see LICENSES/pi-MIT.txt.

//! In-memory object stores with the pi-durable IndexedDB layout.
//!
//! No browser and no disk. [`open`] keeps the database name and the records
//! in the process. Keys match when their JSON text matches.

use std::collections::BTreeMap;

use serde_json::Value;

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

#[derive(Debug)]
struct StoreBody {
    indexes: Vec<ObjectIndex>,
    rows: BTreeMap<String, Value>,
}

/// Process-local database. The name is kept and never used as a path.
#[derive(Debug)]
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
}
