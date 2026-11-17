//! `bash` and `powershell`. Both use the workspace root as the working directory.
//! Combined stdout and stderr is capped at 64 KiB. A non-zero exit, a timeout,
//! or a failure to start is an error string, not a panic.

use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ez4a_types::Json;

use crate::Error;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_OUTPUT_BYTES: usize = 64 * 1024;
const WAIT_SLICE: Duration = Duration::from_millis(15);
/// After the child is killed, readers need a moment to observe EOF.
const DRAIN_GRACE: Duration = Duration::from_millis(200);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const POWERSHELL_UTF8: &str =
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}";

#[derive(Debug)]
struct Ready<'a> {
    command: &'a str,
    timeout: Duration,
}

struct OutputBuf {
    bytes: Vec<u8>,
}

enum RunError {
    Start(std::io::Error),
    Tool(String),
}

enum Wait {
    Done(ExitStatus),
    TimedOut,
    Error(std::io::Error),
}

/// Run `bash -c` in `root`. Validation failures do not spawn.
pub(crate) fn bash(root: &Path, arguments: &Json) -> Result<String, Error> {
    let ready = prepare(arguments)?;
    map_program(
        run_program(root, "bash", &["-c", ready.command], ready.timeout),
        "bash",
    )
}

/// Run PowerShell in `root`. `pwsh` is tried first, then `powershell`.
/// A program that cannot start is skipped. Validation failures do not spawn.
pub(crate) fn powershell(root: &Path, arguments: &Json) -> Result<String, Error> {
    let ready = prepare(arguments)?;
    let script = format!("{POWERSHELL_UTF8}\n{}", ready.command);
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script.as_str(),
    ];
    let result = match run_program(root, "pwsh", &args, ready.timeout) {
        Err(RunError::Start(_)) => run_program(root, "powershell", &args, ready.timeout),
        other => other,
    };
    map_program(result, "powershell")
}

fn prepare(arguments: &Json) -> Result<Ready<'_>, Error> {
    let command = required_string(arguments, "command")?;
    if command.is_empty() {
        return Err(Error::message("command is empty"));
    }
    let timeout = parse_timeout(arguments)?;
    Ok(Ready { command, timeout })
}

fn required_string<'a>(arguments: &'a Json, key: &str) -> Result<&'a str, Error> {
    let object = arguments
        .as_object()
        .ok_or_else(|| Error::message("arguments must be an object"))?;
    match object.get(key) {
        Some(Json::String(value)) => Ok(value.as_str()),
        Some(_) => Err(Error::message(format!("{key} must be a string"))),
        None => Err(Error::message(format!("{key} is required"))),
    }
}

fn parse_timeout(arguments: &Json) -> Result<Duration, Error> {
    let Some(value) = arguments.get("timeoutMs") else {
        return Ok(DEFAULT_TIMEOUT);
    };
    let Some(ms) = value.as_u64() else {
        return Err(Error::message(
            "timeoutMs must be an integer from 1 to 120000",
        ));
    };
    if ms == 0 || Duration::from_millis(ms) > MAX_TIMEOUT {
        return Err(Error::message(
            "timeoutMs must be an integer from 1 to 120000",
        ));
    }
    Ok(Duration::from_millis(ms))
}

fn map_program(result: Result<String, RunError>, program: &str) -> Result<String, Error> {
    match result {
        Ok(output) => Ok(output),
        Err(RunError::Start(err)) => Err(Error::message(format!("cannot start {program}: {err}"))),
        Err(RunError::Tool(message)) => Err(Error::message(message)),
    }
}

fn run_program(
    root: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, RunError> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    silence_window(&mut cmd);
    scrub_env(&mut cmd);
    let mut child = cmd.spawn().map_err(RunError::Start)?;
    collect(&mut child, timeout)
}

fn collect(child: &mut Child, timeout: Duration) -> Result<String, RunError> {
    let deadline = Instant::now() + timeout;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (Some(stdout), Some(stderr)) = (stdout, stderr) else {
        kill_child(child);
        return Err(RunError::Tool("command output was not captured".into()));
    };
    let shared = Arc::new(Mutex::new(OutputBuf { bytes: Vec::new() }));
    let stdout_thread = match spawn_drain("ez4a-shell-stdout", stdout, Arc::clone(&shared)) {
        Ok(handle) => handle,
        Err(err) => {
            kill_child(child);
            return Err(RunError::Tool(format!("cannot capture output: {err}")));
        }
    };
    let stderr_thread = match spawn_drain("ez4a-shell-stderr", stderr, Arc::clone(&shared)) {
        Ok(handle) => handle,
        Err(err) => {
            kill_child(child);
            detach_reader(stdout_thread);
            return Err(RunError::Tool(format!("cannot capture output: {err}")));
        }
    };
    let waited = wait_for_exit(child, deadline);
    let drained = finish_io(stdout_thread, stderr_thread, deadline);
    let text = snapshot(&shared);
    match waited {
        Wait::Done(status) if drained => exit_result(text, status),
        Wait::Done(_) | Wait::TimedOut => Err(RunError::Tool(timeout_text(&text, timeout))),
        Wait::Error(err) => Err(RunError::Tool(format!("cannot wait for command: {err}"))),
    }
}

fn spawn_drain(
    name: &str,
    pipe: impl Read + Send + 'static,
    shared: Arc<Mutex<OutputBuf>>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(name.into())
        .spawn(move || drain(pipe, shared))
}

