//! ez4a as an MCP server: the roster's handles for provider agents.
//!
//! The host already speaks JSON-RPC on stdio (`ez4a acp`); this is the MCP
//! door — `initialize`, `tools/list`, `tools/call` — over the same stdio.
//! ez4a registers itself nowhere: hatchery wires the fleet centrally. The
//! handles are laid now, as data, so the central registration only points
//! at this door.
//!
//! The tools are the LLM floor of the lineup, served to OTHER agents:
//! `ez4a_review` (judge work against a request), `ez4a_classify` (a fast
//! one-word verdict), `ez4a_advise` (ask the advisor). No tool writes,
//! spawns, or touches a session: read-only judgment, by design.

use serde_json::{json, Value};

/// One handle in the roster: name, description, JSON schema, and how the
/// call maps to an advisor question.
struct Handle {
    name: &'static str,
    description: &'static str,
    properties: Value,
    required: &'static [&'static str],
    /// Build the advisor question from the call's arguments.
    question: fn(&Value) -> Result<String, String>,
}

fn review_question(args: &Value) -> Result<String, String> {
    let request = args
        .get("request")
        .and_then(Value::as_str)
        .ok_or_else(|| "request is required".to_owned())?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "content is required".to_owned())?;
    if request.trim().is_empty() || content.trim().is_empty() {
        return Err("request and content must be non-empty".into());
    }
    Ok(format!(
        "The request was: {request}\n\nThe work to review:\n{content}"
    ))
}

fn classify_question(args: &Value) -> Result<String, String> {
    let question = args
        .get("question")
        .and_then(Value::as_str)
        .ok_or_else(|| "question is required".to_owned())?;
    let options = args
        .get("options")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    if question.trim().is_empty() {
        return Err("question must be non-empty".into());
    }
    if options.is_empty() {
        Ok(format!("Answer with one word. {question}"))
    } else {
        Ok(format!("Answer with exactly one of: {options}. {question}"))
    }
}

fn advise_question(args: &Value) -> Result<String, String> {
    let question = args
        .get("question")
        .and_then(Value::as_str)
        .ok_or_else(|| "question is required".to_owned())?;
    if question.trim().is_empty() {
        return Err("question must be non-empty".into());
    }
    Ok(question.to_owned())
}

fn handles() -> Vec<Handle> {
    vec![
        Handle {
            name: "ez4a_review",
            description: "Judge work against its request: verdict plus the specific \
problems found. Read-only; nothing is changed for you.",
            properties: json!({
                "request": { "type": "string" },
                "content": { "type": "string" }
            }),
            required: &["request", "content"],
            question: review_question,
        },
        Handle {
            name: "ez4a_classify",
            description: "A fast one-word verdict on a closed question (continue/stop, \
yes/no, or your own option set).",
            properties: json!({
                "question": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" } }
            }),
            required: &["question"],
            question: classify_question,
        },
        Handle {
            name: "ez4a_advise",
            description: "Ask the read-only advisor one open question. It does not \
write, run, or spawn.",
            properties: json!({
                "question": { "type": "string" }
            }),
            required: &["question"],
            question: advise_question,
        },
    ]
}

/// The `tools/list` payload.
pub fn tools_list() -> Value {
    json!({
        "tools": handles()
            .into_iter()
            .map(|handle| {
                json!({
                    "name": handle.name,
                    "description": handle.description,
                    "inputSchema": {
                        "type": "object",
                        "properties": handle.properties,
                        "required": handle.required,
                    }
                })
            })
            .collect::<Vec<_>>()
    })
}

/// The `initialize` result.
pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "ez4a", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Handle one JSON-RPC request line without a model client:
/// `initialize`, `tools/list`, errors, notifications.
pub fn handle_request(line: &str) -> Result<Option<String>, String> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|err| format!("parse error: {err}"))?;
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "not a request: no method".to_owned())?
        .to_owned();
    let id = value.get("id").cloned();
    let Some(id) = id else {
        // A notification: we have nothing to say about any of them yet.
        return Ok(None);
    };
    let result = match method.as_str() {
        "initialize" => initialize_result(),
        "tools/list" => tools_list(),
        "tools/call" => {
            return Ok(Some(error_reply(
                id,
                -32602,
                "tools/call needs the model client; use serve_mcp",
            )))
        }
        other => {
            return Ok(Some(error_reply(
                id,
                -32601,
                &format!("unknown method: {other}"),
            )))
        }
    };
    let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    Ok(Some(serde_json::to_string(&reply).map_err(|e| e.to_string())?))
}

