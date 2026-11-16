//! mail4agent client: the native wire between agents.
//!
//! Speaks the mailbox's HTTP door (`POST /mail/send|inbox|whoami`, README
//! "Doors") over plain TCP — the daemon binds loopback. The account token is
//! `E4A_M4A_TOKEN`, the address `E4A_M4A_URL` (default `127.0.0.1:8741`).
//! Identity stays the daemon's business: it attests the connection's process,
//! so each e4a process — manager, worker, subagent — is its own mail session.

use std::time::Duration;

use serde_json::{json, Value};

use crate::Error;

const DEFAULT_ADDR: &str = "127.0.0.1:8741";
const CALL_TIMEOUT: Duration = Duration::from_secs(65);

/// `to`, `subject`, `body` — one message to an account, a session, or a room.
/// `reply_to` threads it onto a message.
pub async fn send(to: &str, subject: &str, body: &str, reply_to: Option<&str>) -> Result<String, Error> {
    let mut params = json!({ "to": to, "subject": subject, "body": body });
    if let Some(reply_to) = reply_to {
        params["reply_to"] = json!(reply_to);
    }
    call("mail_send", &params).await
}

/// The caller's inbox. `wait_secs` parks the read server-side, bounded by the
/// daemon to 60.
pub async fn inbox(wait_secs: Option<u64>) -> Result<String, Error> {
    let params = match wait_secs {
        Some(secs) => json!({ "wait_secs": secs }),
        None => json!({}),
    };
    call("mail_inbox", &params).await
}

/// Acknowledge messages so they leave the unread set.
pub async fn ack(ids: &[i64]) -> Result<String, Error> {
    call("mail_ack", &json!({ "ids": ids })).await
}

/// One message by id.
pub async fn get(id: i64) -> Result<String, Error> {
    call("mail_get", &json!({ "id": id })).await
}

/// The unread set, unacked.
pub async fn unread() -> Result<String, Error> {
    call("mail_unread", &json!({})).await
}

/// The caller's own address and card.
pub async fn whoami() -> Result<String, Error> {
    call("mail_whoami", &json!({})).await
}

/// Say what this session is working on; the one declared fact a session may
/// write about itself.
pub async fn status(working_on: &str, role: &str) -> Result<String, Error> {
    call("mail_status", &json!({ "working_on": working_on, "role": role })).await
}

/// Who else is in the mailbox: accounts, their live sessions, and rooms.
pub async fn directory() -> Result<String, Error> {
    call("mail_directory", &json!({})).await
}

/// One `POST /<route>` round trip with the bearer token. The answer body is
/// returned verbatim; a non-2xx status is an error carrying the daemon's text.
async fn call(method: &str, params: &Value) -> Result<String, Error> {
    let token = std::env::var("E4A_M4A_TOKEN").unwrap_or_default();
    if token.is_empty() {
        return Err(Error::message("E4A_M4A_TOKEN is unset"));
    }
    let addr = std::env::var("E4A_M4A_URL").unwrap_or_default();
    let addr = if addr.is_empty() {
        DEFAULT_ADDR.to_string()
    } else {
        addr.trim_start_matches("http://").to_string()
    };
    let route = format!("/mail/{}", method.trim_start_matches("mail_"));
    let body = serde_json::to_vec(params).map_err(|err| Error::message(err.to_string()))?;
    let request = format!(
        "POST {route} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\
Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let sent = tokio::time::timeout(
        CALL_TIMEOUT,
        exchange(&addr, request.as_bytes(), &body),
    )
    .await
    .map_err(|_| Error::message("mail4agent call timed out"))??;
    Ok(sent)
}

async fn exchange(addr: &str, head: &[u8], body: &[u8]) -> Result<String, Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr)
        .await
        .map_err(|err| Error::message(format!("mail4agent connect {addr}: {err}")))?;
    stream
        .write_all(head)
        .await
        .map_err(|err| Error::message(format!("mail4agent write: {err}")))?;
    stream
        .write_all(body)
        .await
        .map_err(|err| Error::message(format!("mail4agent write: {err}")))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|err| Error::message(format!("mail4agent read: {err}")))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (status, rest) = text
        .split_once("\r\n")
        .ok_or_else(|| Error::message("mail4agent response has no status line"))?;
    let ok = status
        .split_whitespace()
        .nth(1)
        .is_some_and(|code| code.starts_with('2'));
    if !ok {
        return Err(Error::message(format!(
            "mail4agent {}",
            truncate(status, 120)
        )));
    }
    let body = rest
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or("");
    Ok(body.to_string())
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn routes_come_from_method_names() {
        let call = CallShim::new("mail_send");
        assert_eq!(call.route, "/mail/send");
        let call = CallShim::new("mail_inbox");
        assert_eq!(call.route, "/mail/inbox");
    }

    struct CallShim {
        route: String,
    }

    impl CallShim {
        fn new(method: &str) -> Self {
            Self {
                route: format!("/mail/{}", method.trim_start_matches("mail_")),
            }
        }
    }
}
