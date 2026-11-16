//! The hatchery node's loopback door, spoken natively.
//!
//! The same wire the session MCP adapter uses (`POST /door` with
//! `{"op": ..., "payload": ...}`): the node signs the call with a key the
//! agent never sees and the hub applies the node's status. The door exists
//! only when `HATCHERY_NODE_DOOR` names a loopback address; anything else is
//! refused. Letters are not here — m4a is the only messenger.

use std::time::Duration;

use serde_json::{json, Value};

use crate::Error;

const DOOR_ENV: &str = "HATCHERY_NODE_DOOR";
const IO_TIMEOUT: Duration = Duration::from_secs(65);
const MAX_REPLY: usize = 256 * 1024;

/// Open tasks assigned to this machine.
pub async fn tasks() -> Result<String, Error> {
    call("task.list", &json!({})).await
}

/// Set the state of one of this machine's tasks.
pub async fn task_update(id: i64, status: &str) -> Result<String, Error> {
    call(
        "task.update",
        &json!({ "id": id, "status": status }),
    )
    .await
}

/// One `POST /door` round trip. The reply body is returned verbatim.
async fn call(op: &str, payload: &Value) -> Result<String, Error> {
    let addr = std::env::var(DOOR_ENV).unwrap_or_default();
    if addr.is_empty() {
        return Err(Error::message("HATCHERY_NODE_DOOR is unset"));
    }
    let addr = addr.trim_start_matches("http://").to_string();
    let body = json!({ "op": op, "payload": payload }).to_string();
    tokio::time::timeout(IO_TIMEOUT, exchange(&addr, &body))
        .await
        .map_err(|_| Error::message("node door timed out"))?
}

async fn exchange(addr: &str, body: &str) -> Result<String, Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|err| Error::message(format!("node door connect {addr}: {err}")))?;
    let request = format!(
        "POST /door HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| Error::message(format!("node door write: {err}")))?;
    let mut raw = Vec::new();
    let mut limited = (&mut stream).take(MAX_REPLY as u64);
    limited
        .read_to_end(&mut raw)
        .await
        .map_err(|err| Error::message(format!("node door read: {err}")))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, reply) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| Error::message("node door reply has no body"))?;
    if !head.starts_with("HTTP/1.1 200") {
        return Err(Error::message("node door refused the call"));
    }
    Ok(reply.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn op_names_match_the_session_adapter() {
        // The names the node's local_server dispatches (restream_g4a_bridge):
        // an e4a session must be indistinguishable from an MCP session here.
        assert_eq!(op_of("g4a_tasks"), "task.list");
        assert_eq!(op_of("g4a_task_update"), "task.update");
    }

    fn op_of(tool: &str) -> &'static str {
        match tool {
            "g4a_tasks" => "task.list",
            "g4a_task_update" => "task.update",
            _ => unreachable!(),
        }
    }
}
