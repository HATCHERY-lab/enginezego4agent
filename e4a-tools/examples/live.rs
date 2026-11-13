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
use e4a_session::{Session, ROOT_CONVERSATION_ID};
use e4a_tools::{run_until_answer, Workspace};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("e4a-live"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("sandbox");
    let workspace = Workspace::new(&root).expect("workspace");
    let db = root.join("session.sqlite");
    let session = Session::sqlite(&db).await.expect("sqlite session");
    let model = env::var("E4A_MODEL").unwrap_or_else(|_| "openrouter/auto".into());
    let llm = OpenRouterClient::new(model);
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "Create note.txt in the workspace. Its exact contents must be the single line engine4agent live. Then read note.txt and reply with only that line.".into(),
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
                "Replace one exact occurrence in a file.",
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
    let note = root.join("note.txt");
    if note.exists() {
        println!("note {}", fs::read_to_string(&note).expect("note"));
    } else {
        println!("note missing");
    }
    let _ = session.close().await;
}

fn schema(name: &str, description: &str, parameters: serde_json::Value) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: description.into(),
        parameters,
    }
}
