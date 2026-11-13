// engine4agent ACP host, not a copy of pi-durable (ACP is not in pi-durable).

//! Newline-delimited JSON-RPC on stdin and stdout.
//!
//! This process does not open a socket or a file store, and it does not print a key.
//! [`handle_line`] runs `session/prompt` as one in-memory faux turn unless both
//! `E4A_SOCKS` and `E4A_OPENROUTER_KEY` are non-empty. That case returns
//! JSON-RPC `-32000` and does not dial.

use e4a_harness::{Generation, TurnInput, TurnKind};
use e4a_llm::{FauxLlm, Usage};
use e4a_session::{EntryRecord, Session, SessionEnd, ROOT_CONVERSATION_ID};
use serde_json::{json, Value};

/// Subcommand of `e4a`. [`Command::Acp`] is the only subcommand besides help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Serve ACP on stdin and stdout.
    Acp,
    /// Print [`USAGE`] and exit successfully.
    Help,
}

/// Usage text for help, no arguments, and unknown arguments.
pub const USAGE: &str = "usage: e4a acp\n";

/// Parse arguments after the program name.
///
/// [`Command::Acp`] and help are the only accepted forms. Anything else,
/// including no arguments, is [`Err`] so the process can exit 2.
pub fn parse_args<I, S>(args: I) -> Result<Command, ()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Err(());
    };
    if args.next().is_some() {
        return Err(());
    }
    match first.as_ref() {
        "acp" => Ok(Command::Acp),
        "help" | "--help" | "-h" => Ok(Command::Help),
        _ => Err(()),
    }
}

/// One JSON-RPC request line: `{"jsonrpc":"2.0","method":method,"params":body}` plus `\n`.
///
/// No LSP `Content-Length` framing.
pub fn frame(method: &str, body: &Value) -> String {
    let method = serde_json::to_string(method).unwrap_or_else(|_| "\"\"".to_string());
    let params = serde_json::to_string(body).unwrap_or_else(|_| "null".to_string());
    format!("{{\"jsonrpc\":\"2.0\",\"method\":{method},\"params\":{params}}}\n")
}

/// Handle one stdin line.
///
/// A line that is not a JSON object with a string `method` returns [`Err`].
/// Known methods return one JSON-RPC line ending in `\n`.
///
/// `session/prompt` with one text block runs one faux generation and returns
/// `{stopReason:"end_turn", text}` when [`prompt_backend`] is `faux`.
/// When both `E4A_SOCKS` and `E4A_OPENROUTER_KEY` are non-empty, the reply is
/// JSON-RPC `-32000` and nothing is dialed. Missing prompt text is JSON-RPC `-32602`.
pub fn handle_line(line: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(line).map_err(|err| format!("invalid JSON: {err}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "JSON-RPC message must be an object".to_string())?;
    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "JSON-RPC message requires a string method".to_string())?;
    let id = obj.get("id").cloned().unwrap_or(Value::Null);
    let params = obj.get("params");
    let reply = match method {
        "initialize" => result(
            id,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": {"loadSession": false}
            }),
        ),
        "session/new" => session_new(id, params),
        "session/prompt" => session_prompt(id, params),
        _ => rpc_error(id, -32601, "Method not found"),
    };
    to_line(&reply)
}

fn session_new(id: Value, params: Option<&Value>) -> Value {
    match cwd_param(params) {
        Some(cwd) => result(id, json!({"sessionId": cwd})),
        None => rpc_error(id, -32602, "cwd must be a non-empty string"),
    }
}

fn cwd_param(params: Option<&Value>) -> Option<&str> {
    let cwd = params?.get("cwd")?.as_str()?;
    if cwd.is_empty() { None } else { Some(cwd) }
}

