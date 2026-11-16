//! Faux agent in a directory jail. No network and no key.
//!
//!   cargo run -p e4a-tools --example sandbox -- E:\nemo-build\e4a-sandbox

use std::env;
use std::fs;
use std::path::PathBuf;

use e4a_harness::{Generation, TurnInput, TurnKind};
use e4a_llm::{FauxLlm, ToolCall, ToolSchema};
use e4a_session::{Session, ROOT_CONVERSATION_ID};
use e4a_tools::{run_waiting_tool, Workspace};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        env::temp_dir().join("e4a-sandbox")
    });
    fs::create_dir_all(&root).expect("sandbox dir");
    let workspace = Workspace::new(&root).expect("workspace");
    let session = Session::memory().await.expect("session");
    let llm = FauxLlm::text("")
        .with_tool_call(ToolCall {
            id: "call-1".into(),
            name: "write".into(),
            arguments: json!({
                "path": "hello.txt",
                "content": "engine4agent sandbox\n"
            }),
        })
        .with_usage(usage(20, 12))
        .then(
            FauxLlm::text("")
                .with_tool_call(ToolCall {
                    id: "call-2".into(),
                    name: "read".into(),
                    arguments: json!({ "path": "hello.txt" }),
                })
                .with_usage(usage(30, 8)),
        )
        .then(FauxLlm::text("read hello.txt: engine4agent sandbox").with_usage(usage(40, 9)));
    let tools = vec![
        schema("read"),
        schema("write"),
        schema("edit"),
    ];
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "Create hello.txt in this folder, read it back, then tell me the text.".into(),
        tools,
    };

    println!("sandbox {}", root.display());
    let mut outcome = Generation::turn(&session, &llm, &input)
        .await
        .expect("turn");
    let mut round = 1_u32;
    loop {
        match &outcome.kind {
            TurnKind::WaitingTool { tool_task_id, .. } => {
                let task = session.task(*tool_task_id).await.expect("task").expect("tool");
                println!(
                    "round {round} model asked tool {} {}",
                    task.input["name"], task.input["callId"]
                );
                let ran = run_waiting_tool(&session, outcome.task_id, &workspace)
                    .await
                    .expect("tool");
                println!(
                    "round {round} tool {} error={} result={}",
                    ran.name, ran.is_error, ran.content
                );
                outcome = Generation::bind(outcome.task_id, ROOT_CONVERSATION_ID)
                    .continue_after_tool(&session, &llm)
                    .await
                    .expect("continue");
                round += 1;
            }
            TurnKind::Answered { entry_id } => {
                let entry = session.entry(*entry_id).await.expect("entry").expect("row");
                let text = entry.model.as_ref().unwrap()[0]["content"][0]["text"]
                    .as_str()
                    .unwrap_or("");
                println!("answer {text}");
                println!("model_calls {}", llm.calls());
                println!("file {}", fs::read_to_string(root.join("hello.txt")).expect("file"));
                break;
            }
            TurnKind::RestartLimit => {
                println!("stopped at model cap");
                break;
            }
        }
    }
    session.close().await.expect("close");
}

fn schema(name: &str) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: "filesystem tool jailed to the sandbox".into(),
        parameters: json!({ "type": "object" }),
    }
}

fn usage(input: i64, output: i64) -> e4a_llm::Usage {
    e4a_llm::Usage {
        input_tokens: input,
        output_tokens: output,
        provider: "faux".into(),
        model: "scripted".into(),
        cost_usd: 0.0,
    }
}
