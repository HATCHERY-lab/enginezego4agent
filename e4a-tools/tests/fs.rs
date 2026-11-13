use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use e4a_harness::{Generation, TurnInput, TurnKind, MODEL_CALL_CAP};
use e4a_llm::{FauxLlm, ToolCall, ToolSchema};
use e4a_session::{Session, TaskState, ROOT_CONVERSATION_ID};
use e4a_tools::{run_until_answer, run_waiting_tool, Workspace};
use e4a_types::{TaskOutcome, TaskStatus};
use serde_json::{json, Value};

fn scratch_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "e4a-tools-{}-{}-{}",
        std::process::id(),
        nanos,
        n
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        Self(scratch_dir())
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Gone(PathBuf);

impl Drop for Gone {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn workspace() -> (Scratch, Workspace) {
    let scratch = Scratch::new();
    let workspace = Workspace::new(scratch.path()).unwrap();
    (scratch, workspace)
}

async fn tool_turn(session: &Session, name: &str, arguments: Value) -> i64 {
    let llm = FauxLlm::text("").with_tool_call(ToolCall {
        id: "call-1".into(),
        name: name.into(),
        arguments,
    });
    let outcome = Generation::turn(
        session,
        &llm,
        &TurnInput {
            conversation_id: ROOT_CONVERSATION_ID,
            user_text: format!("use {name}"),
            tools: vec![ToolSchema {
                name: name.into(),
                description: "filesystem".into(),
                parameters: json!({ "type": "object" }),
            }],
        },
    )
    .await
    .unwrap();
    assert!(matches!(outcome.kind, TurnKind::WaitingTool { .. }));
    assert_eq!(llm.calls(), 1);
    outcome.task_id
}

#[test]
fn write_read_edit_round_trip() {
    let (scratch, ws) = workspace();
    let wrote = ws.call(
        "write",
        &json!({ "path": "dir/note.txt", "content": "alpha beta" }),
    );
    assert!(!wrote.is_error, "{}", wrote.content);
    assert_eq!(
        fs::read_to_string(scratch.path().join("dir/note.txt")).unwrap(),
        "alpha beta"
    );
    let read = ws.call("read", &json!({ "path": "dir/note.txt" }));
    assert_eq!(read.content, "alpha beta");
    assert!(!read.is_error);
    let edited = ws.call(
        "edit",
        &json!({ "path": "dir/note.txt", "old": "beta", "new": "gamma" }),
    );
    assert!(!edited.is_error, "{}", edited.content);
    assert_eq!(
        fs::read_to_string(scratch.path().join("dir/note.txt")).unwrap(),
        "alpha gamma"
    );
    let again = ws.call(
        "write",
        &json!({ "path": "dir/note.txt", "content": "overwritten" }),
    );
    assert!(!again.is_error, "{}", again.content);
    assert_eq!(
        fs::read_to_string(scratch.path().join("dir/note.txt")).unwrap(),
        "overwritten"
    );
    let inside = scratch.path().join("abs.txt");
    let absolute = ws.call(
        "write",
        &json!({ "path": inside.to_str().unwrap(), "content": "in" }),
    );
    assert!(!absolute.is_error, "{}", absolute.content);
    assert_eq!(fs::read_to_string(inside).unwrap(), "in");
}

#[test]
fn edit_rejects_zero_or_many_matches() {
    let (scratch, ws) = workspace();
    let missing = ws.call(
        "edit",
        &json!({ "path": "nope.txt", "old": "a", "new": "b" }),
    );
    assert!(missing.is_error);
    assert!(!scratch.path().join("nope.txt").exists());

    fs::write(scratch.path().join("n.txt"), "one").unwrap();
    let absent = ws.call(
        "edit",
        &json!({ "path": "n.txt", "old": "z", "new": "q" }),
    );
    assert!(absent.is_error);
    assert!(absent.content.contains("not found"));
    assert_eq!(fs::read_to_string(scratch.path().join("n.txt")).unwrap(), "one");

    let empty = ws.call(
        "edit",
        &json!({ "path": "n.txt", "old": "", "new": "x" }),
    );
    assert!(empty.is_error);
    assert_eq!(fs::read_to_string(scratch.path().join("n.txt")).unwrap(), "one");

    fs::write(scratch.path().join("n.txt"), "one one").unwrap();
    let many = ws.call(
        "edit",
        &json!({ "path": "n.txt", "old": "one", "new": "two" }),
    );
    assert!(many.is_error);
    assert!(many.content.contains("2 times"));
    assert_eq!(
        fs::read_to_string(scratch.path().join("n.txt")).unwrap(),
        "one one"
    );
}

#[test]
fn read_rejects_oversize_and_non_utf8() {
    let (scratch, ws) = workspace();
    let max = "a".repeat(256 * 1024);
    fs::write(scratch.path().join("max.txt"), &max).unwrap();
    let ok = ws.call("read", &json!({ "path": "max.txt" }));
    assert!(!ok.is_error, "{}", ok.content);
    assert_eq!(ok.content.len(), 256 * 1024);

    let big = "a".repeat(256 * 1024 + 1);
    fs::write(scratch.path().join("big.txt"), &big).unwrap();
    let over = ws.call("read", &json!({ "path": "big.txt" }));
    assert!(over.is_error);
    assert!(over.content.contains("262144"));

    fs::write(scratch.path().join("bin.txt"), [0xff, 0xfe]).unwrap();
    let binary = ws.call("read", &json!({ "path": "bin.txt" }));
    assert!(binary.is_error);
    assert!(binary.content.contains("utf-8"));
}

#[test]
fn unknown_tool_is_an_error_result() {
    let (_scratch, ws) = workspace();
    let effect = ws.call("bash", &json!({ "cmd": "whoami" }));
    assert!(effect.is_error);
    assert!(effect.content.contains("unknown tool"));
    let effect = ws.call("powershell", &json!({}));
    assert!(effect.is_error);
    assert!(effect.content.contains("unknown tool"));
}

#[test]
fn path_jail_refuses_parent_and_absolute() {
    let (scratch, ws) = workspace();
    let outside_dir = Scratch::new();
    let name = format!("e4a-rel-{}.txt", std::process::id());
    let via_parent = Gone(ws.root().parent().unwrap().join(&name));
    let relative = ws.call(
        "write",
        &json!({ "path": format!("../{name}"), "content": "pwned" }),
    );
    assert!(relative.is_error, "{}", relative.content);
    assert!(relative.content.contains("escapes the workspace"));
    assert!(!via_parent.0.exists(), "wrote {}", via_parent.0.display());

    let nested = format!("missing/../../{name}");
    let nested_effect = ws.call(
        "write",
        &json!({ "path": nested, "content": "pwned" }),
    );
    assert!(nested_effect.is_error, "{}", nested_effect.content);
    assert!(!via_parent.0.exists());

    let target = outside_dir.path().join("pwned.txt");
    let absolute = ws.call(
        "write",
        &json!({ "path": target.to_str().unwrap(), "content": "pwned" }),
    );
    assert!(absolute.is_error, "{}", absolute.content);
    assert!(absolute.content.contains("escapes the workspace"));
    assert!(!target.exists(), "wrote {}", target.display());
    assert!(!scratch.path().join("pwned.txt").exists());
}

#[tokio::test]
async fn run_waiting_tool_executes_read_write_edit() {
    let (scratch, ws) = workspace();
    let session = Session::memory().await.unwrap();
    let write_id = tool_turn(
        &session,
        "write",
        json!({ "path": "note.txt", "content": "hello" }),
    )
    .await;
    let wrote = run_waiting_tool(&session, write_id, &ws).await.unwrap();
    assert!(!wrote.is_error, "{}", wrote.content);
    assert_eq!(wrote.name, "write");
    assert_eq!(wrote.content, "wrote note.txt");
    assert_eq!(fs::read_to_string(scratch.path().join("note.txt")).unwrap(), "hello");
    let write_entry = session.entry(wrote.entry_id).await.unwrap().unwrap();
    assert_eq!(write_entry.kind, "pi.toolResult");
    let model = write_entry.model.unwrap();
    assert_eq!(model[0]["role"], json!("tool"));
    assert_eq!(model[0]["toolCallId"], json!("call-1"));
    assert_eq!(model[0]["name"], json!("write"));
    assert_eq!(model[0]["isError"], json!(false));
    assert_eq!(model[0]["content"], json!("wrote note.txt"));
    let tool = session.task(wrote.tool_task_id).await.unwrap().unwrap();
    assert!(tool.ended_at.is_some());
    match tool.state {
        TaskState::Terminal {
            outcome: TaskOutcome::Completed { result },
        } => assert_eq!(result["entryId"], json!(wrote.entry_id)),
        other => panic!("expected completed tool, got {other:?}"),
    }
    let parent = session.task(write_id).await.unwrap().unwrap();
    assert_eq!(parent.state.status(), TaskStatus::Waiting);

    let read_id = tool_turn(&session, "read", json!({ "path": "note.txt" })).await;
    let read = run_waiting_tool(&session, read_id, &ws).await.unwrap();
    assert!(!read.is_error, "{}", read.content);
    assert_eq!(read.content, "hello");
    let read_entry = session.entry(read.entry_id).await.unwrap().unwrap();
    assert_eq!(read_entry.model.unwrap()[0]["content"], json!("hello"));

    let edit_id = tool_turn(
        &session,
        "edit",
        json!({ "path": "note.txt", "old": "hello", "new": "world" }),
    )
    .await;
    let edited = run_waiting_tool(&session, edit_id, &ws).await.unwrap();
    assert!(!edited.is_error, "{}", edited.content);
    assert_eq!(edited.content, "edited note.txt");
    assert_eq!(fs::read_to_string(scratch.path().join("note.txt")).unwrap(), "world");
    assert_eq!(
        session.task(write_id).await.unwrap().unwrap().state.status(),
        TaskStatus::Waiting
    );
    let view = session.watch(ROOT_CONVERSATION_ID).await.unwrap();
    assert!(view.borrow().entries.iter().any(|entry| {
        entry.id == wrote.entry_id && entry.kind == "pi.toolResult"
    }));
    session.close().await.unwrap();
}

#[tokio::test]
async fn waiting_tool_refuses_paths_outside_the_workspace() {
    let (_scratch, ws) = workspace();
    let outside_dir = Scratch::new();
    let session = Session::memory().await.unwrap();

    let name = format!("e4a-rel-{}-{}.txt", std::process::id(), line!());
    let via_parent = Gone(ws.root().parent().unwrap().join(&name));
    let relative_id = tool_turn(
        &session,
        "write",
        json!({ "path": format!("../{name}"), "content": "pwned" }),
    )
    .await;
    let relative = run_waiting_tool(&session, relative_id, &ws).await.unwrap();
    assert!(relative.is_error, "{}", relative.content);
    assert!(relative.content.contains("escapes the workspace"));
    assert!(!via_parent.0.exists(), "wrote {}", via_parent.0.display());
    let relative_entry = session.entry(relative.entry_id).await.unwrap().unwrap();
    assert_eq!(relative_entry.kind, "pi.toolResult");
    assert_eq!(relative_entry.model.unwrap()[0]["isError"], json!(true));
    match session.task(relative.tool_task_id).await.unwrap().unwrap().state {
        TaskState::Terminal {
            outcome: TaskOutcome::Failed { error, .. },
        } => assert_eq!(error.message, relative.content),
        other => panic!("expected failed tool, got {other:?}"),
    }
    assert_eq!(
        session.task(relative_id).await.unwrap().unwrap().state.status(),
        TaskStatus::Waiting
    );

    let target = outside_dir.path().join("pwned.txt");
    let absolute_id = tool_turn(
        &session,
        "write",
        json!({ "path": target.to_str().unwrap(), "content": "pwned" }),
    )
    .await;
    let absolute = run_waiting_tool(&session, absolute_id, &ws).await.unwrap();
    assert!(absolute.is_error, "{}", absolute.content);
    assert!(absolute.content.contains("escapes the workspace"));
    assert!(!target.exists(), "wrote {}", target.display());
    assert!(session.task(absolute.tool_task_id).await.unwrap().unwrap().ended_at.is_some());
    session.close().await.unwrap();
}

#[tokio::test]
async fn unknown_tool_commits_an_error_result() {
    let (_scratch, ws) = workspace();
    let session = Session::memory().await.unwrap();
    let generation_id = tool_turn(&session, "bash", json!({ "path": "x" })).await;
    let outcome = run_waiting_tool(&session, generation_id, &ws).await.unwrap();
    assert!(outcome.is_error);
    assert!(outcome.content.contains("unknown tool bash"));
    let entry = session.entry(outcome.entry_id).await.unwrap().unwrap();
    assert_eq!(entry.kind, "pi.toolResult");
    assert_eq!(entry.model.unwrap()[0]["isError"], json!(true));
    assert!(matches!(
        session.task(outcome.tool_task_id).await.unwrap().unwrap().state,
        TaskState::Terminal {
            outcome: TaskOutcome::Failed { .. }
        }
    ));
    let answered = Generation::turn(
        &session,
        &FauxLlm::text("done"),
        &TurnInput {
            conversation_id: ROOT_CONVERSATION_ID,
            user_text: "no tool".into(),
            tools: Vec::new(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(answered.kind, TurnKind::Answered { .. }));
    let error = run_waiting_tool(&session, answered.task_id, &ws)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not waiting"));
    session.close().await.unwrap();
}

fn offered(name: &str) -> TurnInput {
    TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: format!("use {name}"),
        tools: vec![ToolSchema {
            name: name.into(),
            description: "filesystem".into(),
            parameters: json!({ "type": "object" }),
        }],
    }
}

#[tokio::test]
async fn continue_after_write_answers_with_the_file() {
    let (scratch, ws) = workspace();
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("")
        .with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "write".into(),
            arguments: json!({ "path": "note.txt", "content": "hello" }),
        })
        .then(FauxLlm::text("wrote it"));
    let outcome = Generation::turn(&session, &llm, &offered("write"))
        .await
        .unwrap();
    assert!(matches!(outcome.kind, TurnKind::WaitingTool { .. }));
    let generation = Generation::bind(outcome.task_id, ROOT_CONVERSATION_ID);
    let early = generation
        .continue_after_tool(&session, &llm)
        .await
        .unwrap_err();
    assert!(early.to_string().contains("still waiting"), "{early}");
    assert_eq!(llm.calls(), 1);
    assert!(!scratch.path().join("note.txt").exists());
    assert_eq!(
        session.task(outcome.task_id).await.unwrap().unwrap().state.status(),
        TaskStatus::Waiting
    );

