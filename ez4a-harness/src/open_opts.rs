// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/harness.ts and packages/durable/src/harness/agent.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/harness and harness/agent, MIT, see LICENSES/pi-MIT.txt.

//! Open-options and agent settings read from JSON.
//!
//! Upstream `Harness.open` takes storage, a registry, and callbacks. This module
//! only normalizes the serializable settings and agent fields. It does not open
//! a database or a network.

use serde_json::{Map, Value};

/// Default `settings.restartLimit` (`agent.js` `DEFAULT_RESTART_LIMIT`).
pub const DEFAULT_RESTART_LIMIT: u64 = 5;

/// `ModelRef`: provider plus model id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub model_id: String,
}

/// Effort accepted on open. Upstream thinking levels also include `minimal` and `xhigh`;
/// those belong on [`ThinkingLevel`], not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effort {
    Off,
    Low,
    Medium,
    High,
    Max,
}

impl Effort {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "off" => Ok(Self::Off),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "max" => Ok(Self::Max),
            _ => Err(EFFORT_ERROR.into()),
        }
    }
}

/// `ModelThinkingLevel`, including levels effort does not accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl ThinkingLevel {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            _ => Err(THINKING_ERROR.into()),
        }
    }
}

/// Agent fields that can be omitted. `keep_recent_tokens` is `None` when the
/// JSON omits it or sends `0` (unset, not "keep nothing").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentSettings {
    pub model: Option<ModelRef>,
    pub effort: Option<Effort>,
    pub thinking: Option<ThinkingLevel>,
    pub tools: Option<Vec<String>>,
    pub cwd: Option<String>,
    pub keep_recent_tokens: Option<u64>,
}

/// Normalized `Harness.open` options plus the agent document fields.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenOptions {
    pub agent: AgentSettings,
    /// Resolved restart limit. Omitted, non-numeric, or below `1` becomes
    /// [`DEFAULT_RESTART_LIMIT`], matching `restartLimit()` in `agent.js`.
    pub restart_limit: u64,
    /// Top-level keys this normalizer does not interpret.
    pub extra: Map<String, Value>,
}

const EFFORT_ERROR: &str = "effort must be one of off, low, medium, high, max";
const THINKING_ERROR: &str =
    "thinking must be one of off, minimal, low, medium, high, xhigh, max";

/// Read open options from a JSON object.
///
/// Missing object is an error. An empty model string is an error. `effort`
/// must be `off`, `low`, `medium`, `high`, `max`, or absent. `thinking` and
/// `thinkingLevel` are the upstream thinking level. Unknown keys are returned
/// in [`OpenOptions::extra`].
pub fn normalize_open(raw: &Value) -> Result<OpenOptions, String> {
    let Some(object) = raw.as_object() else {
        return Err("open options must be a JSON object".into());
    };
    let mut extra = object.clone();
    let mut agent = AgentSettings::default();
    let mut restart_limit = DEFAULT_RESTART_LIMIT;

    if let Some(mut settings) = take_object(&mut extra, "settings")? {
        apply_harness_keys(&mut settings, &mut restart_limit, &mut agent.keep_recent_tokens)?;
        if !settings.is_empty() {
            extra.insert("settings".to_owned(), Value::Object(settings));
        }
    }
    if let Some(mut stored) = take_object(&mut extra, "agent")? {
        apply_agent_keys(&mut stored, &mut agent)?;
        if !stored.is_empty() {
            extra.insert("agent".to_owned(), Value::Object(stored));
        }
    }
    apply_agent_keys(&mut extra, &mut agent)?;
    if let Some(value) = extra.remove("restartLimit") {
        restart_limit = parse_restart_limit(&value);
    }

    Ok(OpenOptions {
        agent,
        restart_limit,
        extra,
    })
}

fn take_object(
    map: &mut Map<String, Value>,
    key: &str,
) -> Result<Option<Map<String, Value>>, String> {
    match map.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(object)) => Ok(Some(object)),
        Some(_) => Err(format!("{key} must be an object")),
    }
}

fn apply_harness_keys(
    map: &mut Map<String, Value>,
    restart_limit: &mut u64,
    keep_recent_tokens: &mut Option<u64>,
) -> Result<(), String> {
    if let Some(value) = map.remove("restartLimit") {
        *restart_limit = parse_restart_limit(&value);
    }
    if let Some(value) = map.remove("compaction") {
        apply_compaction(&value, map, keep_recent_tokens)?;
    }
    Ok(())
}

