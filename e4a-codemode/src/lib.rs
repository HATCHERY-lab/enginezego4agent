// Portions derived from pi-durable 1.5.2 (packages/durable/src/codemode), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Code-mode tool protocol. Models, tool descriptions, and the text the tool
//! shows the model are plain data. No JavaScript engine is linked.

pub mod describe;
pub mod models;
pub mod output;
pub mod protocol;

pub use describe::{
    codemode_description, codemode_section, declaration_of, longest_namespace, namespace_of,
    to_codemode_identifier, CodemodeNamespace, ToolDeclaration, ToolDescription,
    DEFAULT_CODEMODE_INLINE_BUDGET,
};
pub use models::{
    check_classifier_context, check_images_context, describe_value, models_description,
    parse_catalog_call, resolve_model, to_model_info, to_model_type, to_provider, CatalogRequest,
    ClassifierContext, ImageBlock, ImagesContext, InputBlock, KnownModel, ModelError, ModelRef,
    ModelTool, ModelType, Question, CATALOG_GLOBALS, CLASSIFY_TOOL, CODEMODE_INTERNAL_PREFIX,
    GENERATE_IMAGES_TOOL, MAX_CONCURRENT_MODEL_CALLS, MODEL_TOOLS,
};
pub use output::{
    image_extension, parse_output, token_estimate, CallNote, OutputError, OutputFailure,
    ToolResult, DEFAULT_MAX_OUTPUT_TOKENS,
};
pub use protocol::{
    parse_host_message, parse_source, parse_worker_message, CallOutcome, CallStatus, CallTarget,
    CodemodeError, DeclaredGlobal, DeclaredTool, During, ErrorKind, HostMessage, JournalEntry,
    JournalState, NamespaceTools, NestedCall, OutputLimits, ParsedSource, ProtocolError, RunStart,
    RunState, ScriptOutput, SourceError, SourceOptions, ToolDetails, WorkerMessage,
    CODEMODE_MODULE_VERSION, CODEMODE_TOOL_NAME, DEFAULT_OUTPUT_LIMITS, DEFAULT_SEARCH_LIMIT,
    DISCOVERY_GLOBALS,
};

/// One sandbox execution. `tools` are the names the script may call.
///
/// This is the host's request, not a QuickJS worker start. Nothing here runs JavaScript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxRequest {
    pub code: String,
    pub tools: Vec<String>,
}

/// What a sandbox returns when it does not run the script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxReply {
    pub ok: bool,
    pub text: String,
}

/// No JavaScript runtime is linked.
///
/// Empty `code` is the same error reply. It does not panic.
pub fn eval_unsupported(req: &SandboxRequest) -> SandboxReply {
    if req.code.is_empty() {
        return SandboxReply {
            ok: false,
            text: "js runtime not linked".to_owned(),
        };
    }
    let _ = req.tools.len();
    SandboxReply {
        ok: false,
        text: "js runtime not linked".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_code_is_an_error_reply() {
        let reply = eval_unsupported(&SandboxRequest {
            code: String::new(),
            tools: Vec::new(),
        });
        assert!(!reply.ok);
        assert_eq!(reply.text, "js runtime not linked");
    }

    #[test]
    fn source_is_not_run() {
        let reply = eval_unsupported(&SandboxRequest {
            code: "return 1".to_owned(),
            tools: vec!["read".to_owned()],
        });
        assert!(!reply.ok);
        assert_eq!(reply.text, "js runtime not linked");
    }

    #[test]
    fn namespace_uses_double_underscore_not_slash() {
        assert_eq!(namespace_of("mcp__github__search"), Some("mcp"));
        assert_eq!(namespace_of("read"), None);
        assert_eq!(namespace_of("ns/tool"), None);
        assert_eq!(namespace_of("__hidden"), None);
    }
}
