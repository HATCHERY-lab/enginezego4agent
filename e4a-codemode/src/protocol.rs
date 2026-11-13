// Portions derived from pi-durable 1.5.2 (packages/durable/src/codemode/codemode.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Records a code-mode run stores, and the JSON messages the host and the
//! sandbox exchange. Parsing a message does not start a VM.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Bump when stored results would be replayed by different code.
pub const CODEMODE_MODULE_VERSION: u32 = 2;

pub const CODEMODE_TOOL_NAME: &str = "codemode";

/// Globals answered from the run's start data.
pub const DISCOVERY_GLOBALS: [&str; 3] = ["searchTools", "describeTool", "describeNamespace"];

pub const DEFAULT_SEARCH_LIMIT: usize = 8;

/// `cut()` width for argument previews in a call row.
pub const ARGS_PREVIEW_CHARS: usize = 200;

/// `cut()` width for error previews in a call row.
pub const ERROR_PREVIEW_CHARS: usize = 500;

/// The tool's own output limits. The harness cuts anything past them.
pub const DEFAULT_OUTPUT_LIMITS: OutputLimits = OutputLimits {
    max_bytes: 4 * 1024 * 1024,
    max_lines: 100_000,
};

const OPTIONS_PREFIX: &str = "// @options:";
const MAX_TIMEOUT_MS: u64 = 2_147_483_647;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const SUPPORTED_FIELDS: &str = "`max_output_tokens` and `timeout_ms`";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputLimits {
    pub max_bytes: usize,
    pub max_lines: usize,
}

/// `script`, `timeout`, `aborted`, or `sandbox`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorKind {
    Script,
    Timeout,
    Aborted,
    Sandbox,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Script => "script",
            Self::Timeout => "timeout",
            Self::Aborted => "aborted",
            Self::Sandbox => "sandbox",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodemodeError {
    pub kind: ErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallTarget {
    Tool,
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallStatus {
    Running,
    Ok,
    Error,
    Cancelled,
}

/// One item of the script's own output. `console` is set for `console.*`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ScriptOutput {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        console: Option<bool>,
    },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct During {
    pub seq: i64,
    pub issued: i64,
}

/// Worker to host. Tool arguments cross as a JSON string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkerMessage {
    #[serde(rename = "call")]
    Call {
        id: i64,
        target: CallTarget,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<String>,
    },
    #[serde(rename = "output")]
    Output { item: ScriptOutput },
    #[serde(rename = "done")]
    Done {
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        writes: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        during: Option<During>,
    },
    #[serde(rename = "crash")]
    Crash { message: String },
    #[serde(rename = "started")]
    Started { fingerprint: String },
    #[serde(rename = "idle")]
    Idle { seq: i64, issued: i64 },
}

