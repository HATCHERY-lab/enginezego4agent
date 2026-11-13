// Portions derived from pi-durable 1.5.2 (packages/durable/src/codemode/models.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! `models.*` as a script sees it: catalog lookups, classifier context, and image context.
//! Paid calls stay data. Nothing here talks to a provider.

use serde_json::{Map, Value};

/// Reserved names of code mode's internal tools. They are never listed to scripts or the model.
pub const CODEMODE_INTERNAL_PREFIX: &str = "pi.codemode.";

pub const CLASSIFY_TOOL: &str = "pi.codemode.models.classify";
pub const GENERATE_IMAGES_TOOL: &str = "pi.codemode.models.generateImages";

/// Catalog functions, host globals of the `models` namespace.
pub const CATALOG_GLOBALS: [&str; 3] = [
    "models.getModelsOfType",
    "models.getAvailableOfType",
    "models.getModelOfType",
];

/// `models.classify()` and `models.generateImages()` calls one script may have in flight.
pub const MAX_CONCURRENT_MODEL_CALLS: usize = 4;

const CLASSIFIER_CONTEXT_SHAPE: &str = "{ state: { ... }, images?: [{ type: \"image\", data: <base64>, mimeType }], questions: { <id>: { type: \"choice\", instructions, criteria: { <label>: <meaning> } } | { type: \"score\", instructions, criteria: [<lowest level>, ..., <highest level>] } | { type: \"bool\", instructions, criteria: { true: <meaning>, false: <meaning> } } } }";

const MODEL_TYPE_ORDER: [ModelType; 3] = [
    ModelType::Chat,
    ModelType::Image,
    ModelType::Classifier,
];

/// What a catalog entry is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelType {
    Chat,
    Image,
    Classifier,
}

impl ModelType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Image => "image",
            Self::Classifier => "classifier",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "chat" => Some(Self::Chat),
            "image" => Some(Self::Image),
            "classifier" => Some(Self::Classifier),
            _ => None,
        }
    }
}

/// Internal tool behind one paid `models.*` global. The script never sees `name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelTool {
    pub name: &'static str,
    pub description: &'static str,
}

pub const MODEL_TOOLS: [ModelTool; 2] = [
    ModelTool {
        name: CLASSIFY_TOOL,
        description: "models.classify() of code mode scripts.",
    },
    ModelTool {
        name: GENERATE_IMAGES_TOOL,
        description: "models.generateImages() of code mode scripts.",
    },
];

/// Provider and id, the only fields a script may use to name a model.
///
/// A script-supplied `baseUrl` or `headers` is ignored so it cannot receive credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRef {
    pub provider: String,
    pub id: String,
}

/// One catalog row `resolve_model` can see. `headers` are not stored here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownModel {
    pub model_type: ModelType,
    pub provider: String,
    pub id: String,
}

/// Image block a classifier or an image model accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageBlock {
    pub data: String,
    pub mime_type: String,
}

/// One block of `models.generateImages()` input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputBlock {
    Text { text: String },
    Image(ImageBlock),
}

/// Checked `models.generateImages()` context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagesContext {
    pub input: Vec<InputBlock>,
}

/// One classifier question after `check_classifier_context`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    Choice {
        instructions: String,
        criteria: Vec<(String, String)>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
    Bool {
        instructions: String,
        true_meaning: String,
        false_meaning: String,
    },
}

/// Checked `models.classify()` context. Question order is the object's order.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassifierContext {
    pub state: Map<String, Value>,
    pub images: Option<Vec<ImageBlock>>,
    pub questions: Vec<(String, Question)>,
}

