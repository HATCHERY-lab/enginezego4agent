// engine zero for agents ACP host, not a copy of pi-durable (ACP is not in pi-durable).

//! Newline-delimited JSON-RPC on stdin and stdout.
//!
//! The journal is the store named by `EZ4A_STORE`. Sqlite and JSONL sit under
//! the workspace root. This process does not print a key.
//! [`handle_line`] runs `session/prompt` through [`ez4a_tools::run_until_answer`].
//! The model is scripted unless both `EZ4A_SOCKS` and the openrouter key from
//! `ez4a_harness::resolve` are non-empty. That case dials OpenRouter. A dial or
//! turn failure is JSON-RPC `-32603`, and the key is not copied into the message.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use ez4a_harness::{resolve, shutdown, ProviderSessions, TurnInput, TurnKind};
use ez4a_llm::{FauxLlm, Llm, OpenRouterClient, ToolSchema, Usage};
use ez4a_session::{EntryRecord, Session, SessionEnd, StoreKind, ROOT_CONVERSATION_ID};
use ez4a_tools::{request_cancel_session, run_until_answer_in, Workspace};
use serde_json::{json, Value};

/// Subcommand of `ez4a`: serve ACP, run one worker turn, or print help.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Serve ACP on stdin and stdout.
    Acp,
    /// One hired-role turn in its own root; the answer goes to stdout.
    Run { root: String, prompt: String, role: String },
    /// Print [`USAGE`] and exit successfully.
    Help,
}

/// Usage text for help, no arguments, and unknown arguments.
pub const USAGE: &str = "usage: ez4a acp | ez4a run --root <dir> --prompt <text>\n";

/// Parse arguments after the program name.
///
/// [`Command::Acp`] and help are the only accepted forms. Anything else,
/// including no arguments, is [`Err`] so the process can exit 2.
pub fn parse_args<I, S>(args: I) -> Result<Command, ()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect();
    let Some(first) = args.first() else {
        return Err(());
    };
    match first.as_str() {
        "acp" if args.len() == 1 => Ok(Command::Acp),
        "run" => parse_run(&args[1..]),
        "help" | "--help" | "-h" if args.len() == 1 => Ok(Command::Help),
        _ => Err(()),
    }
}

/// `run --root <dir> --prompt <text>`, both flags required, in any order.
fn parse_run(args: &[String]) -> Result<Command, ()> {
    let mut root = None;
    let mut prompt = None;
    let mut role = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--root" => {
                root = args.get(index + 1).cloned();
                index += 2;
            }
            "--prompt" => {
                prompt = args.get(index + 1).cloned();
                index += 2;
            }
            "--role" => {
                role = args.get(index + 1).cloned();
                index += 2;
            }
            _ => return Err(()),
        }
    }
    Ok(Command::Run {
        root: root.ok_or(())?,
        prompt: prompt.ok_or(())?,
        role: role.unwrap_or_else(|| "dev".into()),
    })
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
/// `session/prompt` with one text block runs the tool loop on the `EZ4A_STORE`
/// journal and returns `{stopReason:"end_turn", text}` when the model answers.
/// The workspace root is `cwd`, or `sessionId` when `cwd` is absent.
/// [`prompt_backend`] is `faux` unless both `EZ4A_SOCKS` and the key from
/// `ez4a_harness::resolve("openrouter")` are non-empty, in which case the turn
/// dials OpenRouter. Missing prompt text, or a missing workspace root, is
/// JSON-RPC `-32602`. A failure is JSON-RPC `-32603` and does not include the key.
/// One request may emit `session/update` notifications and then the response.
///
/// `session/cancel` is a notification: the vec is empty. Other methods put the
/// JSON-RPC response last.
pub fn handle_exchange(line: &str) -> Result<Vec<String>, String> {
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
    let values = match method {
        "initialize" => vec![initialize(id)],
        "session/new" => vec![session_new(id, params)],
        "session/load" => session_load(id, params),
        "session/prompt" => session_prompt(id, params),
        "session/cancel" => {
            let session_id = params
                .and_then(|params| params.get("sessionId"))
                .and_then(Value::as_str)
                .unwrap_or("*");
            request_cancel_session(session_id);
            Vec::new()
        }
        "session/close" => vec![session_close(id, params)],
        "session/list" => vec![session_list(id)],
        "session/set_model" => vec![session_set_model(id, params)],
        "session/set_config_option" => vec![session_set_config(id, params)],
        _ => vec![rpc_error(id, -32601, "Method not found")],
    };
    values.into_iter().map(|value| to_line(&value)).collect()
}