fn session_prompt(id: Value, params: Option<&Value>) -> Value {
    let Some(user_text) = prompt_text(params) else {
        return rpc_error(id, -32602, "prompt must be one non-empty text block");
    };
    let socks = std::env::var("E4A_SOCKS").ok();
    let key = std::env::var("E4A_OPENROUTER_KEY").ok();
    if prompt_backend(socks.as_deref(), key.as_deref()) == "openrouter" {
        // Dial is the next slice. The key stays in this frame and is not copied into the reply.
        return rpc_error(id, -32000, "openrouter dial is the next slice");
    }
    // Sync stdin loop: one current-thread runtime for this turn, no process and no network.
    let runtime = match tokio::runtime::Builder::new_current_thread().build() {
        Ok(runtime) => runtime,
        Err(err) => return rpc_error(id, -32603, &err.to_string()),
    };
    match runtime.block_on(faux_prompt(user_text)) {
        Ok(text) => result(id, json!({"stopReason": "end_turn", "text": text})),
        Err(err) => rpc_error(id, -32603, &err),
    }
}

/// `openrouter` only when both `socks` and `key` are non-empty. Otherwise `faux`.
///
/// Does not read the process environment and does not dial.
fn prompt_backend(socks: Option<&str>, key: Option<&str>) -> &'static str {
    match (socks, key) {
        (Some(socks), Some(key)) if !socks.is_empty() && !key.is_empty() => "openrouter",
        _ => "faux",
    }
}

fn prompt_text(params: Option<&Value>) -> Option<&str> {
    let blocks = params?.get("prompt")?.as_array()?;
    let [block] = blocks.as_slice() else {
        return None;
    };
    let block = block.as_object()?;
    if block.get("type")?.as_str()? != "text" {
        return None;
    }
    let text = block.get("text")?.as_str()?;
    if text.is_empty() { None } else { Some(text) }
}

async fn faux_prompt(user_text: &str) -> Result<String, String> {
    let session = Session::memory().await.map_err(|err| err.to_string())?;
    let answered = turn_text(&session, user_text).await;
    let closed = session
        .close()
        .await
        .map_err(|err| err.to_string())
        .and_then(|end| match end {
            SessionEnd::Closed => Ok(()),
            SessionEnd::Failed { error } => Err(error),
        });
    match (answered, closed) {
        (Ok(text), Ok(())) => Ok(text),
        (Ok(_), Err(err)) | (Err(err), _) => Err(err),
    }
}

async fn turn_text(session: &Session, user_text: &str) -> Result<String, String> {
    let llm = FauxLlm::text(user_text).with_usage(Usage {
        input_tokens: 1,
        output_tokens: 1,
        provider: "faux".into(),
        model: "scripted".into(),
    });
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: user_text.to_string(),
        tools: Vec::new(),
    };
    let outcome = Generation::turn(session, &llm, &input)
        .await
        .map_err(|err| err.to_string())?;
    let TurnKind::Answered { entry_id } = outcome.kind else {
        return Err("faux turn did not answer".to_string());
    };
    let entry = session
        .entry(entry_id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("assistant entry {entry_id} is missing"))?;
    entry_text(&entry)
}

fn entry_text(entry: &EntryRecord) -> Result<String, String> {
    entry
        .model
        .as_ref()
        .and_then(|model| model.get(0))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.get(0))
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "assistant entry has no text".to_string())
}