/// Host to worker: the result of one call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum HostMessage {
    #[serde(rename = "result")]
    Result {
        id: i64,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        now: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        properties: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredTool {
    pub name: String,
    pub js_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredGlobal {
    pub name: String,
    pub spread: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamespaceTools {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub tools: Vec<String>,
}

/// What a call starts with. A rerun starts from exactly this.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStart {
    pub version: u32,
    pub started_at: i64,
    pub seed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<i64>,
    pub tools: Vec<DeclaredTool>,
    pub namespaces: Vec<NamespaceTools>,
    pub globals: Vec<DeclaredGlobal>,
    pub max_output_tokens: u64,
    pub store: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<RunStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JournalGlobal {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Value>,
    pub outcome: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    pub id: i64,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_after: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global: Option<JournalGlobal>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalState {
    pub entries: Vec<JournalEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_issued: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedCall {
    pub id: i64,
    pub name: String,
    pub args: String,
    pub status: CallStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDetails {
    pub calls: Vec<NestedCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_output_path: Option<String>,
}

/// A call's answer. `ok` selects which fields matter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallOutcome {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SourceOptions {
    pub max_output_tokens: Option<u64>,
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSource {
    /// Script text. An options line leaves the separating newline in place.
    pub code: String,
    pub options: SourceOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SourceError(pub String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ProtocolError(pub String);

pub fn parse_worker_message(blob: &str) -> Result<WorkerMessage, ProtocolError> {
    serde_json::from_str(blob).map_err(|err| ProtocolError(format!("worker message: {err}")))
}

pub fn parse_host_message(blob: &str) -> Result<HostMessage, ProtocolError> {
    serde_json::from_str(blob).map_err(|err| ProtocolError(format!("host message: {err}")))
}

/// Split an optional first-line `// @options: {...}` from the script.
pub fn parse_source(input: &str) -> Result<ParsedSource, SourceError> {
    if input.trim().is_empty() {
        return Err(SourceError(
            "Expected JavaScript source text (non-empty). Provide JS only, optionally with a first line `// @options: {\"max_output_tokens\": 1000}`.".to_owned(),
        ));
    }
    let lines: Vec<&str> = input.split('\n').collect();
    for (index, line) in lines.iter().enumerate().skip(1) {
        if line.trim_start().starts_with(OPTIONS_PREFIX) {
            return Err(SourceError(format!(
                "The {OPTIONS_PREFIX} line must be the first line of the input; found one on line {}",
                index + 1
            )));
        }
    }
    let first = lines.first().copied().unwrap_or("");
    let first = first.strip_suffix('\r').unwrap_or(first);
    let trimmed = first.trim_start();
    if !trimmed.starts_with(OPTIONS_PREFIX) {
        return Ok(ParsedSource {
            code: input.to_owned(),
            options: SourceOptions::default(),
        });
    }
    let code = match input.find('\n') {
        None => String::new(),
        Some(index) => input[index..].to_owned(),
    };
    if code.trim().is_empty() {
        return Err(SourceError(
            "The @options line must be followed by JavaScript source on subsequent lines".to_owned(),
        ));
    }
    let directive = trimmed[OPTIONS_PREFIX.len()..].trim();
    Ok(ParsedSource {
        code,
        options: parse_options(directive)?,
    })
}

/// Preview used for call rows. Longer than `max` UTF-16 units, the text keeps
/// `max - 3` units and an ellipsis. `max` below 3 keeps no units.
pub fn preview(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() <= max {
        return text.to_owned();
    }
    let keep = max.saturating_sub(3);
    let head = String::from_utf16_lossy(&units[..keep]);
    format!("{head}...")
}

fn parse_options(directive: &str) -> Result<SourceOptions, SourceError> {
    if directive.is_empty() {
        return Err(SourceError(options_object()));
    }
    let value: Value = serde_json::from_str(directive).map_err(|err| {
        SourceError(format!(
            "@options must be valid JSON with supported fields {SUPPORTED_FIELDS}: {err}"
        ))
    })?;
    let Some(map) = value.as_object() else {
        return Err(SourceError(options_object()));
    };
    for key in map.keys() {
        if key != "max_output_tokens" && key != "timeout_ms" {
            return Err(SourceError(format!(
                "@options only supports {SUPPORTED_FIELDS}; got `{key}`"
            )));
        }
    }
    let mut options = SourceOptions::default();
    if let Some(value) = map.get("max_output_tokens") {
        options.max_output_tokens = Some(expect_safe(
            value,
            "@options field `max_output_tokens` must be a non-negative safe integer",
            0,
        )?);
    }
    if let Some(value) = map.get("timeout_ms") {
        options.timeout_ms = Some(expect_safe(
            value,
            &format!(
                "@options field `timeout_ms` must be a positive integer up to {MAX_TIMEOUT_MS}"
            ),
            1,
        )?);
    }
    if let Some(timeout) = options.timeout_ms {
        if timeout > MAX_TIMEOUT_MS {
            return Err(SourceError(format!(
                "@options field `timeout_ms` must be a positive integer up to {MAX_TIMEOUT_MS}"
            )));
        }
    }
    Ok(options)
}

fn expect_safe(value: &Value, message: &str, min: u64) -> Result<u64, SourceError> {
    let Some(number) = value.as_u64() else {
        return Err(SourceError(message.to_owned()));
    };
    if number < min || number > MAX_SAFE_INTEGER {
        return Err(SourceError(message.to_owned()));
    }
    Ok(number)
}

fn options_object() -> String {
    format!("@options must be a JSON object with supported fields {SUPPORTED_FIELDS}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source_names_the_reason() {
        let err = parse_source("  \n").expect_err("blank");
        assert!(err.to_string().starts_with("Expected JavaScript source text"));
    }

    #[test]
    fn options_line_keeps_the_newline_on_the_code() {
        let parsed = parse_source("// @options: {\"timeout_ms\": 1000}\nreturn 1").expect("options");
        assert_eq!(parsed.code, "\nreturn 1");
        assert_eq!(parsed.options.timeout_ms, Some(1000));
        assert!(parsed.options.max_output_tokens.is_none());
    }

    #[test]
    fn worker_call_round_trip() {
        let message = WorkerMessage::Call {
            id: 1,
            target: CallTarget::Tool,
            name: "read".to_owned(),
            args: Some("{}".to_owned()),
        };
        let text = serde_json::to_string(&message).expect("json");
        let parsed = parse_worker_message(&text).expect("worker message");
        assert_eq!(parsed, message);
    }

    #[test]
    fn preview_cuts_on_utf16_length() {
        assert_eq!(preview("abcdef", 5), "ab...");
        assert_eq!(preview("ab", ARGS_PREVIEW_CHARS), "ab");
    }
}