/// The last line of [`handle_exchange`]. A cancel notification is an empty string.
pub fn handle_line(line: &str) -> Result<String, String> {
    let mut lines = handle_exchange(line)?;
    Ok(lines.pop().unwrap_or_default())
}

fn initialize(id: Value) -> Value {
    result(
        id,
        json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {
                    "image": false,
                    "audio": false,
                    "embeddedContext": false
                }
            },
            "agentInfo": {
                "name": "ez4a",
                "title": "engine zero for agents",
                "version": "1.5.3"
            },
            "authMethods": []
        }),
    )
}

fn session_new(id: Value, params: Option<&Value>) -> Value {
    match cwd_param(params) {
        Some(cwd) => {
            remember_session(cwd, cwd);
            result(id, json!({"sessionId": cwd}))
        }
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

fn session_prompt(id: Value, params: Option<&Value>) -> Vec<Value> {
    let Some(user_text) = prompt_text(params) else {
        return vec![rpc_error(id, -32602, "prompt must be one non-empty text block")];
    };
    let Some(root) = workspace_root(params) else {
        return vec![rpc_error(
            id,
            -32602,
            "cwd or sessionId must be a non-empty string",
        )];
    };
    remember_session(root, root);
    let socks = std::env::var("EZ4A_SOCKS").ok();
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
        Err(err) => return vec![rpc_error(id, -32603, &err.to_string())],
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
        Ok((text, tail, updates)) => {
            let mut lines = updates;
            lines.push(result(
                id,
                json!({"stopReason": "end_turn", "text": text, "tail": tail}),
            ));
            lines
        }
        Err(err) if err == "cancelled" => vec![result(
            id,
            json!({"stopReason": "cancelled", "text": "", "tail": []}),
        )],
        Err(err) => {
            let message = if openrouter {
                redact_key(&err, key.as_deref().unwrap_or(""))
            } else {
                err
            };
            vec![rpc_error(id, -32603, &message)]
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
) -> Result<(String, Vec<String>, Vec<Value>), String> {
    let llm = FauxLlm::text(user_text).with_usage(Usage {
        input_tokens: 1,
        output_tokens: 1,
        provider: "faux".into(),
        model: "scripted".into(),
        cost_usd: 0.0,
    });
    memory_prompt(user_text, root, &llm, openrouter).await
}

async fn openrouter_prompt(
    user_text: &str,
    root: &str,
    socks: &str,
    key: &str,
    openrouter: bool,
) -> Result<(String, Vec<String>, Vec<Value>), String> {
    let from_env = std::env::var("EZ4A_MODEL").ok();
    let model = session_model(root).unwrap_or_else(|| {
        role_model(
            from_env.as_deref(),
            std::env::var("EZ4A_DEV_MODEL").ok().as_deref(),
            ez4a_harness::dev_agent().model,
        )
    });
    let llm = OpenRouterClient::new(model)
        .with_socks(socks)
        .with_key(key);
    memory_prompt(user_text, root, &llm, openrouter).await
}

/// `EZ4A_MODEL` wins over the role's own `EZ4A_<ROLE>_MODEL`; the role's
/// default is last. An empty value counts as unset. Advisor and gate
/// sessions resolve the same way against `EZ4A_ADVISOR_MODEL` and
/// `EZ4A_JEV_MODEL` when they arrive. `openrouter/auto` stays reachable by
/// setting `EZ4A_MODEL` to it.
fn role_model(global: Option<&str>, own: Option<&str>, role_default: &str) -> String {
    for value in [global, own] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            return value.to_string();
        }
    }
    role_default.to_string()
}

async fn memory_prompt(
    user_text: &str,
    root: &str,
    llm: &impl Llm,
    openrouter: bool,
) -> Result<(String, Vec<String>, Vec<Value>), String> {
    let session = open_journal(root).await?;
    let provider = if openrouter { "openrouter" } else { "faux" };
    let mut sessions = ProviderSessions::default();
    if let Err(err) = sessions.remember("ez4a", provider) {
        let _ = close_journal(&session).await;
        return Err(err);
    }
    let answered = turn_text(&session, llm, user_text, root).await;
    // Cadence, omp's reviewMode=turn: once a turn answers, the advisor branch
    // reviews its delta — but only when its own model is named. Best effort: a
    // failed review never fails the turn.
    if answered.is_ok() {
        if let Ok(model) = std::env::var("EZ4A_ADVISOR_MODEL") {
            if !model.trim().is_empty() {
                let advisor = ez4a_llm::OpenRouterClient::new(model.trim().to_string());
                if let Ok(workspace) = ez4a_tools::Workspace::new(root) {
                    if let Err(err) =
                        ez4a_tools::review(&session, &advisor, &workspace, ez4a_session::ROOT_CONVERSATION_ID)
                            .await
                    {
                        eprintln!("advisor review failed: {err}");
                    }
                }
            }
        }
    }
    let released = shutdown(&mut sessions, "ez4a");
    // A missed release outranks the turn and the journal close. The turn outranks close.
    let shutdown_err = if released == vec![provider.to_string()] {
        None
    } else {
        Some("provider session was not released".to_string())
    };
    let updates = journal_updates(&session, root).await;
    let closed = close_journal(&session).await;
    if let Some(err) = shutdown_err {
        return Err(err);
    }
    match (answered, closed) {
        (Ok((text, tail)), Ok(())) => Ok((text, tail, updates)),
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

/// Journal under `root`. Unset, empty, and `sqlite` open `ez4a-session.sqlite`.
/// `jsonl` opens `ez4a-session-jsonl`. `idb` opens the name `ez4a-session` and
/// does not use `root`. Any other `EZ4A_STORE` is `unknown session store` and
/// does not open a session.
async fn open_journal(root: &str) -> Result<Session, String> {
    let configured = if let Some(store) = session_store(root) {
        store
    } else {
        match std::env::var("EZ4A_STORE") {
            Ok(value) => value,
            Err(std::env::VarError::NotPresent) => String::new(),
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err("unknown session store".to_string());
            }
        }
    };
    let root = Path::new(root);
    let opened = match configured.as_str() {
        "" | "sqlite" => {
            let path = root.join("ez4a-session.sqlite");
            Session::open_kind(StoreKind::Sqlite, &path).await
        }
        "jsonl" => {
            let path = root.join("ez4a-session-jsonl");
            Session::open_kind(StoreKind::Jsonl, &path).await
        }
        "idb" => Session::open_kind(StoreKind::Idb, Path::new("ez4a-session")).await,
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
    ez4a_tools::resume_turns(session, &workspace, llm)
        .await
        .map_err(|err| err.to_string())?;
    let input = TurnInput {
        conversation_id: ROOT_CONVERSATION_ID,
        user_text: user_text.to_string(),
        tools: offered_tools(),
    };
    let outcome = run_until_answer_in(session, llm, &workspace, &input, root)
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

/// Filesystem and shell tools match `ez4a-tools` `examples/live.rs`.
/// `code` and `resize` are offered here as well.
/// The full schema table: worker tools plus the agent, mail, and door tools
/// a role may be granted. Names a role lists and the table lacks are dropped.
fn tools_for(names: &[String]) -> Vec<ToolSchema> {
    let mut table = offered_tools();
    table.extend(roster_tools());
    names
        .iter()
        .filter_map(|name| table.iter().find(|schema| &schema.name == name))
        .cloned()
        .collect()
}

/// Schemas the roster grants beyond the worker set: spawning, mail, the node
/// door.
fn roster_tools() -> Vec<ToolSchema> {
    vec![
        schema(
            "agent",
            "Spawn one worker agent with a self-contained prompt. It runs with the worker tool set (it cannot spawn agents) in its own workspace under subagents/. Its final answer comes back as the tool result.",
            json!({
                "type":"object",
                "properties":{"prompt":{"type":"string"}},
                "required":["prompt"]
            }),
        ),
        schema(
            "mail_send",
            "Send mail through hatchery's mail4agent. to is an account, a session address, or #room.",
            json!({
                "type":"object",
                "properties":{
                    "to":{"type":"string"},
                    "subject":{"type":"string"},
                    "body":{"type":"string"},
                    "reply_to":{"type":"string"}
                },
                "required":["to","subject","body"]
            }),
        ),
        schema(
            "mail_inbox",
            "Read your mail. wait_secs parks the read for new mail (max 60).",
            json!({
                "type":"object",
                "properties":{"wait_secs":{"type":"integer"}}
            }),
        ),
        schema(
            "mail_whoami",
            "Your own mail address and card.",
            json!({"type":"object","properties":{}}),
        ),
        schema(
            "g4a_tasks",
            "List open tasks assigned to this machine.",
            json!({"type":"object","properties":{}}),
        ),
        schema(
            "g4a_task_update",
            "Set the state of one of your tasks (in_progress or done).",
            json!({
                "type":"object",
                "properties":{
                    "id":{"type":"integer"},
                    "status":{"type":"string","enum":["in_progress","done"]}
                },
                "required":["id","status"]
            }),
        ),
    ]
}

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

/// One worker turn in its own root, for a spawned subagent process.
///
/// The workspace is `root` itself and the journal lives beside it. The model
/// is the dev role's, resolved through `EZ4A_MODEL` / `EZ4A_DEV_MODEL`. The
/// worker tool set is [`offered_tools`]: no `agent` — a worker does not spawn
/// workers. The answer's text is the return value.
pub async fn run_turn(root: &str, prompt: &str, role_id: &str) -> Result<String, String> {
    let session = open_journal(root).await?;
    let workspace = ez4a_tools::Workspace::new(root).map_err(|err| err.to_string())?;
    let catalog = {
        let dir = std::env::var("EZ4A_ROLES_DIR").unwrap_or_default();
        let dir = if dir.is_empty() {
            None
        } else {
            Some(std::path::PathBuf::from(dir))
        };
        ez4a_harness::load(dir.as_deref())
    };
    let role = ez4a_harness::find(&catalog, role_id)
        .ok_or_else(|| format!("unknown role {role_id}"))?;
    let model = role_model(
        std::env::var("EZ4A_MODEL").ok().as_deref(),
        Some(role.model.as_str()),
        ez4a_harness::dev_agent().model,
    );
    let llm = ez4a_llm::OpenRouterClient::new(model);
    ez4a_tools::resume_turns(&session, &workspace, &llm)
        .await
        .map_err(|err| err.to_string())?;
    let input = ez4a_harness::TurnInput {
        conversation_id: ez4a_session::ROOT_CONVERSATION_ID,
        user_text: role.prompt_for(prompt),
        tools: tools_for(&role.tools),
    };
    let outcome = ez4a_tools::run_until_answer_in(&session, &llm, &workspace, &input, root)
        .await
        .map_err(|err| err.to_string())?;
    let ez4a_harness::TurnKind::Answered { entry_id } = outcome.kind else {
        return Err("subagent turn did not answer".to_string());
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

struct SavedSession {
    cwd: String,
    model: Option<String>,
    store: Option<String>,
}

fn sessions() -> std::sync::MutexGuard<'static, BTreeMap<String, SavedSession>> {
    static SESSIONS: Mutex<BTreeMap<String, SavedSession>> = Mutex::new(BTreeMap::new());
    SESSIONS.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn remember_session(id: &str, cwd: &str) {
    sessions().entry(id.to_string()).or_insert(SavedSession {
        cwd: cwd.to_string(),
        model: None,
        store: None,
    });
}

fn session_model(id: &str) -> Option<String> {
    sessions().get(id).and_then(|saved| saved.model.clone())
}

fn session_store(id: &str) -> Option<String> {
    sessions().get(id).and_then(|saved| saved.store.clone())
}

fn session_load(id: Value, params: Option<&Value>) -> Vec<Value> {
    let Some(session_id) = params
        .and_then(|params| params.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
    else {
        return vec![rpc_error(id, -32602, "sessionId must be a non-empty string")];
    };
    let cwd = cwd_param(params).unwrap_or(session_id);
    remember_session(session_id, cwd);
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => return vec![rpc_error(id, -32603, &err.to_string())],
    };
    match runtime.block_on(replay(cwd, session_id)) {
        Ok(mut updates) => {
            updates.push(result(id, json!({"sessionId": session_id})));
            updates
        }
        Err(err) => vec![rpc_error(id, -32603, &err)],
    }
}

async fn replay(cwd: &str, session_id: &str) -> Result<Vec<Value>, String> {
    let session = open_journal(cwd).await?;
    let socks = std::env::var("EZ4A_SOCKS").ok();
    let key = resolve("openrouter").ok();
    let openrouter = prompt_backend(socks.as_deref(), key.as_deref()) == "openrouter";
    let finished = if openrouter {
        let model = session_model(session_id).unwrap_or_else(|| {
            role_model(
                std::env::var("EZ4A_MODEL").ok().as_deref(),
                std::env::var("EZ4A_DEV_MODEL").ok().as_deref(),
                ez4a_harness::dev_agent().model,
            )
        });
        let llm = OpenRouterClient::new(model)
            .with_socks(socks.unwrap_or_default())
            .with_key(key.unwrap_or_default());
        ez4a_tools::resume_turns(&session, &Workspace::new(cwd).map_err(|err| err.to_string())?, &llm)
            .await
            .map(|_| ())
    } else {
        let llm = FauxLlm::text("resumed");
        ez4a_tools::resume_turns(&session, &Workspace::new(cwd).map_err(|err| err.to_string())?, &llm)
            .await
            .map(|_| ())
    };
    let updates = journal_updates(&session, session_id).await;
    let closed = close_journal(&session).await;
    finished.map_err(|err| err.to_string())?;
    closed?;
    Ok(updates)
}

fn session_close(id: Value, params: Option<&Value>) -> Value {
    let Some(session_id) = params
        .and_then(|params| params.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
    else {
        return rpc_error(id, -32602, "sessionId must be a non-empty string");
    };
    request_cancel_session(session_id);
    sessions().remove(session_id);
    result(id, Value::Null)
}

fn session_list(id: Value) -> Value {
    let listed: Vec<Value> = sessions()
        .iter()
        .map(|(session_id, saved)| json!({"sessionId": session_id, "cwd": saved.cwd}))
        .collect();
    result(id, json!({"sessions": listed}))
}

fn session_set_model(id: Value, params: Option<&Value>) -> Value {
    let Some(session_id) = params
        .and_then(|params| params.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
    else {
        return rpc_error(id, -32602, "sessionId must be a non-empty string");
    };
    let Some(model) = params
        .and_then(|params| params.get("modelId"))
        .and_then(Value::as_str)
        .filter(|model| !model.is_empty())
    else {
        return rpc_error(id, -32602, "modelId must be a non-empty string");
    };
    remember_session(session_id, session_id);
    if let Some(saved) = sessions().get_mut(session_id) {
        saved.model = Some(model.to_string());
    }
    result(id, Value::Null)
}

fn session_set_config(id: Value, params: Option<&Value>) -> Value {
    let Some(session_id) = params
        .and_then(|params| params.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
    else {
        return rpc_error(id, -32602, "sessionId must be a non-empty string");
    };
    let Some(config_id) = params
        .and_then(|params| params.get("configId"))
        .and_then(Value::as_str)
    else {
        return rpc_error(id, -32602, "configId must be a string");
    };
    let Some(value) = params
        .and_then(|params| params.get("value"))
        .and_then(Value::as_str)
    else {
        return rpc_error(id, -32602, "value must be a string");
    };
    remember_session(session_id, session_id);
    let mut saved_sessions = sessions();
    let Some(saved) = saved_sessions.get_mut(session_id) else {
        return rpc_error(id, -32603, "session was not stored");
    };
    match config_id {
        "model" => saved.model = Some(value.to_string()),
        "store" if matches!(value, "" | "sqlite" | "jsonl" | "idb") => {
            saved.store = Some(value.to_string());
        }
        "store" => return rpc_error(id, -32602, "store must be sqlite, jsonl, or idb"),
        _ => return rpc_error(id, -32602, "unknown configId"),
    }
    result(id, Value::Null)
}

async fn journal_updates(session: &Session, session_id: &str) -> Vec<Value> {
    let Ok(view) = session.watch(ROOT_CONVERSATION_ID).await else {
        return Vec::new();
    };
    let entries: Vec<(String, String)> = view
        .borrow()
        .entries
        .iter()
        .map(|entry| (entry.kind.clone(), chunk_text(entry)))
        .collect();
    drop(view);
    entries
        .into_iter()
        .map(|(kind, text)| {
            let session_update = match kind.as_str() {
                "pi.user" => "user_message_chunk",
                "pi.toolResult" => "tool_call_update",
                _ => "agent_message_chunk",
            };
            json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": session_update,
                        "content": {"type": "text", "text": text}
                    }
                }
            })
        })
        .collect()
}

fn chunk_text(entry: &EntryRecord) -> String {
    entry_text(entry).unwrap_or_else(|_| {
        entry
            .model
            .as_ref()
            .map(|model| model.to_string())
            .unwrap_or_default()
    })
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
        assert_eq!(value["result"]["agentCapabilities"]["loadSession"], json!(true));
        assert_eq!(value["result"]["agentInfo"]["name"], json!("ez4a"));
        assert_eq!(value["result"]["authMethods"], json!([]));
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
    fn role_model_picks_env_over_role_default() {
        let dev = ez4a_harness::dev_agent().model;
        assert_eq!(role_model(None, None, dev), dev);
        assert_eq!(role_model(Some(""), None, dev), dev);
        assert_eq!(role_model(None, Some(""), dev), dev);
        assert_eq!(role_model(None, Some("vendor/dev"), dev), "vendor/dev");
        assert_eq!(
            role_model(Some("vendor/global"), Some("vendor/dev"), dev),
            "vendor/global"
        );
        assert_eq!(
            role_model(Some("openrouter/auto"), None, dev),
            "openrouter/auto"
        );
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
    fn session_controls_list_model_cancel_and_load() {
        let root = std::env::temp_dir().join(format!(
            "ez4a-acp-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&root);
        let root = root.to_string_lossy().replace('\\', "/");
        let listed = handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"session/new","params":{{"cwd":"{root}"}}}}"#
        ))
        .unwrap();
        assert!(listed.contains(&root));
        let list = handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"session/list"}"#).unwrap();
        assert!(list.contains(&root));
        let model = handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":3,"method":"session/set_model","params":{{"sessionId":"{root}","modelId":"cohere/north-mini-code:free"}}}}"#
        ))
        .unwrap();
        assert!(model.contains("\"result\":null") || model.contains("\"result\": null"));
        let config = handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":4,"method":"session/set_config_option","params":{{"sessionId":"{root}","configId":"store","value":"sqlite"}}}}"#
        ))
        .unwrap();
        assert!(!config.contains("\"error\""));
        handle_exchange(&format!(
            r#"{{"jsonrpc":"2.0","method":"session/cancel","params":{{"sessionId":"{root}"}}}}"#
        ))
        .unwrap();
        let cancelled = handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"session/prompt","params":{{"sessionId":"{root}","prompt":[{{"type":"text","text":"hi"}}]}}}}"#
        ))
        .unwrap();
        assert!(cancelled.contains("cancelled"), "{cancelled}");
        let loaded = handle_exchange(&format!(
            r#"{{"jsonrpc":"2.0","id":6,"method":"session/load","params":{{"sessionId":"{root}","cwd":"{root}"}}}}"#
        ))
        .unwrap();
        assert!(loaded.iter().any(|line| line.contains("user_message_chunk") || line.contains("agent_message_chunk") || line.contains("sessionId")));
        let closed = handle_line(&format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"session/close","params":{{"sessionId":"{root}"}}}}"#
        ))
        .unwrap();
        assert!(closed.contains("\"result\":null") || closed.contains("\"result\": null"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unknown_method_is_not_found() {
        let line = handle_line(r#"{"jsonrpc":"2.0","id":4,"method":"session/nope"}"#)
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
    fn run_takes_root_and_prompt_in_any_order() {
        assert_eq!(
            parse_args(["run", "--root", "E:/dir", "--prompt", "do it"]),
            Ok(Command::Run {
                root: "E:/dir".into(),
                prompt: "do it".into(),
                role: "dev".into()
            })
        );
        assert_eq!(
            parse_args(["run", "--prompt", "do it", "--role", "reviewer", "--root", "E:/dir"]),
            Ok(Command::Run {
                root: "E:/dir".into(),
                prompt: "do it".into(),
                role: "reviewer".into()
            })
        );
        assert!(parse_args(["run"]).is_err());
        assert!(parse_args(["run", "--root", "E:/dir"]).is_err());
        assert!(parse_args(["run", "--prompt", "x"]).is_err());
        assert!(parse_args(["run", "--else", "x"]).is_err());
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
        assert!(USAGE.contains("ez4a acp"));
    }
}