fn drain(mut pipe: impl Read, shared: Arc<Mutex<OutputBuf>>) {
    let mut chunk = [0_u8; 8192];
    loop {
        let read = match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut guard = shared.lock().unwrap_or_else(|poison| poison.into_inner());
        append_capped(&mut guard, &chunk[..read]);
    }
}

fn append_capped(buf: &mut OutputBuf, chunk: &[u8]) {
    let len = buf.bytes.len();
    if len >= MAX_OUTPUT_BYTES {
        return;
    }
    let room = MAX_OUTPUT_BYTES - len;
    if chunk.len() <= room {
        buf.bytes.extend_from_slice(chunk);
    } else {
        buf.bytes.extend_from_slice(&chunk[..room]);
    }
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
                thread::sleep(WAIT_SLICE.min(deadline - now));
            }
            Err(err) => {
                kill_child(child);
                return Wait::Error(err);
            }
        }
    }
}

fn finish_io(stdout: JoinHandle<()>, stderr: JoinHandle<()>, deadline: Instant) -> bool {
    if readers_finished(&stdout, &stderr, deadline)
        || readers_finished(&stdout, &stderr, Instant::now() + DRAIN_GRACE)
    {
        let _ = stdout.join();
        let _ = stderr.join();
        true
    } else {
        // A grandchild can keep the pipe open after the child is dead.
        // Detach instead of blocking the tool past the timeout.
        detach_reader(stdout);
        detach_reader(stderr);
        false
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

fn snapshot(shared: &Mutex<OutputBuf>) -> String {
    let guard = shared.lock().unwrap_or_else(|poison| poison.into_inner());
    String::from_utf8_lossy(&guard.bytes).into_owned()
}

fn exit_result(text: String, status: ExitStatus) -> Result<String, RunError> {
    if status.success() {
        return Ok(text);
    }
    if text.is_empty() {
        let message = match status.code() {
            Some(code) => format!("exit {code}"),
            None => "command failed".into(),
        };
        return Err(RunError::Tool(message));
    }
    Err(RunError::Tool(text))
}

fn timeout_text(text: &str, timeout: Duration) -> String {
    let note = format!("command timed out after {}ms", timeout.as_millis());
    if text.is_empty() {
        note
    } else if text.ends_with('\n') {
        format!("{text}{note}")
    } else {
        format!("{text}\n{note}")
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
    cmd.args(["/F", "/T", "/PID", &pid.to_string()])
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
            cmd.env_remove(key);
        }
    }
}

/// `EZ4A_OPENROUTER_KEY`, `OPENROUTER_API_KEY`, and every name containing
/// `KEY`, `TOKEN`, or `SECRET`. Matching is ASCII case-insensitive.
/// A name that is not Unicode is removed because it cannot be checked.
fn env_name_is_sensitive(name: &OsStr) -> bool {
    let Some(text) = name.to_str() else {
        return true;
    };
    if text.eq_ignore_ascii_case("EZ4A_OPENROUTER_KEY")
        || text.eq_ignore_ascii_case("OPENROUTER_API_KEY")
    {
        return true;
    }
    let upper = text.to_ascii_uppercase();
    upper.contains("KEY") || upper.contains("TOKEN") || upper.contains("SECRET")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::time::Duration;

    use serde_json::json;

    use super::{env_name_is_sensitive, parse_timeout, prepare};

    #[test]
    fn strips_key_token_and_secret_names() {
        assert!(env_name_is_sensitive(OsStr::new("EZ4A_OPENROUTER_KEY")));
        assert!(env_name_is_sensitive(OsStr::new("OPENROUTER_API_KEY")));
        assert!(env_name_is_sensitive(OsStr::new("openrouter_api_key")));
        assert!(env_name_is_sensitive(OsStr::new("GH_TOKEN")));
        assert!(env_name_is_sensitive(OsStr::new("aws_secret_access_key")));
        assert!(!env_name_is_sensitive(OsStr::new("PATH")));
        assert!(!env_name_is_sensitive(OsStr::new("HOME")));
        assert!(!env_name_is_sensitive(OsStr::new("CARGO_TARGET_DIR")));
    }

    #[test]
    fn empty_command_and_bad_timeout_do_not_spawn() {
        let err = prepare(&json!({ "command": "" })).unwrap_err();
        assert!(err.to_string().contains("empty"));
        let err = prepare(&json!({})).unwrap_err();
        assert!(err.to_string().contains("required"));
        let err = prepare(&json!({ "command": 1 })).unwrap_err();
        assert!(err.to_string().contains("string"));
        let err = prepare(&json!("nope")).unwrap_err();
        assert!(err.to_string().contains("object"));
        assert_eq!(parse_timeout(&json!({})).unwrap(), Duration::from_secs(30));
        assert_eq!(
            parse_timeout(&json!({ "timeoutMs": 1 })).unwrap(),
            Duration::from_millis(1)
        );
        assert_eq!(
            parse_timeout(&json!({ "timeoutMs": 120_000 })).unwrap(),
            Duration::from_millis(120_000)
        );
        assert!(parse_timeout(&json!({ "timeoutMs": 0 })).is_err());
        assert!(parse_timeout(&json!({ "timeoutMs": 120_001 })).is_err());
        assert!(parse_timeout(&json!({ "timeoutMs": -1 })).is_err());
        assert!(parse_timeout(&json!({ "timeoutMs": 1.5 })).is_err());
        assert!(parse_timeout(&json!({ "timeoutMs": "30000" })).is_err());
    }
}
