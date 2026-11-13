//! Filesystem tools jailed to one canonical root.
//! `edit` replaces one exact non-overlapping copy of `old`. It is not a diff.
//! `edit_all` replaces every non-overlapping copy.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use e4a_types::Json;

use crate::edit_diff::apply_hunks;
use crate::mutation_queue::{Mutation, MutationKind, Queue};
use crate::Error;

/// Largest file `read` or `edit` will load.
const MAX_READ_BYTES: u64 = 256 * 1024;

/// How many missing path components a not-yet-created file may add.
const MAX_MISSING_COMPONENTS: usize = 128;

/// Result of one named tool. Failures set [`Self::is_error`] and do not panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolEffect {
    pub is_error: bool,
    pub content: String,
}

/// Root directory for `read`, `write`, `edit`, `edit_all`, `code`, and `resize`.
///
/// Every path is canonicalized and must stay inside [`Self::root`]. `read` and
/// `edit` refuse files larger than 256 KiB and files that are not UTF-8.
/// `edit` errors when `old` occurs zero times or more than once. `edit_all`
/// errors when `old` is empty or occurs zero times.
/// A successful `write` records [`MutationKind::Write`]. A successful `edit`
/// or `edit_all` records one [`MutationKind::Edit`]. Jail failures and other
/// errors record nothing.
#[derive(Debug)]
pub struct Workspace {
    root: PathBuf,
    queue: Mutex<Queue>,
}

impl Clone for Workspace {
    fn clone(&self) -> Self {
        let queue = self.lock_queue().clone();
        Self {
            root: self.root.clone(),
            queue: Mutex::new(queue),
        }
    }
}

