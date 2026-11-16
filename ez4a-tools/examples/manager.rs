//! One manager turn: the manager spawns a worker subagent through the
//! `agent` tool and answers from its result. Live models.
//!
//! Requires `EZ4A_SOCKS`, `EZ4A_OPENROUTER_KEY`, and `EZ4A_BIN` pointing at the
//! built `ez4a` binary — the subagent is a real process running
//! `ez4a run --root <subdir> --prompt <text>` with the worker tool set.
//!
//!   cargo build --release -p ez4a-host
//!   EZ4A_BIN=<target>/release/ez4a.exe cargo run -p ez4a-tools --example manager -- E:\nemo-build\ez4a-manager

use std::env;
use std::fs;
use std::path::PathBuf;

use ez4a_harness::{manager_agent, TurnInput};
use ez4a_llm::{OpenRouterClient, ToolSchema};
use ez4a_session::{Session, ROOT_CONVERSATION_ID};
use ez4a_tools::{resume_turns, run_until_answer, Workspace};
use serde_json::json;

fn main() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(run());
}

async fn run() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("ez4a-manager"));
    if env::var("EZ4A_KEEP").ok().as_deref() != Some("1") {
        let _ = fs::remove_dir_all(&root);
    }
    fs::create_dir_all(&root).expect("sandbox");
    let workspace = Workspace::new(&root).expect("workspace");
    let session = Session::sqlite(&root.join("session.sqlite"))
        .await
        .expect("sqlite");

    let model = env::var("EZ4A_MODEL")
        .ok()
        .filter(|model| !model.is_empty())
        .unwrap_or_else(|| manager_agent().model.to_string());
    let llm = OpenRouterClient::new(model);

    resume_turns(&session, &workspace, &llm)
        .await
        .expect("resume interrupted turns");
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: env::var("EZ4A_PROMPT").unwrap_or_else(|_| {
            "Spawn one worker agent and tell it to create hello.txt containing exactly hi. \
When its answer comes back, reply done."
                .into()
        }),
        tools: manager_tools(),
    };
    let outcome = run_until_answer(&session, &llm, &workspace, &input)
        .await
        .expect("manager turn");
    println!("manager stop {:?}", outcome.kind);

    let subagents = root.join("subagents");
    for entry in fs::read_dir(&subagents).unwrap_or_else(|_| fs::read_dir(&root).unwrap()) {
        let entry = entry.expect("entry");
        println!("subagent dir {}", entry.path().display());
    }
    let hello = subagents
        .join("agent-1")
        .join("hello.txt");
    println!(
        "hello.txt {}",
        match fs::read_to_string(&hello) {
            Ok(text) => format!("= {text:?}"),
            Err(err) => format!("missing ({err})"),
        }
    );
    if let Some(usage) = session
        .snapshot_doc(&ez4a_harness::usage_doc(), Some(ROOT_CONVERSATION_ID))
        .await
        .unwrap_or(None)
    {
        println!("pi.usage {}", serde_json::to_string(&usage["models"]).unwrap());
    }
    let _ = session.close().await;
}

/// The manager set: worker tools plus `agent` and the mail wire.
fn manager_tools() -> Vec<ToolSchema> {
    let mut tools = worker_tools();
    tools.push(ToolSchema {
        name: "agent".into(),
        description: "Spawn one worker agent. It runs with the worker tool set (it cannot \
spawn agents) in its own workspace under subagents/. Give it a self-contained prompt; \
its final answer comes back as the tool result."
            .into(),
        parameters: json!({
            "type": "object",
            "properties": { "prompt": { "type": "string" } },
            "required": ["prompt"]
        }),
    });
    for (name, description) in [
        (
            "mail_send",
            "Send mail through hatchery's mail4agent. `to` is an account, a session address, or #room.",
        ),
        ("mail_inbox", "Read your mail. wait_secs parks the read for new mail (max 60)."),
        ("mail_whoami", "Your own mail address and card."),
    ] {
        tools.push(ToolSchema {
            name: name.into(),
            description: description.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string" },
                    "subject": { "type": "string" },
                    "body": { "type": "string" },
                    "wait_secs": { "type": "integer" }
                }
            }),
        });
    }
    tools
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