    let wrote = run_waiting_tool(&session, outcome.task_id, &ws)
        .await
        .unwrap();
    assert!(!wrote.is_error, "{}", wrote.content);
    assert_eq!(
        fs::read_to_string(scratch.path().join("note.txt")).unwrap(),
        "hello"
    );

    let done = generation
        .continue_after_tool(&session, &llm)
        .await
        .unwrap();
    let TurnKind::Answered { entry_id } = done.kind else {
        panic!("expected an answer, got {:?}", done.kind);
    };
    assert!(done.called_llm);
    assert_eq!(done.failed_attempts, 0);
    assert_eq!(llm.calls(), 2);
    let stored = session.entry(entry_id).await.unwrap().unwrap();
    assert_eq!(stored.kind, "pi.assistant");
    assert_eq!(
        stored.model.unwrap()[0]["content"][0]["text"],
        json!("wrote it")
    );
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].messages.len(), 3);
    assert_eq!(seen[1].messages[1].tool_calls.len(), 1);
    assert_eq!(seen[1].messages[1].tool_calls[0].id, "call-1");
    assert_eq!(seen[1].messages[1].tool_calls[0].name, "write");
    assert_eq!(seen[1].messages[2].role, "tool");
    assert_eq!(seen[1].messages[2].tool_call_id.as_deref(), Some("call-1"));
    assert!(!seen[1].messages[2].is_error);
    assert!(seen[1].messages[2].tool_calls.is_empty());
    assert_eq!(seen[1].messages[2].text, "wrote note.txt");
    assert_eq!(
        session.task(outcome.task_id).await.unwrap().unwrap().state.status(),
        TaskStatus::Terminal
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn continue_after_tool_error_reaches_the_model() {
    let (scratch, ws) = workspace();
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("")
        .with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({ "path": "missing.txt" }),
        })
        .then(FauxLlm::text("saw the error"));
    let outcome = Generation::turn(&session, &llm, &offered("read"))
        .await
        .unwrap();
    let generation = Generation::bind(outcome.task_id, ROOT_CONVERSATION_ID);
    let failed = run_waiting_tool(&session, outcome.task_id, &ws)
        .await
        .unwrap();
    assert!(failed.is_error, "{}", failed.content);
    assert!(!scratch.path().join("missing.txt").exists());
    let entry = session.entry(failed.entry_id).await.unwrap().unwrap();
    assert_eq!(entry.model.unwrap()[0]["isError"], json!(true));

    let done = generation
        .continue_after_tool(&session, &llm)
        .await
        .unwrap();
    let TurnKind::Answered { entry_id } = done.kind else {
        panic!("expected an answer, got {:?}", done.kind);
    };
    assert!(done.called_llm);
    assert_eq!(llm.calls(), 2);
    let stored = session.entry(entry_id).await.unwrap().unwrap();
    assert_eq!(
        stored.model.unwrap()[0]["content"][0]["text"],
        json!("saw the error")
    );
    let seen = llm.requests();
    assert_eq!(seen.len(), 2);
    let tool = &seen[1].messages[2];
    assert_eq!(tool.role, "tool");
    assert_eq!(tool.tool_call_id.as_deref(), Some("call-1"));
    assert!(tool.is_error);
    assert!(tool.tool_calls.is_empty());
    assert_eq!(tool.text, failed.content);
    session.close().await.unwrap();
}