impl Workspace {
    /// Canonicalize `root`. It must already exist and be a directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let requested = root.into();
        let meta = std::fs::metadata(&requested).map_err(|err| {
            Error::message(format!("workspace root {}: {err}", requested.display()))
        })?;
        if !meta.is_dir() {
            return Err(Error::message(format!(
                "workspace root {} is not a directory",
                requested.display()
            )));
        }
        let root = requested.canonicalize().map_err(|err| {
            Error::message(format!("workspace root {}: {err}", requested.display()))
        })?;
        Ok(Self {
            root,
            queue: Mutex::new(Queue::default()),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Mutations from successful `write`, `edit`, and `edit_all` calls.
    ///
    /// Drop the guard before those methods. They lock this same mutex.
    /// A poisoned lock still returns the queue.
    pub fn queue(&self) -> MutexGuard<'_, Queue> {
        self.lock_queue()
    }

    /// UTF-8 text of one file inside the root.
    pub fn read(&self, path: &str) -> Result<String, Error> {
        let resolved = self.resolve(path)?;
        read_text(&resolved)
    }

    /// Create parent directories inside the root and overwrite the file.
    ///
    /// On success, records [`MutationKind::Write`] for `path`.
    pub fn write(&self, path: &str, content: &str) -> Result<(), Error> {
        let resolved = self.resolve(path)?;
        write_file(&self.root, &resolved, content)?;
        self.record(path, MutationKind::Write);
        Ok(())
    }

    /// Replace the single exact occurrence of `old` with `new`.
    ///
    /// Matches do not overlap. An empty `old` is an error. The file is left
    /// unchanged when the occurrence count is not exactly one. On success,
    /// records one [`MutationKind::Edit`] for `path`.
    pub fn edit(&self, path: &str, old: &str, new: &str) -> Result<(), Error> {
        if old.is_empty() {
            return Err(Error::message("old string is empty"));
        }
        let resolved = self.resolve(path)?;
        let text = read_text(&resolved)?;
        let matches = text.matches(old).count();
        if matches == 0 {
            return Err(Error::message("old string was not found"));
        }
        if matches > 1 {
            return Err(Error::message(format!(
                "old string matched {matches} times"
            )));
        }
        let updated = text.replacen(old, new, 1);
        write_file(&self.root, &resolved, &updated)?;
        self.record(path, MutationKind::Edit);
        Ok(())
    }

    /// Replace every non-overlapping occurrence of `old` with `new`.
    ///
    /// An empty `old` is an error. Zero occurrences is an error and the file
    /// is left unchanged. On success, records one [`MutationKind::Edit`].
    pub fn edit_all(&self, path: &str, old: &str, new: &str) -> Result<(), Error> {
        if old.is_empty() {
            return Err(Error::message("old string is empty"));
        }
        let resolved = self.resolve(path)?;
        let text = read_text(&resolved)?;
        let updated = apply_hunks(&text, old, new).map_err(Error::message)?;
        write_file(&self.root, &resolved, &updated)?;
        self.record(path, MutationKind::Edit);
        Ok(())
    }

    /// Dispatch `read`, `write`, `edit`, `edit_all`, `bash`, `powershell`, `code`, or `resize`.
    /// An unknown name is an error result.
    pub fn call(&self, name: &str, arguments: &Json) -> ToolEffect {
        match self.execute(name, arguments) {
            Ok(content) => ToolEffect {
                is_error: false,
                content,
            },
            Err(error) => ToolEffect {
                is_error: true,
                content: error.to_string(),
            },
        }
    }

    fn execute(&self, name: &str, arguments: &Json) -> Result<String, Error> {
        match name {
            "read" => {
                let path = arg(arguments, "path")?;
                self.read(path)
            }
            "write" => {
                let path = arg(arguments, "path")?;
                let content = arg(arguments, "content")?;
                self.write(path, content)?;
                Ok(format!("wrote {path}"))
            }
            "edit" => {
                let path = arg(arguments, "path")?;
                let old = arg(arguments, "old")?;
                let new = arg(arguments, "new")?;
                self.edit(path, old, new)?;
                Ok(format!("edited {path}"))
            }
            "edit_all" => {
                let path = arg(arguments, "path")?;
                let old = arg(arguments, "old")?;
                let new = arg(arguments, "new")?;
                self.edit_all(path, old, new)?;
                Ok(format!("edited {path}"))
            }
            "bash" => crate::shell::bash(&self.root, arguments),
            "powershell" => crate::shell::powershell(&self.root, arguments),
            "code" => {
                let script = arg(arguments, "script")?;
                e4a_codemode::eval(script).map_err(Error::message)
            }
            "resize" => {
                let path = arg(arguments, "path")?;
                let w = arg_u32(arguments, "w")?;
                let h = arg_u32(arguments, "h")?;
                let resolved = self.resolve(path)?;
                let bytes = read_bytes(&self.root, &resolved)?;
                crate::photon::plan(
                    crate::photon::PhotonOp::Resize { w, h },
                    bytes.len() as u64,
                )
                .map_err(Error::message)?;
                finish_resize(&self.root, path, &resolved, &bytes, w, h)
            }
            other => Err(Error::message(format!("unknown tool {other}"))),
        }
    }

    fn resolve(&self, raw: &str) -> Result<PathBuf, Error> {
        if raw.is_empty() || raw.contains('\0') {
            return Err(Error::message("path is empty"));
        }
        let requested = Path::new(raw);
        let joined = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        };
        let (anchor, suffix) = existing_anchor(&joined)?;
        if !anchor.starts_with(&self.root) {
            return Err(Error::message("path escapes the workspace"));
        }
        let mut full = anchor;
        for name in suffix.into_iter().rev() {
            if name == "." {
                continue;
            }
            if !is_single_normal(&name) {
                return Err(Error::message("path escapes the workspace"));
            }
            full.push(&name);
            if !full.starts_with(&self.root) {
                return Err(Error::message("path escapes the workspace"));
            }
        }
        Ok(full)
    }

    fn lock_queue(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Record `path` as the caller wrote it. An empty path is not queued.
    /// Call only after the filesystem change has succeeded.
    fn record(&self, path: &str, kind: MutationKind) {
        if path.is_empty() {
            return;
        }
        let _ = self.lock_queue().push(Mutation {
            path: path.to_string(),
            kind,
        });
    }
}

fn arg<'a>(arguments: &'a Json, key: &str) -> Result<&'a str, Error> {
    let object = arguments
        .as_object()
        .ok_or_else(|| Error::message("arguments must be an object"))?;
    match object.get(key) {
        Some(Json::String(value)) => Ok(value.as_str()),
        Some(_) => Err(Error::message(format!("{key} must be a string"))),
        None => Err(Error::message(format!("{key} is required"))),
    }
}

fn arg_u32(arguments: &Json, key: &str) -> Result<u32, Error> {
    let object = arguments
        .as_object()
        .ok_or_else(|| Error::message("arguments must be an object"))?;
    match object.get(key) {
        Some(Json::Number(number)) => number
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| Error::message(format!("{key} must fit in u32"))),
        Some(_) => Err(Error::message(format!("{key} must be an integer"))),
        None => Err(Error::message(format!("{key} is required"))),
    }
}

/// One past the image plan cap, so an oversized file still reaches `plan`.
const MAX_IMAGE_READ: u64 = 5_000_001;