/// Arguments of one catalog global, after the same checks as `catalogCall`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogRequest {
    ModelsOfType {
        model_type: ModelType,
        provider: Option<String>,
    },
    AvailableOfType {
        model_type: ModelType,
        provider: Option<String>,
    },
    ModelOfType {
        model_type: ModelType,
        provider: String,
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ModelError(pub String);

/// The `models` paragraph of the code-mode tool description.
pub fn models_description() -> String {
    let last = format!(
        "At most {MAX_CONCURRENT_MODEL_CALLS} classify/generateImages calls run at once; more wait."
    );
    [
        "`models` (all functions return promises):",
        "- `models.getModelsOfType(type, provider?)`, `models.getAvailableOfType(type, provider?)` (configured auth only), `models.getModelOfType(type, provider, id)`: the catalog; `type` is \"chat\", \"image\", or \"classifier\".",
        "- `models.classify(model, { state: { ... }, images?: [{ type: \"image\", data, mimeType }], questions: { <id>: { type: \"choice\", instructions, criteria: { <label>: <meaning> } } | { type: \"score\", instructions, criteria: [<lowest level>, ..., <highest level>] } | { type: \"bool\", instructions, criteria: { true: <meaning>, false: <meaning> } } } })` resolves to `{ answers: { <id>: ... }, stopReason, errorMessage?, usage? }`.",
        "- `models.generateImages(model, { input: [{ type: \"text\", text: <prompt> }, ...optional { type: \"image\", data, mimeType }] })` resolves to `{ output: [image blocks], stopReason, errorMessage?, usage? }`; show each image with `image(block)`.",
        last.as_str(),
    ]
    .join("\n")
}

/// A script value in an error message. `None` is JavaScript `undefined`.
pub fn describe_value(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "undefined".to_owned();
    };
    match value {
        Value::Null => "null".to_owned(),
        Value::Array(items) if items.is_empty() => "an empty array".to_owned(),
        Value::Array(_) => "an array".to_owned(),
        Value::Object(map) if map.is_empty() => "{}".to_owned(),
        Value::Object(map) => {
            let keys: Vec<&str> = map.keys().map(String::as_str).collect();
            let shown = keys.len().min(6);
            let mut text = String::from("{ ");
            text.push_str(&keys[..shown].join(", "));
            if keys.len() > 6 {
                text.push_str(", ...");
            }
            text.push_str(" }");
            text
        }
        Value::String(_) => "a string".to_owned(),
        Value::Number(_) => "a number".to_owned(),
        Value::Bool(_) => "a boolean".to_owned(),
    }
}

pub fn to_model_type(value: Option<&Value>) -> Result<ModelType, ModelError> {
    if let Some(Value::String(text)) = value {
        if let Some(kind) = ModelType::parse(text) {
            return Ok(kind);
        }
    }
    let shown = match value {
        None => "undefined".to_owned(),
        Some(value) => json_stringify(value),
    };
    Err(ModelError(format!(
        "Unknown model type {shown}. Use \"chat\", \"image\", or \"classifier\"."
    )))
}

/// `undefined` and `null` are an omitted provider. Anything else must be a string.
pub fn to_provider(value: Option<&Value>) -> Result<Option<String>, ModelError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(ModelError("provider must be a string".to_owned())),
    }
}

/// Copy a catalog entry and drop `headers`. Configured headers can carry credentials.
pub fn to_model_info(model: &Value) -> Result<Value, ModelError> {
    let Some(map) = model.as_object() else {
        return Err(ModelError(format!(
            "model must be an object, got {}",
            describe_value(Some(model))
        )));
    };
    let mut copy = map.clone();
    copy.remove("headers");
    Ok(Value::Object(copy))
}

