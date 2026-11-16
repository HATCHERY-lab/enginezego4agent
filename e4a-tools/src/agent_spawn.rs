//! The manager's `agent` tool: spawn a worker as its own process.
//!
//! A subagent is a real process — `e4a run --root <dir> --prompt <text>` — so
//! it gets its own journal, its own crash domain, and its own attestable mail
//! identity. The worker tool set has no `agent` tool: workers do not spawn
//! workers (owner directive 2026-11-17). The child's stdout answer is the
//! tool result the manager sees.

use std::path::PathBuf;
use std::time::Duration;

use crate::{Error, Workspace};

const SPAWN_TIMEOUT: Duration = Duration::from_secs(300);

/// Run one worker subagent under `workspace` and return its answer text.
///
/// The child root is `<workspace>/subagents/agent-<n>`, the first free slot.
/// The binary is `E4A_BIN` when set, else the current executable — the same
/// `e4a` binary serving the manager. A non-zero exit, a timeout, or an empty
/// answer is an error result for the manager.
pub async fn run_agent_child(workspace: &Workspace, prompt: &str) -> Result<String, Error> {
    if prompt.trim().is_empty() {
        return Err(Error::message("agent prompt is empty"));
    }
    let root = child_root(workspace)?;
    let binary = std::env::var("E4A_BIN").unwrap_or_default();
    let binary = if binary.is_empty() {
        std::env::current_exe().map_err(|err| Error::message(err.to_string()))?
    } else {
        binary.into()
    };
    let mut command = tokio::process::Command::new(&binary);
    command
        .arg("run")
        .arg("--root")
        .arg(&root)
        .arg("--prompt")
        .arg(prompt)
        .current_dir(workspace.root())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // SAFETY of the boundary: the prompt travels as one argv element, never
    // through a shell, so a model cannot smuggle extra flags or commands.
    let output = tokio::time::timeout(SPAWN_TIMEOUT, command.output())
        .await
        .map_err(|_| Error::message("agent child timed out"))?
        .map_err(|err| Error::message(format!("agent child did not start: {err}")))?;
    let answer = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::message(format!(
            "agent child failed ({}): {}",
            output.status, stderr
        )));
    }
    if answer.is_empty() {
        return Err(Error::message("agent child returned no answer"));
    }
    Ok(answer)
}

/// First free `subagents/agent-<n>` slot under the workspace.
fn child_root(workspace: &Workspace) -> Result<PathBuf, Error> {
    let base = workspace.root().join("subagents");
    std::fs::create_dir_all(&base).map_err(|err| Error::message(err.to_string()))?;
    for n in 1..=999 {
        let dir = base.join(format!("agent-{n}"));
        if !dir.exists() {
            std::fs::create_dir(&dir).map_err(|err| Error::message(err.to_string()))?;
            return Ok(dir);
        }
    }
    Err(Error::message("subagent slots are exhausted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_prompt_is_refused_before_anything_spawns() {
        let root = std::env::temp_dir().join(format!("e4a-spawn-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let workspace = Workspace::new(&root).unwrap();
        let err = futures_probe(workspace).unwrap_err();
        assert_eq!(err.to_string(), "agent prompt is empty");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn futures_probe(workspace: Workspace) -> Result<String, crate::Error> {
        // The empty-prompt check runs before any await, so a plain call on a
        // current-thread runtime covers it without spawning.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        runtime.block_on(run_agent_child(&workspace, "  "))
    }
}
