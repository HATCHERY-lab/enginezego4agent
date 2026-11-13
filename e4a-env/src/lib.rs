// Portions derived from pi-durable 1.5.2 (packages/durable/src/env), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Local execution environment confined to one canonical root.
//! [`LocalEnv::exec`] runs `powershell` on Windows and `bash` elsewhere,
//! with the working directory inside that root.

pub mod node_io;
pub mod watch;
mod watch_thread;

pub use watch_thread::WatchHub;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// How many missing path components a not-yet-created file may add.
const MAX_MISSING_COMPONENTS: usize = 128;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const WAIT_SLICE: Duration = Duration::from_millis(15);
/// After the child exits or is killed, readers need a moment to observe EOF.
const DRAIN_GRACE: Duration = Duration::from_millis(200);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

    /// Run `command` with `cwd` as the working directory.
    ///
    /// [`LocalEnv`] uses `powershell` on Windows and `bash` elsewhere.
    /// A zero `timeout` means 30 seconds. A longer one is capped at 120 seconds.
    /// Names containing `KEY`, `TOKEN`, or `SECRET` are removed from the
    /// environment, as are `E4A_OPENROUTER_KEY` and `OPENROUTER_API_KEY`.
    /// `stdout` and `stderr` together are capped at 64 KiB.
    ///
    /// An empty command, a `cwd` outside the root, or a failure to start is an
    /// error and does not return [`ExecOutput`]. A timeout kills the child and
    /// is an error. A non-zero exit is [`Ok`] with that exit code.
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

/// Finished command.
///
/// `stdout` and `stderr` together are at most 64 KiB. A non-zero `exit_code`
/// is a finished command, not an error.
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

    /// Validate `command` and `cwd` before a process is created.
    ///
    /// An empty command returns here, before the path jail, so the check does
    /// not spawn and does not touch the filesystem.
    fn prepare_exec<'a>(
        &self,
        command: &'a str,
        cwd: &Path,
        timeout: Duration,
    ) -> Result<PreparedExec<'a>, Error> {
        if command.is_empty() {
            return Err(Error::message("command is empty"));
        }
        let cwd = self.resolve_existing(cwd)?;
        let meta = std::fs::metadata(&cwd)
            .map_err(|err| Error::message(format!("cannot resolve path: {err}")))?;
        if !meta.is_dir() {
            return Err(Error::message("path is not a directory"));
        }
        Ok(PreparedExec {
            command,
            cwd,
            timeout: clamp_timeout(timeout),
        })
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

    fn exec(&self, command: &str, cwd: &Path, timeout: Duration) -> Result<ExecOutput, Error> {
        let prepared = self.prepare_exec(command, cwd, timeout)?;
        run_command(&prepared)
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

struct PreparedExec<'a> {
    command: &'a str,
    cwd: PathBuf,
    timeout: Duration,
}

struct OutputBuf {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[derive(Clone, Copy)]
enum Pipe {
    Stdout,
    Stderr,
}

enum Wait {
    Done(ExitStatus),
    TimedOut,
    Failed(std::io::Error),
}

fn clamp_timeout(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        DEFAULT_TIMEOUT
    } else if timeout > MAX_TIMEOUT {
        MAX_TIMEOUT
    } else {
        timeout
    }
}

fn run_command(prepared: &PreparedExec<'_>) -> Result<ExecOutput, Error> {
    let mut cmd = Command::new(shell_program());
    let _ = shell_args(&mut cmd, prepared.command)
        .current_dir(&prepared.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    silence_window(&mut cmd);
    scrub_env(&mut cmd);
    let mut child = cmd.spawn().map_err(|err| {
        Error::message(format!("cannot start {}: {err}", shell_program()))
    })?;
    collect(&mut child, prepared.timeout)
}

#[cfg(windows)]
fn shell_program() -> &'static str {
    "powershell"
}

#[cfg(not(windows))]
fn shell_program() -> &'static str {
    "bash"
}

#[cfg(windows)]
fn shell_args<'a>(cmd: &'a mut Command, command: &str) -> &'a mut Command {
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        command,
    ])
}

#[cfg(not(windows))]
fn shell_args<'a>(cmd: &'a mut Command, command: &str) -> &'a mut Command {
    cmd.args(["-c", command])
}

fn collect(child: &mut Child, timeout: Duration) -> Result<ExecOutput, Error> {
    let deadline = Instant::now() + timeout;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        kill_child(child);
        return Err(Error::message("command output was not captured"));
    };
    let shared = Arc::new(Mutex::new(OutputBuf {
        stdout: Vec::new(),
        stderr: Vec::new(),
    }));
    let stdout_thread = match spawn_drain(
        "e4a-env-stdout",
        stdout,
        Pipe::Stdout,
        Arc::clone(&shared),
    ) {
        Ok(handle) => handle,
        Err(err) => {
            kill_child(child);
            return Err(Error::message(format!("cannot capture output: {err}")));
        }
    };
    let stderr_thread = match spawn_drain(
        "e4a-env-stderr",
        stderr,
        Pipe::Stderr,
        Arc::clone(&shared),
    ) {
        Ok(handle) => handle,
        Err(err) => {
            kill_child(child);
            detach_reader(stdout_thread);
            return Err(Error::message(format!("cannot capture output: {err}")));
        }
    };
    let waited = wait_for_exit(child, deadline);
    finish_io(stdout_thread, stderr_thread);
    match waited {
        Wait::Done(status) => {
            let (stdout, stderr) = snapshot(&shared);
            exit_output(stdout, stderr, status)
        }
        Wait::TimedOut => Err(Error::message(format!(
            "command timed out after {}ms",
            timeout.as_millis()
        ))),
        Wait::Failed(err) => Err(Error::message(format!("cannot wait for command: {err}"))),
    }
}

