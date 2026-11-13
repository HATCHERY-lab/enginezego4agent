// Portions derived from pi-durable 1.5.2 (packages/durable/src/env), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Local execution environment confined to one canonical root.
//! [`LocalEnv::exec`] does not spawn a process.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// How many missing path components a not-yet-created file may add.
const MAX_MISSING_COMPONENTS: usize = 128;

/// Filesystem and command operations.
///
/// [`LocalEnv`] canonicalizes every path and refuses one outside its root.
pub trait ExecutionEnv {
    /// UTF-8 contents of one file. Fails when the file is larger than `max_bytes`.
    fn read_file(&self, path: &Path, max_bytes: u64) -> Result<String, Error>;

    /// Create or replace a UTF-8 file. Missing parent directories are created.
    fn write_file(&self, path: &Path, content: &str) -> Result<(), Error>;

    /// Names in one directory.
    ///
    /// A name that disappears between enumeration and metadata (`ENOENT`) is
    /// skipped. Any other metadata error fails the listing. Symlinks are not
    /// followed. An unsupported file type is skipped.
    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, Error>;

    /// Run `command` in `cwd`, bounded by `timeout`.
    ///
    /// [`LocalEnv`] does not spawn. It returns an error instead.
    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<ExecOutput, Error>;
}

/// One listed name. `path` is that name joined to the directory, not the
/// canonical target of a symlink.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    pub path: PathBuf,
    pub kind: FileKind,
    pub size: u64,
    /// Modification time, milliseconds since the Unix epoch.
    /// `0` when the platform does not report one.
    pub mtime_ms: u64,
}

/// Kind of a listed name. A symlink is not followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

/// Finished command. This slice never builds one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Root directory for filesystem operations.
///
/// The root is canonicalized in [`LocalEnv::new`]. A path whose canonical
/// location is outside that root is refused, including a symlink that points
/// out.
#[derive(Clone, Debug)]
pub struct LocalEnv {
    root: PathBuf,
}

impl LocalEnv {
    /// Canonicalize `root`. It must already exist and be a directory.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, Error> {
        let requested = root.into();
        let meta = std::fs::metadata(&requested).map_err(|err| {
            Error::message(format!("root {}: {err}", requested.display()))
        })?;
        if !meta.is_dir() {
            return Err(Error::message(format!(
                "root {} is not a directory",
                requested.display()
            )));
        }
        let root = requested.canonicalize().map_err(|err| {
            Error::message(format!("root {}: {err}", requested.display()))
        })?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn locate(&self, path: &Path) -> Result<PathBuf, Error> {
        if path.as_os_str().is_empty() {
            return Err(Error::message("path is empty"));
        }
        if contains_nul(path) {
            return Err(Error::message("path is invalid"));
        }
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            Ok(self.root.join(path))
        }
    }

    fn resolve_existing(&self, path: &Path) -> Result<PathBuf, Error> {
        let joined = self.locate(path)?;
        let canonical = joined
            .canonicalize()
            .map_err(|err| Error::message(format!("cannot resolve path: {err}")))?;
        self.ensure_inside(&canonical)?;
        Ok(canonical)
    }

    /// Canonicalize the longest existing prefix. The missing tail must be
    /// ordinary single components, so `..` cannot leave the root.
    fn resolve_create(&self, path: &Path) -> Result<PathBuf, Error> {
        let joined = self.locate(path)?;
        let (anchor, suffix) = existing_anchor(&joined)?;
        self.ensure_inside(&anchor)?;
        let mut full = anchor;
        for name in suffix.into_iter().rev() {
            if name == "." {
                continue;
            }
            if !is_single_normal(&name) {
                return Err(Error::message("path escapes the root"));
            }
            full.push(&name);
            if !full.starts_with(&self.root) {
                return Err(Error::message("path escapes the root"));
            }
        }
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let canon = full.canonicalize().map_err(|err| {
                    Error::message(format!("cannot resolve path: {err}"))
                })?;
                self.ensure_inside(&canon)?;
                if std::fs::metadata(&canon).is_ok_and(|item| item.is_dir()) {
                    return Err(Error::message("path is a directory"));
                }
                Ok(canon)
            }
            Ok(meta) if meta.is_dir() => Err(Error::message("path is a directory")),
            Ok(_) => Ok(full),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(full),
            Err(err) => Err(Error::message(format!("cannot resolve path: {err}"))),
        }
    }

    fn ensure_inside(&self, path: &Path) -> Result<(), Error> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            Err(Error::message("path escapes the root"))
        }
    }
}

