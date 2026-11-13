//! One Jev gate call. Not the developer loop.
//!
//!   cargo run -p e4a-tools --example jev_probe -- E:\nemo-build\e4a-jev

use std::env;
use std::fs;
use std::path::PathBuf;

use e4a_llm::OpenRouterClient;
use e4a_tools::{run_jev_gate, Workspace};
use serde_json::json;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("e4a-jev"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("note.txt"), "hello").expect("seed");
    let workspace = Workspace::new(&root).expect("workspace");
    let model = env::var("E4A_JEV_MODEL").unwrap_or_else(|_| "cohere/north-mini-code:free".into());
    let llm = OpenRouterClient::new(model);
    let local = run_jev_gate(
        &llm,
        &workspace,
        "powershell",
        &json!({"command": "Remove-Item note.txt"}),
        0,
    )
    .await
    .expect("local stop");
    println!("local {local:?}");
    let gated = run_jev_gate(
        &llm,
        &workspace,
        "write",
        &json!({"path": "note.txt", "content": "replaced"}),
        0,
    )
    .await
    .expect("gate");
    println!("gate {gated:?}");
    let note = fs::read_to_string(root.join("note.txt")).unwrap_or_default();
    println!("note {note:?}");
}
