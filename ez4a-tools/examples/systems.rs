//! Local engine systems. No model and no network.
//!
//!   cargo run -p ez4a-tools --example systems -- E:\nemo-build\ez4a-systems

use std::env;
use std::fs;
use std::path::PathBuf;

use ez4a_tools::Workspace;

fn main() {
    let root = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| env::temp_dir().join("ez4a-systems"));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("root");
    let ws = Workspace::new(&root).expect("workspace");
    check("jail ..", ws.write("../outside.txt", "no").is_err());
    check("write", ws.write("a.txt", "one").is_ok());
    check("read", ws.read("a.txt").ok().as_deref() == Some("one"));
    check("edit one", ws.edit("a.txt", "one", "two").is_ok());
    check("write many", ws.write("b.txt", "tt").is_ok());
    check("edit many", ws.edit("b.txt", "t", "T").is_err());
    check("edit_all", ws.edit_all("b.txt", "t", "T").is_ok());
    check(
        "file",
        fs::read_to_string(root.join("a.txt")).ok().as_deref() == Some("two")
            && fs::read_to_string(root.join("b.txt")).ok().as_deref() == Some("TT"),
    );
    let shell = ws.call(
        "powershell",
        &serde_json::json!({"command": "Write-Output hi"}),
    );
    check(
        "powershell",
        !shell.is_error && shell.content.contains("hi"),
    );
    let empty = ws.call(
        "powershell",
        &serde_json::json!({"command": ""}),
    );
    check("powershell empty", empty.is_error);
    let n = ws.queue().drain("a.txt").len() + ws.queue().drain("b.txt").len();
    check("queue", n == 4);
    println!("queue_len {n}");
}

fn check(name: &str, ok: bool) {
    println!("{} {}", if ok { "ok" } else { "FAIL" }, name);
}
