// Portions derived from pi-durable 1.5.2 (packages/durable/src/session), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use e4a_session::{
    agent_doc, compaction_fields, provider_doc, Checkpoint, CompactionSettings,
    ConversationOwnership, DocDef, DocumentFork, DocumentHistory, EntryDraft, Op, Seg, Session,
    SessionEnd, SubmissionCreate, SubmissionSettlement, SubmissionStatus, SubmissionType,
    TaskCreate, TaskState, AGENT_KIND, PROVIDER_KIND, ROOT_CONVERSATION_ID,
};
use serde_json::{json, Value};

fn user_entry(text: &str) -> EntryDraft {
    EntryDraft {
        kind: "pi.user".into(),
        model: Some(json!([{ "role": "user", "content": text }])),
        data: None,
        head: None,
        edits: None,
    }
}

fn assistant_entry(text: &str) -> EntryDraft {
    EntryDraft {
        kind: "pi.assistant".into(),
        model: Some(json!([{ "role": "assistant", "content": [{ "type": "text", "text": text }] }])),
        data: None,
        head: None,
        edits: None,
    }
}

fn note_doc() -> DocDef {
    DocDef::conversation(
        "note",
        1,
        DocumentHistory::Latest,
        DocumentFork::Current,
        || json!({ "text": "" }),
        Checkpoint::Never,
    )
}

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "e4a-session-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        Self { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let wal = PathBuf::from(format!("{}-wal", self.path.display()));
        let shm = PathBuf::from(format!("{}-shm", self.path.display()));
        let _ = fs::remove_file(wal);
        let _ = fs::remove_file(shm);
    }
}

