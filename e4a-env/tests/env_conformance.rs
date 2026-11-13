// Portions derived from pi-durable 1.5.2 (packages/durable/src/testing/env-conformance.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! `LocalEnv` cases taken from the pi-durable env conformance suite.
//!
//! Watch, abort, binary ranges, argv exec, and optional `listFiles` are not
//! this environment. The suite does not refuse a path outside the root.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use e4a_env::{DirEntry, Error, ExecutionEnv, FileKind, LocalEnv};

const READ_LIMIT: u64 = 1024;

fn scratch_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!(
        "e4a-envconf-{}-{}-{}",
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

    fn env(&self) -> LocalEnv {
        must(LocalEnv::new(&self.0))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn must<T>(result: Result<T, Error>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("{err}"),
    }
}

fn sorted_names(entries: &[DirEntry]) -> Vec<String> {
    let mut names = entries
        .iter()
        .map(|entry| entry.name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn disk_names(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn expect_names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn find<'a>(entries: &'a [DirEntry], name: &str) -> &'a DirEntry {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
}

/// Write `data.txt`, read it back, replace it, and write a nested path.
///
/// The suite reads `hello world` in ranges and later reads `sub/made.txt`.
/// `writeFile` creates missing parents (`a/b/c/AGENTS.md`).
#[test]
fn write_and_read_file() {
    let scratch = Scratch::new();
    let env = scratch.env();

    must(env.write_file(Path::new("data.txt"), "hello world"));
    let text = must(env.read_file(Path::new("data.txt"), READ_LIMIT));
    assert_eq!(text, "hello world");
    assert_eq!(text.len(), 11);
    assert_eq!(
        fs::read_to_string(env.root().join("data.txt")).unwrap(),
        "hello world"
    );

    must(env.write_file(Path::new("data.txt"), "two!"));
    assert_eq!(must(env.read_file(Path::new("data.txt"), READ_LIMIT)), "two!");

    must(env.write_file(Path::new("a/b/c/AGENTS.md"), "x"));
    assert_eq!(
        must(env.read_file(Path::new("a/b/c/AGENTS.md"), READ_LIMIT)),
        "x"
    );
}

/// `openBinaryReader("missing.txt")` is `not_found`. A directory is `is_directory`.
#[test]
fn read_missing_file() {
    let scratch = Scratch::new();
    let env = scratch.env();

    let missing = env
        .read_file(Path::new("missing.txt"), READ_LIMIT)
        .unwrap_err();
    assert!(
        missing.to_string().contains("cannot resolve path"),
        "{missing}"
    );
    assert!(!env.root().join("missing.txt").exists());

    must(env.mkdir(Path::new("dir")));
    let directory = env.read_file(Path::new("dir"), READ_LIMIT).unwrap_err();
    assert_eq!(directory.to_string(), "path is not a file");
}

/// Directory reader pages `a.txt`..`e.txt` and `sub` exactly once.
#[test]
fn list_directory_entries_once() {
    let scratch = Scratch::new();
    let env = scratch.env();
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
        must(env.write_file(Path::new(name), name));
    }
    must(env.mkdir(Path::new("sub")));

    let entries = must(env.list_dir(Path::new(".")));
    let names = sorted_names(&entries);
    assert_eq!(
        names,
        expect_names(&["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "sub"])
    );
    assert_eq!(names, disk_names(env.root()));
    assert_eq!(entries.len(), names.len());

    let file = find(&entries, "a.txt");
    assert_eq!(file.kind, FileKind::File);
    assert_eq!(file.size, 5);
    assert_eq!(file.path, env.root().join("a.txt"));

    let sub = find(&entries, "sub");
    assert_eq!(sub.kind, FileKind::Directory);
    assert_eq!(sub.path, env.root().join("sub"));

    for entry in &entries {
        assert!(
            entry.path.starts_with(env.root()),
            "{}",
            entry.path.display()
        );
    }
}

/// Empty directory, missing path (`not_found`), and a file (`not_directory`).
#[test]
fn list_directory_missing_empty_and_file() {
    let scratch = Scratch::new();
    let env = scratch.env();

    must(env.mkdir(Path::new("empty")));
    assert!(must(env.list_dir(Path::new("empty"))).is_empty());

    let missing = env.list_dir(Path::new("missing")).unwrap_err();
    assert!(
        missing.to_string().contains("cannot resolve path"),
        "{missing}"
    );

    must(env.write_file(Path::new("file.txt"), "x"));
    let file = env.list_dir(Path::new("file.txt")).unwrap_err();
    assert_eq!(file.to_string(), "path is not a directory");
}

/// A name that still exists is listed. A name removed before `list_dir` is not.
///
/// The suite deletes between `opendir` and `lstat`. `list_dir` has no reader,
/// so the delete happens before the call. The listing then matches the directory.
#[test]
fn list_skips_nothing_that_exists() {
    let scratch = Scratch::new();
    let env = scratch.env();
    must(env.mkdir(Path::new("dir")));
    for name in ["x", "y", "z"] {
        must(env.write_file(Path::new(&format!("dir/{name}")), name));
    }
    let dir = env.root().join("dir");

    let listed = must(env.list_dir(Path::new("dir")));
    assert_eq!(sorted_names(&listed), disk_names(&dir));
    assert_eq!(sorted_names(&listed), expect_names(&["x", "y", "z"]));

    fs::remove_file(dir.join("y")).unwrap();
    let listed = must(env.list_dir(Path::new("dir")));
    assert_eq!(sorted_names(&listed), disk_names(&dir));
    assert_eq!(sorted_names(&listed), expect_names(&["x", "z"]));

    fs::remove_file(dir.join("x")).unwrap();
    fs::remove_file(dir.join("z")).unwrap();
    let listed = must(env.list_dir(Path::new("dir")));
    assert!(listed.is_empty());
    assert!(disk_names(&dir).is_empty());
}