/// Validate a catalog global's argument list. `args` is that list, or anything else (treated as empty).
pub fn parse_catalog_call(name: &str, args: Option<&Value>) -> Result<CatalogRequest, ModelError> {
    let empty: &[Value] = &[];
    let list = match args {
        Some(Value::Array(items)) => items.as_slice(),
        _ => empty,
    };
    let type_arg = list.first();
    let provider_arg = list.get(1);
    let id_arg = list.get(2);
    match name {
        "models.getModelsOfType" => Ok(CatalogRequest::ModelsOfType {
            model_type: to_model_type(type_arg)?,
            provider: to_provider(provider_arg)?,
        }),
        "models.getAvailableOfType" => Ok(CatalogRequest::AvailableOfType {
            model_type: to_model_type(type_arg)?,
            provider: to_provider(provider_arg)?,
        }),
        "models.getModelOfType" => {
            let (provider, id) = match (provider_arg, id_arg) {
                (Some(Value::String(provider)), Some(Value::String(id))) => {
                    (provider.clone(), id.clone())
                }
                _ => {
                    return Err(ModelError(format!(
                        "models.getModelOfType(type, provider, id) expects three strings, got ({}). The provider and the id are separate arguments, for example models.getModelOfType(\"classifier\", \"typesafe\", \"jev-latest\").",
                        list.iter()
                            .map(|item| describe_value(Some(item)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
            };
            Ok(CatalogRequest::ModelOfType {
                model_type: to_model_type(type_arg)?,
                provider,
                id,
            })
        }
        _ => Err(ModelError(format!("Unknown global {name}"))),
    }
}

/// Resolve a script model by provider and id. A wrong type names the type the catalog actually has.
pub fn resolve_model(
    name: &str,
    kind: ModelType,
    model: Option<&Value>,
    known: &[KnownModel],
) -> Result<ModelRef, ModelError> {
    let hint = list_hint(kind);
    let Some(map) = model.and_then(Value::as_object) else {
        return Err(expects_model(name, kind, model, &hint));
    };
    let (provider, id) = match (
        map.get("provider").and_then(Value::as_str),
        map.get("id").and_then(Value::as_str),
    ) {
        (Some(provider), Some(id)) => (provider, id),
        _ => return Err(expects_model(name, kind, model, &hint)),
    };
    if known_has(known, kind, provider, id) {
        return Ok(ModelRef {
            provider: provider.to_owned(),
            id: id.to_owned(),
        });
    }
    let actual = MODEL_TYPE_ORDER.into_iter().find(|other| {
        *other != kind && known_has(known, *other, provider, id)
    });
    let message = match actual {
        Some(actual) => format!(
            "\"{provider}/{id}\" is {} model, not {} model. {hint}",
            with_article(actual.as_str()),
            with_article(kind.as_str())
        ),
        None => format!(
            "Unknown {} model \"{provider}/{id}\". {hint}",
            kind.as_str()
        ),
    };
    Err(ModelError(message))
}

pub fn check_classifier_context(context: Option<&Value>) -> Result<ClassifierContext, ModelError> {
    let Some(map) = context.and_then(Value::as_object) else {
        return Err(classify_fail(&format!(
            "expects a context object as its second argument, got {}",
            describe_value(context)
        )));
    };
    let state = match map.get("state") {
        Some(Value::Object(state)) => state,
        Some(other) => {
            return Err(classify_fail(&format!(
                "context.state must be an object, got {}",
                describe_value(Some(other))
            )));
        }
        None => {
            return Err(classify_fail(&format!(
                "context.state must be an object, got {}",
                describe_value(Some(&Value::Object(map.clone())))
            )));
        }
    };
    let images = match map.get("images") {
        None => None,
        Some(Value::Array(items)) => {
            let mut blocks = Vec::with_capacity(items.len());
            for (index, image) in items.iter().enumerate() {
                blocks.push(image_block(image).ok_or_else(|| {
                    classify_fail(&format!(
                        "context.images[{index}] must be an image block, got {}",
                        describe_value(Some(image))
                    ))
                })?);
            }
            Some(blocks)
        }
        Some(other) => {
            return Err(classify_fail(&format!(
                "context.images must be an array, got {}",
                describe_value(Some(other))
            )));
        }
    };
    let Some(questions) = map.get("questions").and_then(Value::as_object) else {
        return Err(classify_fail(&format!(
            "context.questions must map question IDs to questions, got {}",
            describe_value(map.get("questions"))
        )));
    };
    if questions.is_empty() {
        return Err(classify_fail(&format!(
            "context.questions must map question IDs to questions, got {}",
            describe_value(Some(&Value::Object(questions.clone())))
        )));
    }
    let mut parsed = Vec::with_capacity(questions.len());
    for (id, question) in questions {
        parsed.push((id.clone(), parse_question(id, question)?));
    }
    Ok(ClassifierContext {
        state: state.clone(),
        images,
        questions: parsed,
    })
}

pub fn check_images_context(context: Option<&Value>) -> Result<ImagesContext, ModelError> {
    let Some(map) = context.and_then(Value::as_object) else {
        return Err(images_fail(&format!(
            "expects a context object as its second argument, got {}",
            describe_value(context)
        )));
    };
    let Some(input) = map.get("input").and_then(Value::as_array) else {
        return Err(images_fail(&format!(
            "context.input must be a non-empty array of blocks, got {}",
            describe_value(map.get("input"))
        )));
    };
    if input.is_empty() {
        return Err(images_fail(&format!(
            "context.input must be a non-empty array of blocks, got {}",
            describe_value(Some(&Value::Array(input.clone())))
        )));
    }
    let mut blocks = Vec::with_capacity(input.len());
    for (index, block) in input.iter().enumerate() {
        if let Some(text) = text_block(block) {
            blocks.push(InputBlock::Text { text });
            continue;
        }
        if let Some(image) = image_block(block) {
            blocks.push(InputBlock::Image(image));
            continue;
        }
        return Err(images_fail(&format!(
            "context.input[{index}] must be a text or image block, got {}",
            describe_value(Some(block))
        )));
    }
    Ok(ImagesContext { input: blocks })
}

fn parse_question(id: &str, question: &Value) -> Result<Question, ModelError> {
    let Some(map) = question.as_object() else {
        return Err(classify_fail(&format!(
            "context.questions.{id} must be a question object, got {}",
            describe_value(Some(question))
        )));
    };
    let Some(instructions) = map.get("instructions").and_then(Value::as_str) else {
        return Err(classify_fail(&format!(
            "context.questions.{id}.instructions must be a string"
        )));
    };
    let instructions = instructions.to_owned();
    let criteria = map.get("criteria");
    match map.get("type").and_then(Value::as_str) {
        Some("choice") => {
            let Some(criteria) = criteria.and_then(Value::as_object).filter(|map| {
                !map.is_empty() && map.values().all(Value::is_string)
            }) else {
                return Err(classify_fail(&format!(
                    "context.questions.{id} is a \"choice\" question, so criteria must map each label to its meaning"
                )));
            };
            let pairs = criteria
                .iter()
                .filter_map(|(label, meaning)| {
                    meaning.as_str().map(|meaning| (label.clone(), meaning.to_owned()))
                })
                .collect();
            Ok(Question::Choice {
                instructions,
                criteria: pairs,
            })
        }
        Some("score") => {
            let Some(criteria) = criteria.and_then(Value::as_array).filter(|items| {
                !items.is_empty() && items.iter().all(Value::is_string)
            }) else {
                return Err(classify_fail(&format!(
                    "context.questions.{id} is a \"score\" question, so criteria must list the levels as strings, lowest first"
                )));
            };
            let levels = criteria
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            Ok(Question::Score {
                instructions,
                criteria: levels,
            })
        }
        Some("bool") => {
            let true_meaning = criteria
                .and_then(Value::as_object)
                .and_then(|map| map.get("true"))
                .and_then(Value::as_str);
            let false_meaning = criteria
                .and_then(Value::as_object)
                .and_then(|map| map.get("false"))
                .and_then(Value::as_str);
            match (true_meaning, false_meaning) {
                (Some(when_true), Some(when_false)) => Ok(Question::Bool {
                    instructions,
                    true_meaning: when_true.to_owned(),
                    false_meaning: when_false.to_owned(),
                }),
                _ => Err(classify_fail(&format!(
                    "context.questions.{id} is a \"bool\" question, so criteria must be {{ true: string, false: string }}"
                ))),
            }
        }
        _ => Err(classify_fail(&format!(
            "context.questions.{id}.type must be \"choice\", \"score\", or \"bool\", got {}",
            match map.get("type") {
                None => "undefined".to_owned(),
                Some(value) => json_stringify(value),
            }
        ))),
    }
}

fn image_block(value: &Value) -> Option<ImageBlock> {
    let map = value.as_object()?;
    if map.get("type").and_then(Value::as_str) != Some("image") {
        return None;
    }
    Some(ImageBlock {
        data: map.get("data").and_then(Value::as_str)?.to_owned(),
        mime_type: map.get("mimeType").and_then(Value::as_str)?.to_owned(),
    })
}

fn text_block(value: &Value) -> Option<String> {
    let map = value.as_object()?;
    if map.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    map.get("text").and_then(Value::as_str).map(str::to_owned)
}

fn expects_model(name: &str, kind: ModelType, model: Option<&Value>, hint: &str) -> ModelError {
    let undefined_hint = if model.is_none() || matches!(model, Some(Value::Null)) {
        " models.getModelOfType() returns undefined for an unknown provider or id."
    } else {
        ""
    };
    ModelError(format!(
        "{name}() expects {} model as its first argument, got {}.{undefined_hint} {hint}",
        with_article(kind.as_str()),
        describe_value(model)
    ))
}

fn list_hint(kind: ModelType) -> String {
    let kind = kind.as_str();
    format!("List the {kind} models you can use with models.getAvailableOfType(\"{kind}\").")
}

fn with_article(word: &str) -> String {
    let article = match word.as_bytes().first() {
        Some(b'a' | b'e' | b'i' | b'o' | b'u') => "an",
        _ => "a",
    };
    format!("{article} {word}")
}

fn known_has(known: &[KnownModel], kind: ModelType, provider: &str, id: &str) -> bool {
    known
        .iter()
        .any(|model| model.model_type == kind && model.provider == provider && model.id == id)
}

fn classify_fail(problem: &str) -> ModelError {
    ModelError(format!(
        "models.classify() {problem}. Expected context: {CLASSIFIER_CONTEXT_SHAPE}."
    ))
}

fn images_fail(problem: &str) -> ModelError {
    ModelError(format!(
        "models.generateImages() {problem}. Expected context: {{ input: [{{ type: \"text\", text: <prompt> }}, ...optional {{ type: \"image\", data: <base64>, mimeType }} references] }}."
    ))
}

fn json_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_type_is_checked_before_provider() {
        let err = parse_catalog_call("models.getModelsOfType", Some(&json!(["nope", 1]))).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Unknown model type \"nope\". Use \"chat\", \"image\", or \"classifier\"."
        );
    }

    #[test]
    fn get_model_checks_provider_before_type() {
        let err = parse_catalog_call("models.getModelOfType", Some(&json!(["nope"]))).unwrap_err();
        assert!(err.to_string().starts_with(
            "models.getModelOfType(type, provider, id) expects three strings, got (a string)."
        ));
    }

    #[test]
    fn wrong_model_type_names_the_other_catalog_row() {
        let known = [KnownModel {
            model_type: ModelType::Image,
            provider: "typesafe".to_owned(),
            id: "jev".to_owned(),
        }];
        let err = resolve_model(
            "models.classify",
            ModelType::Classifier,
            Some(&json!({"provider": "typesafe", "id": "jev"})),
            &known,
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "\"typesafe/jev\" is an image model, not a classifier model. List the classifier models you can use with models.getAvailableOfType(\"classifier\")."
        );
    }

    #[test]
    fn classifier_context_rejects_a_prompt_object() {
        let err = check_classifier_context(Some(&json!({"prompt": "x"}))).unwrap_err();
        assert!(err.to_string().contains("context.state must be an object, got { prompt }"));
    }

    #[test]
    fn images_context_keeps_text_and_image_blocks() {
        let parsed = check_images_context(Some(&json!({
            "input": [
                {"type": "text", "text": "a cat"},
                {"type": "image", "data": "aaaa", "mimeType": "image/png"}
            ]
        })))
        .expect("blocks match the shape");
        assert_eq!(parsed.input.len(), 2);
    }

    #[test]
    fn headers_are_dropped() {
        let info = to_model_info(&json!({"id": "m", "headers": {"authorization": "secret"}}))
            .expect("object");
        assert!(info.get("headers").is_none());
        assert_eq!(info.get("id").and_then(Value::as_str), Some("m"));
    }
}
