// engine4agent ACP host, not a copy of pi-durable (ACP is not in pi-durable).

//! Newline-delimited JSON-RPC on stdin and stdout.
//!
//! This process does not open a socket, read a key, or open a session store.
//! [`handle_line`] does not call a model.

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
/// Known methods return one JSON-RPC line ending in `\n`. `session/prompt`
/// answers `{stopReason:"end_turn"}` and does not call a model.
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
        "session/prompt" => result(id, json!({"stopReason": "end_turn"})),
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
    fn prompt_ends_turn_without_calling_a_model() {
        let line = handle_line(
            r#"{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"/repo","prompt":[{"type":"text","text":"hi"}]}}"#,
        )
        .expect("prompt");
        let value = parse_reply(&line);
        assert_eq!(value["result"]["stopReason"], json!("end_turn"));
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