impl ExecutionEnv for LocalEnv {
    fn read_file(&self, path: &Path, max_bytes: u64) -> Result<String, Error> {
        let resolved = self.resolve_existing(path)?;
        let file = File::open(&resolved)
            .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
        let meta = file
            .metadata()
            .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
        if !meta.is_file() {
            return Err(Error::message("path is not a file"));
        }
        if meta.len() > max_bytes {
            return Err(too_large(max_bytes));
        }
        let mut buf = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|err| Error::message(format!("cannot read file: {err}")))?;
        if buf.len() as u64 > max_bytes {
            return Err(too_large(max_bytes));
        }
        String::from_utf8(buf).map_err(|_| Error::message("file is not utf-8"))
    }

    fn write_file(&self, path: &Path, content: &str) -> Result<(), Error> {
        let resolved = self.resolve_create(path)?;
        let Some(parent) = resolved.parent() else {
            return Err(Error::message("path escapes the root"));
        };
        if parent != self.root && !parent.starts_with(&self.root) {
            return Err(Error::message("path escapes the root"));
        }
        std::fs::create_dir_all(parent).map_err(|err| {
            Error::message(format!("cannot create parent directory: {err}"))
        })?;
        let parent_canon = parent
            .canonicalize()
            .map_err(|err| Error::message(format!("cannot resolve path: {err}")))?;
        self.ensure_inside(&parent_canon)?;
        let Some(name) = resolved.file_name() else {
            return Err(Error::message("path escapes the root"));
        };
        if !is_single_normal(name) {
            return Err(Error::message("path escapes the root"));
        }
        let final_path = parent_canon.join(name);
        self.ensure_inside(&final_path)?;
        match std::fs::symlink_metadata(&final_path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let canon = final_path.canonicalize().map_err(|err| {
                    Error::message(format!("cannot resolve path: {err}"))
                })?;
                self.ensure_inside(&canon)?;
                write_bytes(&canon, content)
            }
            Ok(meta) if meta.is_dir() => Err(Error::message("path is a directory")),
            Ok(_) => write_bytes(&final_path, content),
            Err(err) if err.kind() == ErrorKind::NotFound => write_bytes(&final_path, content),
            Err(err) => Err(Error::message(format!("cannot write file: {err}"))),
        }
    }

    fn list_dir(&self, path: &Path) -> Result<Vec<DirEntry>, Error> {
        let resolved = self.resolve_existing(path)?;
        let meta = std::fs::metadata(&resolved)
            .map_err(|err| Error::message(format!("cannot list directory: {err}")))?;
        if !meta.is_dir() {
            return Err(Error::message("path is not a directory"));
        }
        let entries = std::fs::read_dir(&resolved)
            .map_err(|err| Error::message(format!("cannot list directory: {err}")))?;
        let mut listed = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|err| Error::message(format!("cannot list directory: {err}")))?;
            let name = entry.file_name();
            let path = entry.path();
            // Removed between enumeration and metadata: not part of the listing.
            match std::fs::symlink_metadata(&path) {
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                Err(err) => {
                    return Err(Error::message(format!(
                        "cannot list {}: {err}",
                        path.display()
                    )));
                }
                Ok(meta) => {
                    let Some(kind) = file_kind(&meta) else {
                        continue;
                    };
                    listed.push(DirEntry {
                        name,
                        path,
                        kind,
                        size: meta.len(),
                        mtime_ms: mtime_ms(&meta),
                    });
                }
            }
        }
        Ok(listed)
    }

    fn exec(&self, _command: &str, _cwd: &Path, _timeout: Duration) -> Result<ExecOutput, Error> {
        Err(Error::message("exec is not available in this slice"))
    }
}

fn write_bytes(path: &Path, content: &str) -> Result<(), Error> {
    std::fs::write(path, content.as_bytes())
        .map_err(|err| Error::message(format!("cannot write file: {err}")))
}

fn too_large(max_bytes: u64) -> Error {
    Error::message(format!("file exceeds {max_bytes} bytes"))
}

fn contains_nul(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().contains(&0)
}

fn existing_anchor(path: &Path) -> Result<(PathBuf, Vec<OsString>), Error> {
    let mut cursor = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        if suffix.len() > MAX_MISSING_COMPONENTS {
            return Err(Error::message("path escapes the root"));
        }
        match cursor.try_exists() {
            Ok(true) => {
                let anchor = cursor
                    .canonicalize()
                    .map_err(|err| Error::message(format!("cannot resolve path: {err}")))?;
                return Ok((anchor, suffix));
            }
            Ok(false) => {}
            Err(err) => return Err(Error::message(format!("cannot resolve path: {err}"))),
        }
        let Some(name) = cursor.file_name().map(OsStr::to_os_string) else {
            return Err(Error::message("path escapes the root"));
        };
        let Some(parent) = cursor.parent().map(Path::to_path_buf) else {
            return Err(Error::message("path escapes the root"));
        };
        if parent == cursor {
            return Err(Error::message("path escapes the root"));
        }
        suffix.push(name);
        cursor = parent;
    }
}

fn is_single_normal(name: &OsStr) -> bool {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(part)), None) if part == name => true,
        _ => false,
    }
}

fn file_kind(meta: &std::fs::Metadata) -> Option<FileKind> {
    let ty = meta.file_type();
    if ty.is_symlink() {
        Some(FileKind::Symlink)
    } else if ty.is_dir() {
        Some(FileKind::Directory)
    } else if ty.is_file() {
        Some(FileKind::File)
    } else {
        None
    }
}

fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    let Ok(modified) = meta.modified() else {
        return 0;
    };
    let Ok(elapsed) = modified.duration_since(UNIX_EPOCH) else {
        return 0;
    };
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}
