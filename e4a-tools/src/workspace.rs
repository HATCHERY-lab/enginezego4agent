//! Filesystem tools jailed to one canonical root.
//! `edit` replaces one exact non-overlapping copy of `old`. It is not a diff.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use e4a_types::Json;

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

/// Root directory for `read`, `write`, and `edit`.
///
/// Every path is canonicalized and must stay inside [`Self::root`]. `read` and
/// `edit` refuse files larger than 256 KiB and files that are not UTF-8.
/// `edit` errors when `old` occurs zero times or more than once.
#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
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
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// UTF-8 text of one file inside the root.
    pub fn read(&self, path: &str) -> Result<String, Error> {
        let resolved = self.resolve(path)?;
        read_text(&resolved)
    }

    /// Create parent directories inside the root and overwrite the file.
    pub fn write(&self, path: &str, content: &str) -> Result<(), Error> {
        let resolved = self.resolve(path)?;
        write_file(&self.root, &resolved, content)
    }

    /// Replace the single exact occurrence of `old` with `new`.
    ///
    /// Matches do not overlap. An empty `old` is an error. The file is left
    /// unchanged when the occurrence count is not exactly one.
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
        write_file(&self.root, &resolved, &updated)
    }

    /// Dispatch `read`, `write`, or `edit`. An unknown name is an error result.
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
    std::fs::write(path, content.as_bytes())
        .map_err(|err| Error::message(format!("cannot write file: {err}")))
}