fn apply_agent_keys(map: &mut Map<String, Value>, agent: &mut AgentSettings) -> Result<(), String> {
    if let Some(value) = map.remove("model") {
        agent.model = parse_model(&value)?;
    }
    if let Some(value) = map.remove("effort") {
        agent.effort = parse_effort(&value)?;
    }
    apply_thinking(map, agent)?;
    if let Some(value) = map.remove("tools") {
        agent.tools = parse_tools(&value)?;
    }
    if let Some(value) = map.remove("cwd") {
        agent.cwd = parse_cwd(&value)?;
    }
    if let Some(value) = map.remove("compaction") {
        apply_compaction(&value, map, &mut agent.keep_recent_tokens)?;
    }
    Ok(())
}

fn apply_thinking(map: &mut Map<String, Value>, agent: &mut AgentSettings) -> Result<(), String> {
    let thinking = map.remove("thinking");
    let thinking_level = map.remove("thinkingLevel");
    match (thinking, thinking_level) {
        (None, None) => Ok(()),
        (Some(value), None) | (None, Some(value)) => {
            agent.thinking = parse_thinking(&value)?;
            Ok(())
        }
        (Some(left), Some(right)) => {
            let left = parse_thinking(&left)?;
            let right = parse_thinking(&right)?;
            if left != right {
                return Err("thinking and thinkingLevel disagree".into());
            }
            agent.thinking = left;
            Ok(())
        }
    }
}

/// `0` does not assign: it means unset, so an earlier positive value stands.
fn apply_compaction(
    value: &Value,
    map: &mut Map<String, Value>,
    keep_recent_tokens: &mut Option<u64>,
) -> Result<(), String> {
    if value.is_null() {
        return Ok(());
    }
    let Some(object) = value.as_object() else {
        return Err("compaction must be an object".into());
    };
    let mut rest = object.clone();
    if let Some(raw) = rest.remove("keepRecentTokens") {
        if let Some(tokens) = parse_keep_recent(&raw)? {
            *keep_recent_tokens = Some(tokens);
        }
    }
    if !rest.is_empty() {
        map.insert("compaction".to_owned(), Value::Object(rest));
    }
    Ok(())
}

fn parse_restart_limit(value: &Value) -> u64 {
    let Some(number) = value.as_number() else {
        return DEFAULT_RESTART_LIMIT;
    };
    if let Some(limit) = number.as_u64() {
        return if limit >= 1 {
            limit
        } else {
            DEFAULT_RESTART_LIMIT
        };
    }
    if let Some(limit) = number.as_i64() {
        return if limit >= 1 {
            limit as u64
        } else {
            DEFAULT_RESTART_LIMIT
        };
    }
    let Some(limit) = number.as_f64() else {
        return DEFAULT_RESTART_LIMIT;
    };
    if !limit.is_finite() || limit < 1.0 {
        return DEFAULT_RESTART_LIMIT;
    }
    let floored = limit.floor();
    if floored >= 1.0 && floored <= u64::MAX as f64 {
        floored as u64
    } else {
        DEFAULT_RESTART_LIMIT
    }
}

fn parse_keep_recent(value: &Value) -> Result<Option<u64>, String> {
    match value.as_u64() {
        Some(0) => Ok(None),
        Some(tokens) => Ok(Some(tokens)),
        None => Err(format!(
            "compaction.keepRecentTokens must be an integer 0 or more, got {value}"
        )),
    }
}

fn parse_model(value: &Value) -> Result<Option<ModelRef>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(raw) => {
            if raw.is_empty() {
                return Err("model must not be empty".into());
            }
            model_from_string(raw).map(Some)
        }
        Value::Object(object) => {
            let provider = required_model_string(object, "provider")?;
            let model_id = required_model_string(object, "modelId")?;
            Ok(Some(ModelRef { provider, model_id }))
        }
        _ => Err(
            "model must be a provider/modelId string or an object with provider and modelId"
                .into(),
        ),
    }
}

fn model_from_string(raw: &str) -> Result<ModelRef, String> {
    match raw.split_once('/') {
        Some((provider, model_id)) if !provider.is_empty() && !model_id.is_empty() => {
            Ok(ModelRef {
                provider: provider.to_owned(),
                model_id: model_id.to_owned(),
            })
        }
        Some(_) => Err("model must not be empty".into()),
        None => Err("model must be provider/modelId".into()),
    }
}

fn required_model_string(object: &Map<String, Value>, key: &str) -> Result<String, String> {
    match object.get(key) {
        Some(Value::String(raw)) if !raw.is_empty() => Ok(raw.clone()),
        Some(Value::String(_)) => Err("model must not be empty".into()),
        _ => Err(format!("model.{key} must be a non-empty string")),
    }
}