fn spawn_drain(
    name: &str,
    pipe: impl Read + Send + 'static,
    pipe_id: Pipe,
    shared: Arc<Mutex<OutputBuf>>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || drain(pipe, pipe_id, shared))
}

fn drain(mut pipe: impl Read, pipe_id: Pipe, shared: Arc<Mutex<OutputBuf>>) {
    let mut chunk = [0_u8; 8192];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut guard = shared.lock().unwrap_or_else(|poison| poison.into_inner());
        append_capped(&mut guard, pipe_id, &chunk[..read]);
    }
}

fn append_capped(buf: &mut OutputBuf, pipe_id: Pipe, chunk: &[u8]) {
    let total = buf.stdout.len() + buf.stderr.len();
    if total >= MAX_OUTPUT_BYTES || chunk.is_empty() {
        return;
    }
    let room = MAX_OUTPUT_BYTES - total;
    let take = chunk.len().min(room);
    let dest = match pipe_id {
        Pipe::Stdout => &mut buf.stdout,
        Pipe::Stderr => &mut buf.stderr,
    };
    dest.extend_from_slice(&chunk[..take]);
}

fn wait_for_exit(child: &mut Child, deadline: Instant) -> Wait {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Wait::Done(status),
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    kill_child(child);
                    return Wait::TimedOut;
                }
                thread::sleep(WAIT_SLICE.min(deadline.saturating_duration_since(now)));
            }
            Err(err) => {
                kill_child(child);
                return Wait::Failed(err);
            }
        }
    }
}

fn finish_io(stdout: JoinHandle<()>, stderr: JoinHandle<()>) {
    let deadline = Instant::now() + DRAIN_GRACE;
    if readers_finished(&stdout, &stderr, deadline) {
        let _ = stdout.join();
        let _ = stderr.join();
    } else {
        detach_reader(stdout);
        detach_reader(stderr);
    }
}

fn readers_finished(stdout: &JoinHandle<()>, stderr: &JoinHandle<()>, deadline: Instant) -> bool {
    loop {
        if stdout.is_finished() && stderr.is_finished() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn detach_reader(handle: JoinHandle<()>) {
    if handle.is_finished() {
        let _ = handle.join();
    } else {
        std::mem::forget(handle);
    }
}

fn snapshot(shared: &Mutex<OutputBuf>) -> (String, String) {
    let guard = shared.lock().unwrap_or_else(|poison| poison.into_inner());
    let stdout = bounded_text(&guard.stdout, MAX_OUTPUT_BYTES);
    let room = MAX_OUTPUT_BYTES - stdout.len();
    let stderr = bounded_text(&guard.stderr, room);
    (stdout, stderr)
}

fn bounded_text(bytes: &[u8], room: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= room {
        return text.into_owned();
    }
    let mut end = room;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn exit_output(stdout: String, stderr: String, status: ExitStatus) -> Result<ExecOutput, Error> {
    match status.code() {
        Some(exit_code) => Ok(ExecOutput {
            exit_code,
            stdout,
            stderr,
        }),
        None => Err(Error::message("command exited without a status")),
    }
}

fn kill_child(child: &mut Child) {
    #[cfg(windows)]
    kill_windows_tree(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
fn kill_windows_tree(pid: u32) {
    let mut cmd = Command::new("taskkill");
    let _ = cmd
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    silence_window(&mut cmd);
    scrub_env(&mut cmd);
    let _ = cmd.status();
}

fn silence_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

fn scrub_env(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if env_name_is_sensitive(&key) {
            let _ = cmd.env_remove(key);
        }
    }
}

/// `E4A_OPENROUTER_KEY`, `OPENROUTER_API_KEY`, and any name containing
/// `KEY`, `TOKEN`, or `SECRET`. Matching is ASCII case-insensitive.
/// A non-Unicode name is removed because it cannot be checked.
fn env_name_is_sensitive(name: &OsStr) -> bool {
    let Some(text) = name.to_str() else {
        return true;
    };
    if text.eq_ignore_ascii_case("E4A_OPENROUTER_KEY")
        || text.eq_ignore_ascii_case("OPENROUTER_API_KEY")
    {
        return true;
    }
    let upper = text.to_ascii_uppercase();
    upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET")
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use super::LocalEnv;

    #[test]
    fn empty_command_returns_err_without_spawning() {
        let env = LocalEnv {
            root: Path::new(".").into(),
        };
        let Err(err) = env.prepare_exec("", Path::new("."), Duration::ZERO) else {
            panic!("empty command must fail");
        };
        assert_eq!(err.to_string(), "command is empty");
    }
}