fn read_bytes(root: &Path, path: &Path) -> Result<Vec<u8>, Error> {
    if path == root || !path.starts_with(root) {
        return Err(Error::message("path escapes the workspace"));
    }
    let file = File::open(path).map_err(|err| Error::message(format!("cannot read file: {err}")))?;
    let meta = file
        .metadata()
        .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
    if !meta.is_file() {
        return Err(Error::message("path is not a file"));
    }
    let mut buf = Vec::new();
    file.take(MAX_IMAGE_READ)
        .read_to_end(&mut buf)
        .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
    Ok(buf)
}

#[cfg(feature = "photon")]
fn finish_resize(
    root: &Path,
    path: &str,
    resolved: &Path,
    bytes: &[u8],
    w: u32,
    h: u32,
) -> Result<String, Error> {
    let png = crate::photon::resize_png(bytes, w, h).map_err(Error::message)?;
    write_bytes(root, resolved, &png)?;
    Ok(format!("resized {path}"))
}

#[cfg(not(feature = "photon"))]
fn finish_resize(
    _root: &Path,
    _path: &str,
    _resolved: &Path,
    _bytes: &[u8],
    _w: u32,
    _h: u32,
) -> Result<String, Error> {
    Err(Error::message("photon wasm not linked"))
}

fn existing_anchor(path: &Path) -> Result<(PathBuf, Vec<std::ffi::OsString>), Error> {
    let mut cursor = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        if suffix.len() > MAX_MISSING_COMPONENTS {
            return Err(Error::message("path escapes the workspace"));
        }
        match cursor.try_exists() {
            Ok(true) => {
                let anchor = cursor.canonicalize().map_err(|err| {
                    Error::message(format!("cannot resolve path: {err}"))
                })?;
                return Ok((anchor, suffix));
            }
            Ok(false) => {}
            Err(err) => {
                return Err(Error::message(format!("cannot resolve path: {err}")));
            }
        }
        let Some(name) = cursor.file_name().map(|name| name.to_os_string()) else {
            return Err(Error::message("path escapes the workspace"));
        };
        let Some(parent) = cursor.parent().map(Path::to_path_buf) else {
            return Err(Error::message("path escapes the workspace"));
        };
        if parent == cursor {
            return Err(Error::message("path escapes the workspace"));
        }
        suffix.push(name);
        cursor = parent;
    }
}

fn is_single_normal(name: &std::ffi::OsStr) -> bool {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(part)), None) if part == name => true,
        _ => false,
    }
}

fn read_text(path: &Path) -> Result<String, Error> {
    let file = File::open(path).map_err(|err| Error::message(format!("cannot read file: {err}")))?;
    let meta = file
        .metadata()
        .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
    if !meta.is_file() {
        return Err(Error::message("path is not a file"));
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(Error::message(format!(
            "file exceeds {MAX_READ_BYTES} bytes"
        )));
    }
    let mut buf = Vec::new();
    file.take(MAX_READ_BYTES.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
    if buf.len() as u64 > MAX_READ_BYTES {
        return Err(Error::message(format!(
            "file exceeds {MAX_READ_BYTES} bytes"
        )));
    }
    String::from_utf8(buf).map_err(|_| Error::message("file is not utf-8"))
}

fn write_file(root: &Path, path: &Path, content: &str) -> Result<(), Error> {
    write_bytes(root, path, content.as_bytes())
}

fn write_bytes(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if path == root || !path.starts_with(root) {
        return Err(Error::message("path escapes the workspace"));
    }
    let Some(parent) = path.parent() else {
        return Err(Error::message("path escapes the workspace"));
    };
    if parent != root && !parent.starts_with(root) {
        return Err(Error::message("path escapes the workspace"));
    }
    std::fs::create_dir_all(parent)
        .map_err(|err| Error::message(format!("cannot create parent directory: {err}")))?;
    std::fs::write(path, bytes).map_err(|err| Error::message(format!("cannot write file: {err}")))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::Workspace;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "e4a-tools-workspace-{}-{}",
                std::process::id(),
                nanos
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn code_is_unlinked_and_zero_resize_fails_in_plan() {
        let scratch = Scratch::new();
        let workspace = Workspace::new(&scratch.0).unwrap();
        let code = workspace.call("code", &json!({ "script": "1" }));
        assert!(code.is_error, "{}", code.content);
        assert!(
            code.content.contains("js runtime not linked"),
            "{}",
            code.content
        );
        fs::write(scratch.0.join("shot.png"), b"png").unwrap();
        let resized = workspace.call(
            "resize",
            &json!({ "path": "shot.png", "w": 0, "h": 4 }),
        );
        assert!(resized.is_error, "{}", resized.content);
        assert!(
            resized.content.contains("resize dimension is zero"),
            "{}",
            resized.content
        );
    }
}
