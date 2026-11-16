//! Release probe of SQLite commit cost. Not part of the default suite.
//!
//! `cargo test --release -p ez4a-store --test growth -- --ignored --nocapture`

use std::time::Instant;

use ez4a_store::{
    ConversationRecord, EntryRecord, SqliteStorage, Storage, StorageWrite, ROOT_CONVERSATION_ID,
};
use serde_json::json;

fn entry(id: i64, text: &str) -> StorageWrite {
    StorageWrite::Entry {
        value: EntryRecord {
            id,
            conversation_id: ROOT_CONVERSATION_ID,
            kind: "message".into(),
            model: None,
            data: Some(json!({ "text": text })),
            head: None,
            edits: None,
            by_task_id: None,
        },
    }
}

fn commit_one(storage: &SqliteStorage, id: i64, text: &str) {
    let batches = vec![vec![entry(id, text)]];
    let result = storage
        .commit(&batches, &mut |_index, _seq| {})
        .expect("commit");
    assert_eq!(
        result.stored, 1,
        "commit {id} refused: {:?}",
        result.refused
    );
}

#[test]
#[ignore]
fn sqlite_commit_growth_stays_within_2x() {
    let mut path = std::env::temp_dir();
    path.push(format!("ez4a-growth-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let text = "x".repeat(500);
    let storage = SqliteStorage::open(&path).unwrap();
    storage
        .commit(
            &[vec![StorageWrite::Conversation {
                value: ConversationRecord {
                    id: ROOT_CONVERSATION_ID,
                    parent: None,
                    owner: None,
                },
            }]],
            &mut |_index, _seq| {},
        )
        .unwrap();

    let time_span = |storage: &SqliteStorage, start: i64, count: i64| {
        let began = Instant::now();
        for offset in 0..count {
            commit_one(storage, start + offset, &text);
        }
        began.elapsed()
    };

    let first = time_span(&storage, 2, 500);
    // Fill through record 2500, then time the next 500 (ids 2501..=3000).
    for id in 502..2501 {
        commit_one(&storage, id, &text);
    }
    let mid = time_span(&storage, 2501, 500);
    for id in 3001..4502 {
        commit_one(&storage, id, &text);
    }
    let last = time_span(&storage, 4502, 500);
    storage.close().unwrap();
    let _ = std::fs::remove_file(&path);

    eprintln!(
        "growth first_500_ms={} mid_500_ms={} last_500_ms={}",
        first.as_millis(),
        mid.as_millis(),
        last.as_millis()
    );
    assert!(
        last <= first * 2,
        "last 500 commits ({last:?}) were slower than twice the first 500 ({first:?})"
    );
}
