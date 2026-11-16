// engine4agent dev agent. Pi supplies the tool loop; the model does not.

//! Dev agent role.
//!
//! Data the engine runs. The model is not assumed to own tools, reasoning, or
//! compaction. The harness supplies them. No network.

/// A named role: which model, which tools, and the standing instructions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentRole {
    pub id: &'static str,
    pub model: &'static str,
    pub tools: &'static [&'static str],
    pub instructions: &'static str,
}

/// Developer role.
///
/// File changes go through tools. A model thought is not a tool result and is
/// not stored as context. Only working tools are offered: the gate and the
/// advisor are not tools the worker calls (owner directive 2026-11-16), and a
/// worker never spawns workers (owner directive 2026-11-17) — that is the
/// manager's `agent` tool. Mail is the worker's wire back to its manager.
pub fn dev_agent() -> AgentRole {
    AgentRole {
        id: "dev",
        model: "cohere/north-mini-code:free",
        tools: &[
            "read",
            "write",
            "edit",
            "edit_all",
            "powershell",
            "child",
            "mail_send",
            "mail_inbox",
            "mail_whoami",
        ],
        instructions: "You are the developer. Change files only through tools. \
Do not claim a file changed unless a tool result says so. \
Thinking from the model is not stored as context.",
    }
}

/// Model used when the free window on [`dev_agent`]'s model is closed.
///
/// A name only. Nothing is called.
pub fn fallback_model() -> &'static str {
    "google/gemma-4-31b-it:free"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_role_names_the_free_tools_model_and_its_fallback() {
        let role = dev_agent();
        assert_eq!(role.id, "dev");
        assert_eq!(role.model, "cohere/north-mini-code:free");
        assert_eq!(
            role.tools,
            &[
                "read",
                "write",
                "edit",
                "edit_all",
                "powershell",
                "child",
                "mail_send",
                "mail_inbox",
                "mail_whoami"
            ][..]
        );
        assert!(role.instructions.contains("You are the developer."));
        assert!(role
            .instructions
            .contains("Do not claim a file changed unless a tool result says so."));
        assert!(role
            .instructions
            .contains("Thinking from the model is not stored as context."));
        assert_eq!(fallback_model(), "google/gemma-4-31b-it:free");
        assert_ne!(role.model, fallback_model());
    }
}
