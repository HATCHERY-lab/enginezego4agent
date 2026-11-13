// engine4agent ACP host, not a copy of pi-durable (ACP is not in pi-durable).

//! Newline-delimited JSON-RPC on stdin and stdout.
//!
//! The journal is the store named by `E4A_STORE`. Sqlite and JSONL sit under
//! the workspace root. This process does not print a key.
//! [`handle_line`] runs `session/prompt` through [`e4a_tools::run_until_answer`].
//! The model is scripted unless both `E4A_SOCKS` and the openrouter key from
//! `e4a_harness::resolve` are non-empty. That case dials OpenRouter. A dial or
//! turn failure is JSON-RPC `-32603`, and the key is not copied into the message.

use std::path::Path;

use e4a_harness::{resolve, shutdown, ProviderSessions, TurnInput, TurnKind};
use e4a_llm::{FauxLlm, Llm, OpenRouterClient, ToolSchema, Usage};
use e4a_session::{EntryRecord, Session, SessionEnd, StoreKind, ROOT_CONVERSATION_ID};
use e4a_tools::{run_until_answer, Workspace};
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
/// `session/prompt` with one text block runs the tool loop on the `E4A_STORE`
/// journal and returns `{stopReason:"end_turn", text}` when the model answers.
/// The workspace root is `cwd`, or `sessionId` when `cwd` is absent.
/// [`prompt_backend`] is `faux` unless both `E4A_SOCKS` and the key from
/// `e4a_harness::resolve("openrouter")` are non-empty, in which case the turn
/// dials OpenRouter. Missing prompt text, or a missing workspace root, is
/// JSON-RPC `-32602`. A failure is JSON-RPC `-32603` and does not include the key.
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

/// `cwd` when it is a non-empty string. Otherwise `sessionId`, the id `session/new` returns.
fn workspace_root(params: Option<&Value>) -> Option<&str> {
    if let Some(cwd) = cwd_param(params) {
        return Some(cwd);
    }
    let session_id = params?.get("sessionId")?.as_str()?;
    if session_id.is_empty() {
        None
    } else {
        Some(session_id)
    }
}

