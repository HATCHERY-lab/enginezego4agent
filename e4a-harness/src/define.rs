// Portions derived from pi-durable 1.5.2 harness, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Named extension and tool descriptions.
//!
//! Upstream issue #11446: a small data entry. No code-mode and no namespaces.

use crate::Error;

/// Tool name and description. No executable body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
}

/// Extension name, description, and its tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionDef {
    pub name: String,
    pub description: String,
    pub tools: Vec<ToolDef>,
}

/// Tool description. An empty name is rejected.
pub fn define_tool(
    name: impl Into<String>,
    description: impl Into<String>,
) -> Result<ToolDef, Error> {
    let name = name.into();
    if name.is_empty() {
        return Err(Error::message("tool name must not be empty"));
    }
    Ok(ToolDef {
        name,
        description: description.into(),
    })
}

/// Extension description. An empty name is rejected.
pub fn define_extension(
    name: impl Into<String>,
    description: impl Into<String>,
    tools: Vec<ToolDef>,
) -> Result<ExtensionDef, Error> {
    let name = name.into();
    if name.is_empty() {
        return Err(Error::message("extension name must not be empty"));
    }
    Ok(ExtensionDef {
        name,
        description: description.into(),
        tools,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_keep_a_name_and_reject_an_empty_one() {
        let tool = define_tool("read", "Read a file").unwrap();
        assert_eq!(tool.name, "read");
        assert_eq!(tool.description, "Read a file");

        let extension = define_extension("fs", "", vec![tool]).unwrap();
        assert_eq!(extension.name, "fs");
        assert!(extension.description.is_empty());
        assert_eq!(extension.tools.len(), 1);
        assert_eq!(extension.tools[0].name, "read");

        let bare = define_extension("bare", "none", Vec::new()).unwrap();
        assert!(bare.tools.is_empty());

        let tool_error = define_tool("", "no").unwrap_err();
        assert_eq!(tool_error.to_string(), "tool name must not be empty");
        let extension_error = define_extension("", "x", Vec::new()).unwrap_err();
        assert_eq!(
            extension_error.to_string(),
            "extension name must not be empty"
        );
    }
}
