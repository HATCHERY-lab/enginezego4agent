//! Two processes, one sqlite file.
//!
//! `park` writes note.txt, commits a waiting powershell, and exits before the
//! command runs. `resume` opens that file and finishes the command.
//!
//!   cargo run -p ez4a-tools --example resume_proc -- park E:\nemo-build\ez4a-resume-proc
//!   cargo run -p ez4a-tools --example resume_proc -- resume E:\nemo-build\ez4a-resume-proc

use std::env;
use std::fs;
use std::path::PathBuf;

use ez4a_harness::{Generation, TurnInput, TurnKind};
use ez4a_llm::{FauxLlm, ToolCall, ToolSchema};
use ez4a_session::{Session, ROOT_CONVERSATION_ID};
use ez4a_tools::{resume_waiting, Workspace};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = env::args().skip(1);
    let phase = args.next().expect("phase park or resume");
    let root = PathBuf::from(args.next().expect("root"));
    fs::create_dir_all(&root).expect("root");
    let workspace = Workspace::new(&root).expect("workspace");
    let session = Session::sqlite(&root.join("session.sqlite"))
        .await
        .expect("sqlite");
    match phase.as_str() {
        "park" => {
            fs::write(root.join("note.txt"), "resume-ok").expect("note");
            let llm = FauxLlm::text("").with_tool_call(ToolCall {
                id: "c1".into(),
                name: "powershell".into(),
                arguments: json!({"command": "Write-Output ez4a-killed"}),
            });
            let outcome = Generation::turn(&session, &llm, &powershell_input())
                .await
                .expect("park");
            assert!(matches!(outcome.kind, TurnKind::WaitingTool { .. }));
            println!("parked {}", outcome.task_id);
        }
        "park-ask" => {
            let name = args.next().expect("tool name");
            let question = args.next().expect("question");
            let llm = FauxLlm::text("").with_tool_call(ToolCall {
                id: "c1".into(),
                name: name.clone(),
                arguments: json!({"question": question}),
            });
            let input = TurnInput {
                conversation_id: ROOT_CONVERSATION_ID,
                user_text: "ask".into(),
                tools: vec![ToolSchema {
                    name: name,
                    description: "ask".into(),
                    parameters: json!({"type": "object"}),
                }],
            };
            let outcome = Generation::turn(&session, &llm, &input).await.expect("park");
            assert!(matches!(outcome.kind, TurnKind::WaitingTool { .. }));
            println!("parked {}", outcome.task_id);
        }
        "resume" => {
            let finished = resume_waiting(&session, &workspace)
                .await
                .expect("resume");
            assert_eq!(finished.len(), 1);
            println!("resumed {}", finished[0].content.trim());
            println!(
                "note {}",
                fs::read_to_string(root.join("note.txt")).unwrap_or_default()
            );
        }
        other => panic!("unknown phase {other}"),
    }
    session.close().await.expect("close");
}

fn powershell_input() -> TurnInput {
    TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: "run it".into(),
        tools: vec![ToolSchema {
            name: "powershell".into(),
            description: "shell".into(),
            parameters: json!({"type": "object"}),
        }],
    }
}