fn result(id: Value, value: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": value})
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

fn to_line(value: &Value) -> Result<String, String> {
    let mut line = serde_json::to_string(value).map_err(|err| err.to_string())?;
    line.push('\n');
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_reply(line: &str) -> Value {
        assert!(line.ends_with('\n'), "reply must be one line");
        assert_eq!(line.matches('\n').count(), 1);
        serde_json::from_str(line.trim_end()).expect("reply is JSON")
    }

    #[test]
    fn frame_is_one_jsonrpc_line_without_lsp_framing() {
        let line = frame("ping", &json!({}));
        assert_eq!(line, "{\"jsonrpc\":\"2.0\",\"method\":\"ping\",\"params\":{}}\n");
        assert!(!line.contains("Content-Length"));
    }

    #[test]
    fn initialize_advertises_protocol_one_and_no_load_session() {
        let line = handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#)
            .expect("initialize");
        let value = parse_reply(&line);
        assert_eq!(value["jsonrpc"], json!("2.0"));
        assert_eq!(value["id"], json!(1));
        assert_eq!(value["result"]["protocolVersion"], json!(1));
        assert_eq!(value["result"]["agentCapabilities"]["loadSession"], json!(false));
    }

    #[test]
    fn session_new_uses_cwd_as_session_id() {
        let line = handle_line(
            r#"{"jsonrpc":"2.0","id":"s","method":"session/new","params":{"cwd":"/repo"}}"#,
        )
        .expect("session/new");
        let value = parse_reply(&line);
        assert_eq!(value["id"], json!("s"));
        assert_eq!(value["result"]["sessionId"], json!("/repo"));
    }

    #[test]
    fn session_new_rejects_empty_or_missing_cwd() {
        for line in [
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":""}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":1}}"#,
        ] {
            let reply = handle_line(line).expect("protocol error");
            let value = parse_reply(&reply);
            assert_eq!(value["error"]["code"], json!(-32602));
        }
    }

    #[test]
    fn prompt_backend_is_openrouter_only_when_both_are_set() {
        assert_eq!(prompt_backend(None, None), "faux");
        assert_eq!(prompt_backend(Some(""), None), "faux");
        assert_eq!(prompt_backend(None, Some("")), "faux");
        assert_eq!(prompt_backend(Some(""), Some("")), "faux");
        assert_eq!(prompt_backend(Some("socks5h://127.0.0.1:1080"), None), "faux");
        assert_eq!(prompt_backend(None, Some("sk-test")), "faux");
        assert_eq!(prompt_backend(Some(""), Some("sk-test")), "faux");
        assert_eq!(prompt_backend(Some("socks5h://127.0.0.1:1080"), Some("")), "faux");
        assert_eq!(
            prompt_backend(Some("socks5h://127.0.0.1:1080"), Some("sk-test")),
            "openrouter"
        );
    }

    #[test]
    fn prompt_returns_scripted_text() {
        let line = handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"/repo","prompt":[{"type":"text","text":"hi"}]}}"#,
        )
        .expect("prompt");
        let value = parse_reply(&line);
        assert_eq!(value["id"], json!(3));
        assert_eq!(value["result"]["stopReason"], json!("end_turn"));
        // FauxLlm::text replays the script it was given, which is the user text.
        assert_eq!(value["result"]["text"], json!("hi"));
    }

    #[test]
    fn prompt_rejects_missing_text() {
        for line in [
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"/repo"}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"prompt":[]}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"prompt":[{"type":"text"}]}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"prompt":[{"type":"text","text":""}]}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt"}"#,
        ] {
            let reply = handle_line(line).expect("protocol error");
            let value = parse_reply(&reply);
            assert_eq!(value["error"]["code"], json!(-32602));
        }
    }

    #[test]
    fn unknown_method_is_not_found() {
        let line = handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"session/load"}"#)
            .expect("error reply");
        let value = parse_reply(&line);
        assert_eq!(value["id"], json!(4));
        assert_eq!(value["error"]["code"], json!(-32601));
    }

    #[test]
    fn non_json_returns_an_error_string() {
        for line in ["", "   ", "not-json", "1", "[1]", "{\"id\":1}", "{\"method\":1}"] {
            let err = handle_line(line).expect_err("must not panic");
            assert!(!err.is_empty());
        }
    }

    #[test]
    fn acp_is_the_only_subcommand_besides_help() {
        assert_eq!(parse_args(["acp"]), Ok(Command::Acp));
        assert_eq!(parse_args(["help"]), Ok(Command::Help));
        assert_eq!(parse_args(["--help"]), Ok(Command::Help));
        assert_eq!(parse_args(["-h"]), Ok(Command::Help));
        assert!(parse_args(Vec::<&str>::new()).is_err());
        assert!(parse_args(["session"]).is_err());
        assert!(parse_args(["acp", "extra"]).is_err());
        assert!(USAGE.contains("e4a acp"));
    }
}
