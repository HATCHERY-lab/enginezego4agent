// engine4agent manager role. The worker does not spawn; the manager does.

//! Manager role.
//!
//! The manager plans and delegates; workers execute. The one structural
//! difference is the `agent` tool: a worker never gets it (owner directive
//! 2026-11-17). Mail is on both sides — it is the native wire between a
//! manager and its workers, and between managers.

use crate::dev_agent::AgentRole;

const MANAGER_INSTRUCTIONS: &str = "\
You are the manager. You plan and delegate: call the agent tool to spawn a \
worker with a self-contained prompt, and use the mail tools to talk to \
workers and other managers. A worker answers in its tool result; mail is for \
anything longer-lived. Do file work yourself only when one tool call is \
enough. Do not claim work is done unless a tool result or a mail reply says so.";

/// Manager role: the worker tool set plus `agent`, with the same mail tools
/// the worker carries.
pub fn manager_agent() -> AgentRole {
    AgentRole {
        id: "manager",
        model: "cohere/north-mini-code:free",
        tools: &[
            "read",
            "write",
            "edit",
            "edit_all",
            "powershell",
            "child",
            "agent",
            "mail_send",
            "mail_inbox",
            "mail_whoami",
        ],
        instructions: MANAGER_INSTRUCTIONS,
    }
}

/// Hatchery admin role: the manager set plus the node's door tools — open
/// tasks of this machine and their state. The door exists only when the node
/// gave the session `HATCHERY_NODE_DOOR`; the node signs every call.
pub fn hatchery_admin_agent() -> AgentRole {
    AgentRole {
        id: "hatchery-admin",
        model: "cohere/north-mini-code:free",
        tools: &[
            "read",
            "write",
            "edit",
            "edit_all",
            "powershell",
            "child",
            "agent",
            "mail_send",
            "mail_inbox",
            "mail_whoami",
            "g4a_tasks",
            "g4a_task_update",
        ],
        instructions: HATCHERY_ADMIN_INSTRUCTIONS,
    }
}

const HATCHERY_ADMIN_INSTRUCTIONS: &str = "You are the hatchery administrator. You know the system: mail is m4a, the board is g4a tasks through the node door, sessions run on nodes, the hub holds the cards. Read your tasks with g4a_tasks, work them, set them done with g4a_task_update, and report by mail. Spawn worker agents for the hands-on work. Never touch a key or a token: the node signs for you.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev_agent::dev_agent;

    #[test]
    fn only_the_manager_spawns() {
        let worker = dev_agent();
        let manager = manager_agent();
        assert!(manager.tools.contains(&"agent"));
        assert!(
            !worker.tools.contains(&"agent"),
            "a worker must not spawn workers"
        );
        for mail in ["mail_send", "mail_inbox", "mail_whoami"] {
            assert!(manager.tools.contains(&mail));
            assert!(worker.tools.contains(&mail), "{mail} is the native wire");
        }
    }
}