fn session_prompt(id: Value, params: Option<&Value>) -> Value {
    let Some(user_text) = prompt_text(params) else {
        return rpc_error(id, -32602, "prompt must be one non-empty text block");
    };
    let Some(root) = workspace_root(params) else {
        return rpc_error(id, -32602, "cwd or sessionId must be a non-empty string");
    };
    let socks = std::env::var("E4A_SOCKS").ok();
    // Err means no key. The error is not copied into the reply.
    let key = resolve("openrouter").ok();
    let openrouter = prompt_backend(socks.as_deref(), key.as_deref()) == "openrouter";
    // Sync stdin loop: one current-thread runtime for this turn. IO and time
    // are on so OpenRouter can dial; faux does not use them. The key stays in
    // this frame and is not copied into the reply.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return rpc_error(id, -32603, &err.to_string()),
    };
    let answered = if openrouter {
        runtime.block_on(openrouter_prompt(
            user_text,
            root,
            socks.as_deref().unwrap_or(""),
            key.as_deref().unwrap_or(""),
            openrouter,
        ))
    } else {
        runtime.block_on(faux_prompt(user_text, root, openrouter))
    };
    match answered {
        Ok((text, tail)) => result(
            id,
            json!({"stopReason": "end_turn", "text": text, "tail": tail}),
        ),
        Err(err) => {
            let message = if openrouter {
                redact_key(&err, key.as_deref().unwrap_or(""))
            } else {
                err
            };
            rpc_error(id, -32603, &message)
        }
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

async fn faux_prompt(
    user_text: &str,
    root: &str,
    openrouter: bool,
) -> Result<(String, Vec<String>), String> {
    let llm = FauxLlm::text(user_text).with_usage(Usage {
        input_tokens: 1,
        output_tokens: 1,
        provider: "faux".into(),
        model: "scripted".into(),
    });
    memory_prompt(user_text, root, &llm, openrouter).await
}

async fn openrouter_prompt(
    user_text: &str,
    root: &str,
    socks: &str,
    key: &str,
    openrouter: bool,
) -> Result<(String, Vec<String>), String> {
    let from_env = std::env::var("E4A_MODEL").ok();
    let model = openrouter_model(from_env.as_deref());
    let llm = OpenRouterClient::new(model)
        .with_socks(socks)
        .with_key(key);
    memory_prompt(user_text, root, &llm, openrouter).await
}

/// `E4A_MODEL` when it is set and non-empty. Otherwise `openrouter/auto`.
fn openrouter_model(from_env: Option<&str>) -> String {
    match from_env {
        Some(model) if !model.is_empty() => model.to_string(),
        _ => "openrouter/auto".to_string(),
    }
}

async fn memory_prompt(
    user_text: &str,
    root: &str,
    llm: &impl Llm,
    openrouter: bool,
) -> Result<(String, Vec<String>), String> {
    let session = open_journal(root).await?;
    let provider = if openrouter { "openrouter" } else { "faux" };
    let mut sessions = ProviderSessions::default();
    if let Err(err) = sessions.remember("e4a", provider) {
        let _ = close_journal(&session).await;
        return Err(err);
    }
    let answered = turn_text(&session, llm, user_text, root).await;
    let released = shutdown(&mut sessions, "e4a");
    // A missed release outranks the turn and the journal close. The turn outranks close.
    let shutdown_err = if released == vec![provider.to_string()] {
        None
    } else {
        Some("provider session was not released".to_string())
    };
    let closed = close_journal(&session).await;
    if let Some(err) = shutdown_err {
        return Err(err);
    }
    match (answered, closed) {
        (Ok(text), Ok(())) => Ok(text),
        (Ok(_), Err(err)) | (Err(err), _) => Err(err),
    }
}

async fn close_journal(session: &Session) -> Result<(), String> {
    session
        .close()
        .await
        .map_err(|err| err.to_string())
        .and_then(|end| match end {
            SessionEnd::Closed => Ok(()),
            SessionEnd::Failed { error } => Err(error),
        })
}

/// Journal under `root`. Unset, empty, and `sqlite` open `e4a-session.sqlite`.
/// `jsonl` opens `e4a-session-jsonl`. `idb` opens the name `e4a-session` and
/// does not use `root`. Any other `E4A_STORE` is `unknown session store` and
/// does not open a session.
async fn open_journal(root: &str) -> Result<Session, String> {
    let configured = match std::env::var("E4A_STORE") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("unknown session store".to_string());
        }
    };
    let root = Path::new(root);
    let opened = match configured.as_str() {
        "" | "sqlite" => {
            let path = root.join("e4a-session.sqlite");
            Session::open_kind(StoreKind::Sqlite, &path).await
        }
        "jsonl" => {
            let path = root.join("e4a-session-jsonl");
            Session::open_kind(StoreKind::Jsonl, &path).await
        }
        "idb" => Session::open_kind(StoreKind::Idb, Path::new("e4a-session")).await,
        _ => return Err("unknown session store".to_string()),
    };
    opened.map_err(|err| err.to_string())
}

async fn turn_text(
    session: &Session,
    llm: &impl Llm,
    user_text: &str,
    root: &str,
) -> Result<(String, Vec<String>), String> {
    let workspace = Workspace::new(root).map_err(|err| err.to_string())?;
    e4a_tools::resume_waiting(session, &workspace)
        .await
        .map_err(|err| err.to_string())?;
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: user_text.to_string(),
        tools: offered_tools(),
    };
    let outcome = run_until_answer(session, llm, &workspace, &input)
        .await
        .map_err(|err| err.to_string())?;
    let TurnKind::Answered { entry_id } = outcome.kind else {
        return Err("turn did not answer".to_string());
    };
    let entry = session
        .entry(entry_id)
        .await
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("assistant entry {entry_id} is missing"))?;
    Ok((entry_text(&entry)?, outcome.tail))
}

