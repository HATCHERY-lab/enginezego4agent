//! A real directory watch. The thread must report a file created under the root.

use std::fs;
use std::time::{Duration, Instant};

use e4a_env::WatchHub;

#[test]
fn spawn_watch_sees_a_created_file() {
    let root = std::env::temp_dir().join(format!(
        "e4a-watch-{}-{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let hub = WatchHub::spawn_watch(&root).unwrap();
    let rx = hub.receiver().expect("spawn owns the receiver");
    let path = root.join("note.txt");
    fs::write(&path, "hi").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw = false;
    while Instant::now() < deadline {
        if let Ok(event) = rx.recv_timeout(Duration::from_millis(200)) {
            if event.path.ends_with("note.txt") {
                saw = true;
                break;
            }
        }
    }
    drop(hub);
    let _ = fs::remove_dir_all(&root);
    assert!(saw, "watch thread did not report note.txt");
}