#[tokio::test]
async fn tool_loop_stops_after_four_model_calls() {
    let (scratch, ws) = workspace();
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("").with_tool_call(ToolCall {
        id: "call-1".into(),
        name: "write".into(),
        arguments: json!({ "path": "loop.txt", "content": "x" }),
    });
    let mut outcome = Generation::turn(&session, &llm, &offered("write"))
        .await
        .unwrap();
    let generation = Generation::bind(outcome.task_id, ROOT_CONVERSATION_ID);
    for call in 1..=MODEL_CALL_CAP {
        let TurnKind::WaitingTool { tool_task_id, .. } = outcome.kind else {
            panic!("model call {call} should wait on a tool");
        };
        assert_eq!(
            session.task(tool_task_id).await.unwrap().unwrap().state.status(),
            TaskStatus::Waiting
        );
        let ran = run_waiting_tool(&session, generation.task_id(), &ws)
            .await
            .unwrap();
        assert!(!ran.is_error, "{}", ran.content);
        if call == MODEL_CALL_CAP {
            break;
        }
        outcome = generation
            .continue_after_tool(&session, &llm)
            .await
            .unwrap();
        assert!(outcome.called_llm);
    }
    assert_eq!(llm.calls(), MODEL_CALL_CAP as u64);
    assert_eq!(
        fs::read_to_string(scratch.path().join("loop.txt")).unwrap(),
        "x"
    );
    let capped = generation
        .continue_after_tool(&session, &llm)
        .await
        .unwrap();
    assert_eq!(capped.kind, TurnKind::RestartLimit);
    assert!(!capped.called_llm);
    assert_eq!(capped.failed_attempts, 0);
    assert_eq!(llm.calls(), MODEL_CALL_CAP as u64);
    let again = generation
        .continue_after_tool(&session, &llm)
        .await
        .unwrap();
    assert_eq!(again.kind, TurnKind::RestartLimit);
    assert!(!again.called_llm);
    assert_eq!(llm.calls(), MODEL_CALL_CAP as u64);
    assert_eq!(
        session.task(generation.task_id()).await.unwrap().unwrap().state.status(),
        TaskStatus::Terminal
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn run_until_answer_writes_then_answers() {
    let (scratch, ws) = workspace();
    let session = Session::memory().await.unwrap();
    let llm = FauxLlm::text("")
        .with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "write".into(),
            arguments: json!({ "path": "note.txt", "content": "hello" }),
        })
        .then(FauxLlm::text("wrote it"));
    let done = run_until_answer(&session, &llm, &ws, &offered("write"))
        .await
        .unwrap();
    let TurnKind::Answered { entry_id } = done.kind else {
        panic!("expected an answer, got {:?}", done.kind);
    };
    assert_eq!(
        fs::read_to_string(scratch.path().join("note.txt")).unwrap(),
        "hello"
    );
    assert_eq!(llm.calls(), 2);
    assert_eq!(
        session.task(done.task_id).await.unwrap().unwrap().state.status(),
        TaskStatus::Terminal
    );
    let stored = session.entry(entry_id).await.unwrap().unwrap();
    assert_eq!(stored.kind, "pi.assistant");
    assert_eq!(
        stored.model.unwrap()[0]["content"][0]["text"],
        json!("wrote it")
    );
    session.close().await.unwrap();
}