/// Replace `key` with `[redacted]` when the message contains it.
///
/// An empty key matches every string, so it is left unchanged. The key is not written anywhere else.
fn redact_key(message: &str, key: &str) -> String {
    if key.is_empty() || !message.contains(key) {
        message.to_string()
    } else {
        message.replace(key, "[redacted]")
    }
}

/// Filesystem and shell tools match `e4a-tools` `examples/live.rs`.
/// `code` and `resize` are offered here as well.
fn offered_tools() -> Vec<ToolSchema> {
    vec![
        schema(
            "read",
            "Read a UTF-8 file under the workspace root.",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        schema(
            "write",
            "Write a UTF-8 file under the workspace root.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "content":{"type":"string"}
                },
                "required":["path","content"]
            }),
        ),
        schema(
            "edit",
            "Replace one exact occurrence in a file. Fails if the text occurs zero times or more than once.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "old":{"type":"string"},
                    "new":{"type":"string"}
                },
                "required":["path","old","new"]
            }),
        ),
        schema(
            "edit_all",
            "Replace every non-overlapping occurrence in a file.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "old":{"type":"string"},
                    "new":{"type":"string"}
                },
                "required":["path","old","new"]
            }),
        ),
        schema(
            "jev",
            "Ask the fast gate one question. It may read one file, then answers continue, stop, compact, or steer. It does not write or run commands.",
            json!({
                "type":"object",
                "properties":{"question":{"type":"string"}},
                "required":["question"]
            }),
        ),
        schema(
            "advise",
            "Ask the read-only advisor one question. It does not write or run commands.",
            json!({
                "type":"object",
                "properties":{"question":{"type":"string"}},
                "required":["question"]
            }),
        ),
        schema(
            "child",
            "Run one command in the workspace jail for at most 30 seconds.",
            json!({
                "type":"object",
                "properties":{"command":{"type":"string"}},
                "required":["command"]
            }),
        ),
        schema(
            "powershell",
            "Run one PowerShell command in the workspace. Empty command is an error.",
            json!({
                "type":"object",
                "properties":{
                    "command":{"type":"string"}
                },
                "required":["command"]
            }),
        ),
        schema(
            "code",
            "Evaluate one script in the code-mode sandbox.",
            json!({
                "type":"object",
                "properties":{
                    "script":{"type":"string"}
                },
                "required":["script"]
            }),
        ),
        schema(
            "resize",
            "Resize a PNG in the workspace.",
            json!({
                "type":"object",
                "properties":{
                    "path":{"type":"string"},
                    "w":{"type":"integer"},
                    "h":{"type":"integer"}
                },
                "required":["path","w","h"]
            }),
        ),
    ]
}

fn schema(name: &str, description: &str, parameters: Value) -> ToolSchema {
    ToolSchema {
        name: name.into(),
        description: description.into(),
        parameters,
    }
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
    fn openrouter_model_uses_env_or_auto() {
        assert_eq!(openrouter_model(None), "openrouter/auto");
        assert_eq!(openrouter_model(Some("")), "openrouter/auto");
        assert_eq!(openrouter_model(Some("vendor/model")), "vendor/model");
    }

    #[test]
    fn dial_error_replaces_the_key() {
        assert_eq!(redact_key("dial failed", "sk-test"), "dial failed");
        assert_eq!(
            redact_key("authorization: sk-test", "sk-test"),
            "authorization: [redacted]"
        );
        assert_eq!(
            redact_key("sk-test and sk-test", "sk-test"),
            "[redacted] and [redacted]"
        );
        assert_eq!(redact_key("keep", ""), "keep");
    }

    #[test]
    fn prompt_returns_scripted_text() {
        let root = std::env::temp_dir();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/prompt",
            "params": {
                "sessionId": root,
                "prompt": [{"type": "text", "text": "hi"}]
            }
        });
        let line = handle_line(&request.to_string()).expect("prompt");
        let value = parse_reply(&line);
        assert_eq!(value["id"], json!(3));
        assert_eq!(value["result"]["stopReason"], json!("end_turn"));
        // FauxLlm::text replays the script it was given, which is the user text.
        assert_eq!(value["result"]["text"], json!("hi"));
        assert!(value["result"]["tail"].is_array());
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
