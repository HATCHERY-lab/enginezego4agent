//! One live turn through the underlay SOCKS door.
//!
//! Requires `E4A_SOCKS` (`socks5h://127.0.0.1:<port>`) and `E4A_OPENROUTER_KEY`.
//! Optional `E4A_MODEL` (default `openrouter/auto`).
//!
//!   cargo run -p e4a-tools --example live -- E:\nemo-build\e4a-live

use std::env;
use std::fs;
use std::path::PathBuf;

use e4a_harness::{TurnInput, TurnKind};
use e4a_llm::{OpenRouterClient, ToolSchema};
use e4a_session::{Session, StoreKind, ROOT_CONVERSATION_ID};
use e4a_tools::{resume_turns, run_until_answer, Workspace};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("e4a-live"));
    if env::var("E4A_KEEP").ok().as_deref() != Some("1") {
        let _ = fs::remove_dir_all(&root);
    }
    fs::create_dir_all(&root).expect("sandbox");
    let workspace = Workspace::new(&root).expect("workspace");
    let session = open_session(&root).await;
    let model = env::var("E4A_MODEL").unwrap_or_else(|_| "cohere/north-mini-code:free".into());
    let llm = OpenRouterClient::new(model);
    resume_turns(&session, &workspace, &llm)
        .await
        .expect("resume interrupted turn");
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: env::var("E4A_PROMPT").unwrap_or_else(|_| {
            "Create note.txt in the workspace. Its exact contents must be the single line engine4agent live. Then read note.txt and reply with only that line.".into()
        }),
        tools: vec![
            schema(
                "read",
                "Read a UTF-8 file under the workspace root.",
                json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            ),
            schema(
                "write",
                "Write a UTF-8 file under the workspace root.",
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "content":{"type":"string"}
                    },
                    "required":["path","content"]
                }),
            ),
            schema(
                "edit",
                "Replace one exact occurrence in a file. Fails if the text occurs zero times or more than once.",
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "old":{"type":"string"},
                        "new":{"type":"string"}
                    },
                    "required":["path","old","new"]
                }),
            ),
            schema(
                "edit_all",
                "Replace every non-overlapping occurrence in a file.",
                json!({
                    "type":"object",
                    "properties":{
                        "path":{"type":"string"},
                        "old":{"type":"string"},
                        "new":{"type":"string"}
                    },
                    "required":["path","old","new"]
                }),
            ),
            schema(
                "jev",
                "Ask the fast gate one question. It may read one file, then answers continue, stop, compact, or steer. It does not write or run commands.",
                json!({
                    "type":"object",
                    "properties":{"question":{"type":"string"}},
                    "required":["question"]
                }),
            ),
            schema(
                "powershell",
                "Run one PowerShell command in the workspace. Empty command is an error.",
                json!({
                    "type":"object",
                    "properties":{
                        "command":{"type":"string"}
                    },
                    "required":["command"]
                }),
            ),
            schema(
                "advise",
                "Ask the read-only advisor one question. It does not write or run commands.",
                json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}),
            ),
            schema(
                "child",
                "Run one command in the workspace jail for at most 30 seconds.",
                json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
            ),
        ],
    };
    let outcome = run_until_answer(&session, &llm, &workspace, &input)
        .await
        .expect("turn");
    match outcome.kind {
        TurnKind::Answered { .. } => println!("stop answered"),
        TurnKind::WaitingTool { .. } => println!("stop waiting"),
        TurnKind::RestartLimit => println!("stop limit"),
    }
    println!("tail {}", outcome.tail.len());
    if let Ok(view) = session.watch(ROOT_CONVERSATION_ID).await {
        let kinds: Vec<_> = view
            .borrow()
            .entries
            .iter()
            .map(|entry| entry.kind.clone())
            .collect();
        println!("kinds {}", kinds.join(","));
    }
    let outside = root.join("..").join("outside.txt");
    println!(
        "outside {}",
        if outside.exists() { "EXISTS" } else { "absent" }
    );
    for entry in fs::read_dir(&root).expect("list") {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("sqlite") {
            println!("file {} bytes {}", path.display(), entry.metadata().map(|m| m.len()).unwrap_or(0));
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(text) => println!("file {} {:?}", path.file_name().and_then(|n| n.to_str()).unwrap_or("?"), text),
            Err(_) => println!("file {} binary", path.display()),
        }
    }
    let _ = session.close().await;
}

async fn open_session(root: &std::path::Path) -> Session {
    match env::var("E4A_STORE").ok().as_deref() {
        Some("jsonl") => Session::open_kind(StoreKind::Jsonl, &root.join("e4a-session-jsonl"))
            .await
            .expect("jsonl session"),
        Some("idb") => Session::open_kind(StoreKind::Idb, std::path::Path::new("e4a-live"))
            .await
            .expect("idb session"),
        _ => Session::sqlite(&root.join("session.sqlite"))
            .await
            .expect("sqlite session"),
    }
}

fn schema(name: &str, description: &str, parameters: serde_json::Value) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: description.into(),
        parameters,
    }
}