/// The `tools/call` core: map the named handle to an advisor question and
/// answer it. The model client is injected so tests stay offline; the stdio
/// loop passes the real one.
pub async fn tools_call_with<L: ez4a_llm::Llm>(
    params: &Value,
    llm: &L,
) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call needs a name".to_owned())?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    let handle = handles()
        .into_iter()
        .find(|handle| handle.name == name)
        .ok_or_else(|| format!("unknown tool: {name}"))?;
    let question = (handle.question)(&arguments)?;
    let text = ez4a_harness::advise(llm, &question)
        .await
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": false
    }))
}

/// The full dispatch with a model client — what the stdio loop serves.
/// A refused or failed call is a tool result (`isError: true`), not a
/// protocol error: the caller must see the reason.
pub async fn handle_request_with<L: ez4a_llm::Llm>(
    line: &str,
    llm: &L,
) -> Result<Option<String>, String> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|err| format!("parse error: {err}"))?;
    let method = value
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| "not a request: no method".to_owned())?
        .to_owned();
    let id = value.get("id").cloned();
    let Some(id) = id else {
        return Ok(None);
    };
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let result = match method.as_str() {
        "initialize" => initialize_result(),
        "tools/list" => tools_list(),
        "tools/call" => match tools_call_with(&params, llm).await {
            Ok(result) => result,
            Err(message) => json!({
                "content": [ { "type": "text", "text": message } ],
                "isError": true
            }),
        },
        other => {
            return Ok(Some(error_reply(
                id,
                -32601,
                &format!("unknown method: {other}"),
            )))
        }
    };
    let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    Ok(Some(serde_json::to_string(&reply).map_err(|e| e.to_string())?))
}

fn error_reply(id: Value, code: i64, message: &str) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    }))
    .unwrap_or_else(|_| "{\"jsonrpc\":\"2.0\",\"id\":null}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ez4a_llm::FauxLlm;

    fn request(method: &str, params: Value) -> String {
        serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        }))
        .unwrap()
    }

    #[test]
    fn initialize_and_list_speak_mcp() {
        let reply = handle_request(&request("initialize", json!({}))).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["result"]["serverInfo"]["name"], "ez4a");
        assert!(parsed["result"]["capabilities"]["tools"].is_object());

        let reply = handle_request(&request("tools/list", json!({}))).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        let names: Vec<&str> = parsed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["ez4a_review", "ez4a_classify", "ez4a_advise"]);
    }

    #[test]
    fn unknown_methods_answer_with_code_minus_32601() {
        let reply = handle_request(&request("resources/list", json!({})))
            .unwrap()
            .unwrap();
        assert!(reply.contains("-32601"), "{reply}");
    }

    #[test]
    fn notifications_are_silent_and_garbage_is_refused() {
        assert_eq!(
            handle_request("{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}")
                .unwrap(),
            None
        );
        assert!(handle_request("not json").is_err());
        assert!(handle_request("{\"jsonrpc\":\"2.0\"}").is_err());
    }

    #[test]
    fn a_call_maps_to_the_advisor_and_back_as_mcp_content() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let llm = FauxLlm::text("verdict: wrong path");
        let result = runtime
            .block_on(tools_call_with(
                &json!({ "name": "ez4a_review", "arguments": {
                    "request": "write hello", "content": "wrote goodbye"
                }}),
                &llm,
            ))
            .unwrap();
        assert_eq!(result["content"][0]["text"], "verdict: wrong path");
        assert_eq!(result["isError"], false);

        let err = runtime
            .block_on(tools_call_with(
                &json!({ "name": "ez4a_review", "arguments": { "request": "", "content": "x" } }),
                &llm,
            ))
            .unwrap_err();
        assert!(err.contains("non-empty"), "{err}");

        let err = runtime
            .block_on(tools_call_with(
                &json!({ "name": "ez4a_nothing", "arguments": {} }),
                &llm,
            ))
            .unwrap_err();
        assert!(err.contains("unknown tool"), "{err}");
    }

    #[test]
    fn a_refused_call_is_a_tool_result_not_a_protocol_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let llm = FauxLlm::failing("no model today");
        let request = serde_json::to_string(&json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": "ez4a_classify", "arguments": { "question": "go on?" } }
        }))
        .unwrap();
        let reply = runtime
            .block_on(handle_request_with(&request, &llm))
            .unwrap()
            .unwrap();
        let parsed: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(parsed["id"], 7);
        assert_eq!(parsed["result"]["isError"], true);
        assert!(parsed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("no model today"));
    }

    #[test]
    fn the_plain_dispatcher_names_what_it_cannot_do() {
        let reply = handle_request(&request("tools/call", json!({ "name": "x" })))
            .unwrap()
            .unwrap();
        assert!(reply.contains("serve_mcp"), "{reply}");
    }
}