fn parse_effort(value: &Value) -> Result<Option<Effort>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(raw) => Effort::parse(raw).map(Some),
        _ => Err(EFFORT_ERROR.into()),
    }
}

fn parse_thinking(value: &Value) -> Result<Option<ThinkingLevel>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(raw) => ThinkingLevel::parse(raw).map(Some),
        _ => Err(THINKING_ERROR.into()),
    }
}

fn parse_tools(value: &Value) -> Result<Option<Vec<String>>, String> {
    match value {
        Value::Null => Ok(None),
        Value::Array(items) => {
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                names.push(tool_name(item)?);
            }
            Ok(Some(names))
        }
        _ => Err("tools must be a list of names".into()),
    }
}

fn tool_name(value: &Value) -> Result<String, String> {
    match value {
        Value::String(name) if !name.is_empty() => Ok(name.clone()),
        Value::String(_) => Err("tool name must not be empty".into()),
        Value::Object(object) => match object.get("name") {
            Some(Value::String(name)) if !name.is_empty() => Ok(name.clone()),
            Some(Value::String(_)) => Err("tool name must not be empty".into()),
            _ => Err("tools must be a list of names".into()),
        },
        _ => Err("tools must be a list of names".into()),
    }
}

fn parse_cwd(value: &Value) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(cwd) => Ok(Some(cwd.clone())),
        _ => Err("cwd must be a string".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn open(raw: Value) -> OpenOptions {
        normalize_open(&raw).unwrap_or_else(|error| panic!("normalize failed: {error}"))
    }

    #[test]
    fn rejects_a_non_object() {
        for raw in [json!(null), json!([]), json!(1), json!("open")] {
            let error = normalize_open(&raw).unwrap_err();
            assert_eq!(error, "open options must be a JSON object");
        }
    }

    #[test]
    fn empty_object_uses_defaults_and_keeps_no_extra() {
        let options = open(json!({}));
        assert_eq!(options.agent, AgentSettings::default());
        assert_eq!(options.restart_limit, DEFAULT_RESTART_LIMIT);
        assert!(options.extra.is_empty());
    }

    #[test]
    fn reads_agent_fields_and_parks_unknown_keys() {
        let options = open(json!({
            "model": {"provider": "openrouter", "modelId": "thinkingmachines/inkling:free"},
            "effort": "low",
            "thinking": "high",
            "tools": ["read", {"name": "powershell"}],
            "cwd": "E:/work",
            "compaction": {"keepRecentTokens": 20000, "enabled": true},
            "restartLimit": 3,
            "readOnly": false
        }));
        assert_eq!(
            options.agent.model,
            Some(ModelRef {
                provider: "openrouter".into(),
                model_id: "thinkingmachines/inkling:free".into(),
            })
        );
        assert_eq!(options.agent.effort, Some(Effort::Low));
        assert_eq!(options.agent.thinking, Some(ThinkingLevel::High));
        assert_eq!(
            options.agent.tools,
            Some(vec!["read".into(), "powershell".into()])
        );
        assert_eq!(options.agent.cwd.as_deref(), Some("E:/work"));
        assert_eq!(options.agent.keep_recent_tokens, Some(20_000));
        assert_eq!(options.restart_limit, 3);
        assert_eq!(
            options.extra,
            json!({"readOnly": false, "compaction": {"enabled": true}})
                .as_object()
                .unwrap()
                .clone()
        );
    }

    #[test]
    fn model_string_splits_on_the_first_slash_and_empty_is_an_error() {
        let model = open(json!({"model": "openrouter/thinkingmachines/inkling:free"}))
            .agent
            .model
            .unwrap();
        assert_eq!(model.provider, "openrouter");
        assert_eq!(model.model_id, "thinkingmachines/inkling:free");
        assert_eq!(
            normalize_open(&json!({"model": ""})).unwrap_err(),
            "model must not be empty"
        );
        assert_eq!(
            normalize_open(&json!({"model": {"provider": "openrouter", "modelId": ""}}))
                .unwrap_err(),
            "model must not be empty"
        );
        assert_eq!(
            normalize_open(&json!({"model": "/"})).unwrap_err(),
            "model must not be empty"
        );
    }

    #[test]
    fn effort_accepts_only_the_subset_and_may_be_absent() {
        for (raw, effort) in [
            ("off", Effort::Off),
            ("low", Effort::Low),
            ("medium", Effort::Medium),
            ("high", Effort::High),
            ("max", Effort::Max),
        ] {
            let options = open(json!({"effort": raw}));
            assert_eq!(options.agent.effort, Some(effort));
        }
        let absent = open(json!({"cwd": "/tmp"}));
        assert_eq!(absent.agent.effort, None);
        assert_eq!(
            normalize_open(&json!({"effort": "minimal"})).unwrap_err(),
            EFFORT_ERROR
        );
        assert_eq!(
            normalize_open(&json!({"effort": "xhigh"})).unwrap_err(),
            EFFORT_ERROR
        );
        assert_eq!(normalize_open(&json!({"effort": 1})).unwrap_err(), EFFORT_ERROR);
    }

    #[test]
    fn thinking_level_accepts_the_upstream_set() {
        let options = open(json!({"thinkingLevel": "xhigh"}));
        assert_eq!(options.agent.thinking, Some(ThinkingLevel::XHigh));
        let minimal = open(json!({"thinking": "minimal", "thinkingLevel": "minimal"}));
        assert_eq!(minimal.agent.thinking, Some(ThinkingLevel::Minimal));
        assert_eq!(
            normalize_open(&json!({"thinking": "low", "thinkingLevel": "high"})).unwrap_err(),
            "thinking and thinkingLevel disagree"
        );
        assert_eq!(
            normalize_open(&json!({"thinking": "extreme"})).unwrap_err(),
            THINKING_ERROR
        );
    }

    #[test]
    fn keep_recent_zero_is_unset_and_does_not_clear_a_harness_value() {
        let zero = open(json!({"compaction": {"keepRecentTokens": 0}}));
        assert_eq!(zero.agent.keep_recent_tokens, None);
        let omitted = open(json!({"compaction": {"enabled": false}}));
        assert_eq!(omitted.agent.keep_recent_tokens, None);
        let layered = open(json!({
            "settings": {"compaction": {"keepRecentTokens": 100, "reserveTokens": 8}},
            "agent": {"compaction": {"keepRecentTokens": 0}}
        }));
        assert_eq!(layered.agent.keep_recent_tokens, Some(100));
        assert_eq!(layered.restart_limit, DEFAULT_RESTART_LIMIT);
        assert_eq!(
            layered.extra.get("settings").and_then(|value| value.get("compaction")),
            Some(&json!({"reserveTokens": 8}))
        );
        let override_agent = open(json!({
            "settings": {"compaction": {"keepRecentTokens": 100}},
            "compaction": {"keepRecentTokens": 40}
        }));
        assert_eq!(override_agent.agent.keep_recent_tokens, Some(40));
        assert_eq!(
            normalize_open(&json!({"compaction": {"keepRecentTokens": -1}})).unwrap_err(),
            "compaction.keepRecentTokens must be an integer 0 or more, got -1"
        );
        assert_eq!(
            normalize_open(&json!({"compaction": {"keepRecentTokens": 1.5}})).unwrap_err(),
            "compaction.keepRecentTokens must be an integer 0 or more, got 1.5"
        );
    }

    #[test]
    fn restart_limit_defaults_and_floors() {
        assert_eq!(open(json!({})).restart_limit, 5);
        assert_eq!(open(json!({"restartLimit": 0})).restart_limit, 5);
        assert_eq!(open(json!({"restartLimit": -3})).restart_limit, 5);
        assert_eq!(open(json!({"restartLimit": "4"})).restart_limit, 5);
        assert_eq!(open(json!({"restartLimit": 2.9})).restart_limit, 2);
        assert_eq!(
            open(json!({"settings": {"restartLimit": 4}, "restartLimit": 9})).restart_limit,
            9
        );
    }

    #[test]
    fn tools_and_cwd_reject_the_wrong_shape() {
        assert_eq!(
            normalize_open(&json!({"tools": "read"})).unwrap_err(),
            "tools must be a list of names"
        );
        assert_eq!(
            normalize_open(&json!({"tools": [""]})).unwrap_err(),
            "tool name must not be empty"
        );
        assert_eq!(
            normalize_open(&json!({"cwd": 1})).unwrap_err(),
            "cwd must be a string"
        );
        let cleared = open(json!({"tools": null, "cwd": null}));
        assert_eq!(cleared.agent.tools, None);
        assert_eq!(cleared.agent.cwd, None);
    }

    #[test]
    fn nested_agent_fields_lose_to_top_level_keys() {
        let options = open(json!({
            "agent": {
                "model": {"provider": "openrouter", "modelId": "first"},
                "cwd": "/nested",
                "instructions": "stay"
            },
            "model": {"provider": "openrouter", "modelId": "second"},
            "cwd": "/top"
        }));
        assert_eq!(options.agent.model.unwrap().model_id, "second");
        assert_eq!(options.agent.cwd.as_deref(), Some("/top"));
        assert_eq!(
            options.extra.get("agent").and_then(|value| value.get("instructions")),
            Some(&json!("stay"))
        );
    }
}
