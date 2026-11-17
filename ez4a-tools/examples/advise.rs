//! One worker turn, then the advisor branch reviews the delta. Live models.
//!
//! Requires `EZ4A_SOCKS`, `EZ4A_OPENROUTER_KEY`. Optional `EZ4A_MODEL` (worker,
//! default the dev role's free model) and `EZ4A_ADVISOR_MODEL` (default the
//! advisor role's free model).
//!
//!   cargo run -p ez4a-tools --example advise -- E:\nemo-build\ez4a-advise

use std::env;
use std::fs;
use std::path::PathBuf;

use ez4a_harness::{advisor_agent, dev_agent, TurnInput};
use ez4a_llm::{OpenRouterClient, ToolSchema};
use ez4a_session::{Session, ROOT_CONVERSATION_ID};
use ez4a_tools::{resume_turns, review, run_until_answer, Workspace};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("ez4a-advise"));
    if env::var("EZ4A_KEEP").ok().as_deref() != Some("1") {
        let _ = fs::remove_dir_all(&root);
    }
    fs::create_dir_all(&root).expect("sandbox");
    let workspace = Workspace::new(&root).expect("workspace");
    let session = Session::sqlite(&root.join("session.sqlite"))
        .await
        .expect("sqlite");

    let worker_model = env::var("EZ4A_MODEL")
        .ok()
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| dev_agent().model.to_string());
    let advisor_model = env::var("EZ4A_ADVISOR_MODEL")
        .ok()
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| advisor_agent().model.to_string());
    let worker = OpenRouterClient::new(worker_model.clone());
    let advisor = OpenRouterClient::new(advisor_model.clone());

    resume_turns(&session, &workspace, &worker)
        .await
        .expect("resume interrupted turns");
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: env::var("EZ4A_PROMPT").unwrap_or_else(|_| {
            "Create loop.txt in the workspace. Its exact content is the single line \
while (true) {} — nothing else. Make exactly one tool call per message."
                .into()
        }),
        tools: worker_tools(),
    };
    let outcome = run_until_answer(&session, &worker, &workspace, &input)
        .await
        .expect("worker turn");
    println!("worker stop {:?}", outcome.kind);

    let notes = review(&session, &advisor, &workspace, ROOT_CONVERSATION_ID)
        .await
        .expect("advisor review");
    if notes.is_empty() {
        println!("advisor: no notes");
    }
    for note in &notes {
        println!("advisor [{}] {}", note.severity, note.note);
    }

    let view = session.watch(ROOT_CONVERSATION_ID).await.expect("watch");
    let steer = view
        .borrow()
        .entries
        .iter()
        .filter(|entry| entry.kind == "pi.advisory")
        .count();
    println!("pi.advisory entries {steer}");
    drop(view);

    if let Some(usage) = session
        .snapshot_doc(&ez4a_harness::usage_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap_or(None)
    {
        println!("pi.usage {}", serde_json::to_string(&usage["models"]).unwrap());
    }
    let _ = session.close().await;
}

fn worker_tools() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "read".into(),
            description: "Read a UTF-8 file under the workspace root.".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        },
        ToolSchema {
            name: "write".into(),
            description: "Write a UTF-8 file under the workspace root.".into(),
            parameters: json!(
                {"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}
            ),
        },
        ToolSchema {
            name: "edit".into(),
            description: "Replace one exact occurrence in a file.".into(),
            parameters: json!(
                {"type":"object","properties":{"path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"}},"required":["path","old","new"]}
            ),
        },
    ]
}