#[tokio::test]
async fn open_creates_root_and_builtin_docs() {
    let session = Session::memory().await.unwrap();
    let root = session
        .conversation(ROOT_CONVERSATION_ID)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(root.id, ROOT_CONVERSATION_ID);
    assert!(root.parent.is_none());
    let agent = session
        .snapshot_doc(&agent_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent, json!({}));
    let provider = session
        .snapshot_doc(&provider_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .unwrap();
    assert!(provider.get("sessionId").and_then(Value::as_str).is_some());
    session.close().await.unwrap();
}

#[tokio::test]
async fn commit_appends_an_entry() {
    let session = Session::memory().await.unwrap();
    let entry = session
        .commit(|tx| tx.append_entry(ROOT_CONVERSATION_ID, user_entry("hello")))
        .await
        .unwrap();
    assert_eq!(entry.conversation_id, ROOT_CONVERSATION_ID);
    assert_eq!(entry.kind, "pi.user");
    let stored = session.entry(entry.id).await.unwrap().unwrap();
    assert_eq!(stored.id, entry.id);
    session.close().await.unwrap();
}

#[tokio::test]
async fn apply_ops_snapshot_contains_appended_text() {
    let session = Session::memory().await.unwrap();
    let op = e4a_session::append_entry(ROOT_CONVERSATION_ID, "user", "from-op").unwrap();
    session
        .commit(|tx| tx.apply_ops(&[op]))
        .await
        .unwrap();
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    let entry = view
        .borrow()
        .entries
        .first()
        .cloned()
        .expect("appended entry");
    assert_eq!(entry.kind, "pi.user");
    assert_eq!(
        entry.model,
        Some(json!([{ "role": "user", "content": "from-op" }]))
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn document_set_via_op_set() {
    let session = Session::memory().await.unwrap();
    let def = note_doc();
    session
        .commit(|tx| {
            let doc = tx.doc(&def, Some(ROOT_CONVERSATION_ID))?;
            tx.apply_doc_ops(
                doc,
                &[Op::Set(
                    vec![Seg::Key("text".into())],
                    json!("alpha"),
                )],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let value = session
        .snapshot_doc(&def, Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(value["text"], json!("alpha"));
    session.close().await.unwrap();
}

#[tokio::test]
async fn watch_sees_only_committed_state() {
    let session = Session::memory().await.unwrap();
    let mut watch = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    assert!(watch.borrow().entries.is_empty());
    session
        .commit(|tx| {
            tx.append_entry(ROOT_CONVERSATION_ID, user_entry("pending"))?;
            assert!(
                watch.borrow().entries.is_empty(),
                "watch must not see the uncommitted entry"
            );
            Ok(())
        })
        .await
        .unwrap();
    watch.changed().await.unwrap();
    let view = watch.borrow().clone();
    assert_eq!(view.entries.len(), 1);
    assert_eq!(view.entries[0].kind, "pi.user");
    session.close().await.unwrap();
}

#[tokio::test]
async fn tx_submission_roundtrip() {
    let session = Session::memory().await.unwrap();
    let (entry_id, submission_id) = session
        .commit(|tx| {
            let entry = tx.append_entry(ROOT_CONVERSATION_ID, user_entry("submit"))?;
            let created = tx.create_submission(SubmissionCreate {
                conversation_id: ROOT_CONVERSATION_ID,
                request_id: Some("req-1".into()),
                ty: SubmissionType::Input,
                status: SubmissionStatus::Queued,
                entry: None,
                answer: None,
                reason: None,
                detail: None,
                trace: None,
            })?;
            tx.place_submission(created.id, entry.id)?;
            Ok((entry.id, created.id))
        })
        .await
        .unwrap();
    let stored = session.submission(submission_id).await.unwrap().unwrap();
    assert_eq!(stored.status, SubmissionStatus::Placed);
    assert_eq!(stored.entry, Some(entry_id));
    session
        .commit(|tx| {
            let found = tx.submission(submission_id)?.expect("submission");
            assert_eq!(found.id, submission_id);
            tx.settle_submission(
                submission_id,
                SubmissionSettlement::Done { answer: entry_id },
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let done = session.submission(submission_id).await.unwrap().unwrap();
    assert_eq!(done.status, SubmissionStatus::Done);
    assert_eq!(done.answer, Some(entry_id));
    session.close().await.unwrap();
}

#[tokio::test]
async fn fork_at_entry_copies_as_of_not_tasks_and_mints_provider() {
    let session = Session::memory().await.unwrap();
    let settings = CompactionSettings {
        enabled: Some(false),
        reserve_tokens: Some(1_000),
        keep_recent_tokens: None,
        background_tokens: None,
        model_overrides: None,
    };
    let fields = compaction_fields(&settings).unwrap();
    let (at, parent_provider, task_id) = session
        .commit(|tx| {
            let agent = tx.doc(&agent_doc(), Some(ROOT_CONVERSATION_ID))?;
            tx.apply_doc_ops(
                agent,
                &[Op::Set(vec![Seg::Key("compaction".into())], fields.clone())],
            )?;
            tx.append_entry(ROOT_CONVERSATION_ID, user_entry("alpha"))?;
            let answer = tx.append_entry(ROOT_CONVERSATION_ID, assistant_entry("alpha"))?;
            let task = tx.create_task(TaskCreate {
                kind: "pi.generation".into(),
                version: 2,
                conversation_id: ROOT_CONVERSATION_ID,
                input: json!({}),
                owner: None,
                background: false,
                checkpoint: json!({ "phase": "prepare" }),
            })?;
            tx.set_task({
                let mut record = task.clone();
                record.state = TaskState::Running {
                    checkpoint: json!({ "phase": "request" }),
                };
                record
            })?;
            let provider = tx
                .doc(&provider_doc(), Some(ROOT_CONVERSATION_ID))?;
            let session_id = tx.doc_value(provider)?["sessionId"].clone();
            Ok((answer.id, session_id, task.id))
        })
        .await
        .unwrap();
    let child = session
        .commit(|tx| {
            tx.fork_conversation(ROOT_CONVERSATION_ID, at, ConversationOwnership::Ownerless)
        })
        .await
        .unwrap();
    assert_eq!(
        child.parent.map(|parent| (parent.conversation_id, parent.at)),
        Some((ROOT_CONVERSATION_ID, at))
    );
    let child_provider = session
        .snapshot_doc(&provider_doc(), Some(child.id))
        .await
        .unwrap()
        .unwrap();
    assert_ne!(child_provider["sessionId"], parent_provider);
    let child_agent = session
        .snapshot_doc(&agent_doc(), Some(child.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(child_agent["compaction"]["enabled"], json!(false));
    assert_eq!(child_agent["compaction"]["reserveTokens"], json!(1_000));
    let view = session.watch(child.id).await.unwrap().borrow().clone();
    assert_eq!(view.entries.len(), 2);
    assert!(view.entries.iter().any(|entry| entry.id == at));
    assert!(view
        .entries
        .iter()
        .all(|entry| entry.kind == "pi.user" || entry.kind == "pi.assistant"));
    let parent_task = session.task(task_id).await.unwrap().unwrap();
    assert_eq!(parent_task.conversation_id, ROOT_CONVERSATION_ID);
    session.close().await.unwrap();
}

#[tokio::test]
async fn compaction_settings_are_stored_not_run() {
    let session = Session::memory().await.unwrap();
    let settings = CompactionSettings {
        enabled: Some(true),
        reserve_tokens: Some(16_384),
        keep_recent_tokens: Some(20_000),
        background_tokens: Some(32_768),
        model_overrides: None,
    };
    session
        .commit(|tx| {
            let agent = tx.doc(&agent_doc(), Some(ROOT_CONVERSATION_ID))?;
            tx.apply_doc_ops(
                agent,
                &[Op::Set(
                    vec![Seg::Key("compaction".into())],
                    compaction_fields(&settings)?,
                )],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let agent = session
        .snapshot_doc(&agent_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(agent["compaction"]["reserveTokens"], json!(16_384));
    session.close().await.unwrap();
}

#[tokio::test]
async fn sqlite_writer_thread_roundtrip() {
    let db = TempDb::new();
    let session = Session::sqlite(&db.path).await.unwrap();
    let entry = session
        .commit(|tx| tx.append_entry(ROOT_CONVERSATION_ID, user_entry("disk")))
        .await
        .unwrap();
    session.close().await.unwrap();
    let reopened = Session::sqlite(&db.path).await.unwrap();
    let stored = reopened.entry(entry.id).await.unwrap().unwrap();
    assert_eq!(stored.kind, "pi.user");
    reopened.close().await.unwrap();
}

#[tokio::test]
async fn close_ends_cleanly() {
    let session = Session::memory().await.unwrap();
    assert_eq!(session.close().await.unwrap(), SessionEnd::Closed);
}

#[tokio::test]
async fn replay_fork_fixture_structure() {
    let events = load_jsonl("fork.jsonl");
    let fork_at = events
        .iter()
        .find(|event| event["kind"] == "fork-at")
        .expect("fork-at");
    assert_eq!(fork_at["value"], json!(10));
    let fork_commit = events
        .iter()
        .find(|event| {
            event["kind"] == "commit"
                && event["value"]["changes"]
                    .as_array()
                    .map(|changes| {
                        changes.iter().any(|change| {
                            change["type"] == "conversation" && change["value"]["parent"].is_object()
                        })
                    })
                    .unwrap_or(false)
        })
        .expect("fork commit");
    let changes = fork_commit["value"]["changes"].as_array().unwrap();
    assert!(changes.iter().any(|change| {
        change["type"] == "document.copy" && change["record"]["kind"] == AGENT_KIND
    }));
    assert!(changes.iter().any(|change| {
        change["type"] == "document"
            && change["kind"] == "change"
            && change["record"]["kind"] == PROVIDER_KIND
    }));
    assert!(!changes.iter().any(|change| change["type"] == "task"));
    let parent_provider = events
        .iter()
        .find_map(|event| provider_session_id(event, 1))
        .expect("parent provider");
    let child_provider = events
        .iter()
        .find_map(|event| provider_session_id(event, 11))
        .expect("child provider");
    assert_ne!(parent_provider, child_provider);
}

#[tokio::test]
async fn replay_abort_fixture_structure() {
    let events = load_jsonl("abort.jsonl");
    assert!(events.iter().any(|event| event["kind"] == "commit"));
    let has_user = events.iter().any(|event| {
        event["kind"] == "commit"
            && event["value"]["changes"]
                .as_array()
                .map(|changes| {
                    changes.iter().any(|change| {
                        change["type"] == "entry" && change["value"]["kind"] == "pi.user"
                    })
                })
                .unwrap_or(false)
    });
    assert!(has_user);
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| event["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"watch"));
    assert!(!events.iter().any(|event| event["kind"] == "sse"));
}

fn load_jsonl(name: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{line}: {error}")))
        .collect()
}

fn provider_session_id(event: &Value, conversation_id: i64) -> Option<String> {
    let changes = event.get("value")?.get("changes")?.as_array()?;
    for change in changes {
        let Some(record) = change.get("record") else {
            continue;
        };
        if record.get("kind").and_then(Value::as_str) == Some(PROVIDER_KIND)
            && record["scope"]["conversationId"].as_i64() == Some(conversation_id)
        {
            if let Some(session_id) = change["value"]["sessionId"].as_str() {
                return Some(session_id.to_string());
            }
        }
    }
    None
}
