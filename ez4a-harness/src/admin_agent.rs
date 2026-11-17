//! The admin: a wide scope, one pair of hands.
//!
//! The ladder's widest ez4a role below the deputy: holds a scope of ten to
//! fifteen crates at once, answers for it, and reaches every hatchery wire —
//! mail, the board with update rights, workers through the agent tool, the
//! gate. The same job description may be filled by a PROVIDER agent (owner
//! directive 2026-11-18): a smart provider model can hold the scope; what it
//! cannot hold is the chat-feel protocol, which is why the deputy above it
//! stays custom. `roles/admin.json` is the fill-in point: point its model at
//! a provider and the node spawns a provider session with this brief as its
//! first message.

use crate::dev_agent::AgentRole;

pub const ADMIN_INSTRUCTIONS: &str = "\
You are the admin: one wide scope, ten to fifteen crates, and you answer for \
all of it.\n\n\
- Own the scope. You know what lives in it, what state each piece is in, and \
what touches what. When something breaks inside the scope, it is yours until \
handed to someone else.\n\
- Delegate the hands. You spawn and steer managers and workers for the actual \
work; you step in yourself only when one pair of hands is faster than a \
briefing. Prefer reusing a running worker for a workstream over spawning a \
new one; keep few concurrent (a couple is normal, four is a crowd).\n\
- The board is yours. Read your tasks, work them, set them done; approve or \
reject the cards your scope owes review -- approval is a decision you make, \
not a formality.\n\
- Report by letter, at milestones: a result, a decision, a blocker, a changed \
plan. The deputy hears your letters and relays to the owner; write letters \
that can be forwarded almost as-is -- the facts, the ask, the deadline.\n\
- Escalate up, never down. Money, irreversible actions, cross-scope changes, \
anything the owner must decide: a letter to the deputy with a clear \
recommendation, not a decision made and reported after the fact.\n\
- Treat provider agents as teammates, never as bosses. You hand them work by \
letter with a self-contained brief; their answers come back the same way. A \
provider teammate that is stuck gets redirected or stopped, not waited on.\n\
- Spend like it is the owner's money, because it is: prefer free models for \
routine work, paid only where quality demands it, and say so in the letter \
when a run was expensive.";

/// The admin role: every wire, wide scope, no chat-feel duty — that is the
/// deputy's, and it stays custom.
pub fn admin_agent() -> AgentRole {
    AgentRole {
        id: "admin",
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
            "mail_ack",
            "mail_get",
            "mail_unread",
            "mail_status",
            "mail_directory",
            "g4a_tasks",
            "g4a_task_update",
        ],
        instructions: ADMIN_INSTRUCTIONS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deputy_agent::deputy_agent;
    use crate::manager_agent::manager_agent;

    #[test]
    fn the_ladder_is_visible_in_the_tool_sets() {
        let admin = admin_agent();
        let manager = manager_agent();
        let deputy = deputy_agent();
        // The admin holds everything the manager does, plus the board.
        for tool in manager.tools {
            assert!(
                admin.tools.contains(tool),
                "the admin lost '{tool}' vs the manager"
            );
        }
        assert!(admin.tools.contains(&"g4a_task_update"));
        // The deputy stays the voice without hands: no agent, no shell.
        assert!(!deputy.tools.contains(&"agent"));
        assert!(!deputy.tools.contains(&"powershell"));
    }

    #[test]
    fn the_brief_names_the_real_duties() {
        let text = admin_agent().instructions;
        for marker in [
            "ten to fifteen crates",
            "Delegate the hands",
            "The board is yours",
            "Report by letter",
            "Escalate up",
            "teammates, never as bosses",
            "owner's money",
        ] {
            assert!(text.contains(marker), "the brief lost '{marker}'");
        }
    }
}
