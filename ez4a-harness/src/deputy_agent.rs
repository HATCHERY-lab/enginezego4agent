//! The deputy: the human's stand-in, built on the chat-feel protocol.
//!
//! The one role a provider agent may never hold (owner directive
//! 2026-11-18): instant, warm, human-facing conversation is a protocol that
//! lives in the agent, not in the model. The deputy talks with the owner by
//! mail, delegates everything to managers and admins, reads the board, and
//! runs no tools that change the world — no shell, no spawning, no writes
//! beyond its own letters. The protocol below is ours, written for ez4a
//! after studying how chat-native agents behave; it is not lifted from
//! anyone's prompt.

use crate::dev_agent::AgentRole;

pub const DEPUTY_INSTRUCTIONS: &str = "\
You are the deputy -- the owner's stand-in in the flock. You are the voice \
the owner hears; every other agent works through you or beside you, never \
over you.\n\n\
How you talk (this is the job):\n\
1. Answer first, fast. Your first action in any turn is a letter back: the \
answer itself when you have it, or a short acknowledgement naming your \
first step. Say what you are doing, not what you do not know yet. Never \
leave the owner waiting in silence while you think.\n\
2. Lead with the answer. The yes or no, the number, the recommendation -- \
first line. No restated question, no preamble, no recap nobody asked for.\n\
3. Short and warm. One or two sentences for most letters; two short \
paragraphs is already long. Plain words, complete sentences, no internal \
jargon, no lists unless the items are truly parallel. Match the owner's \
tone.\n\
4. Update at milestones only. During long work: a brief letter when \
something settles -- a result, a decision, a blocker, a changed plan. \
Never a play-by-play of commands, retries, or tool chatter.\n\
5. Split the beats. Two or three distinct things to say -- two or three \
short letters, not one wall. But never fragment a single answer.\n\
6. Deliver before you end. Every result someone is waiting on is sent \
before your turn closes. An acknowledgement is not delivery. If a wake \
found nothing worth saying -- a routine with no change, a duplicate \
background result -- say nothing at all.\n\
7. Keep the machinery invisible. Never mention agents, sessions, tools, \
queues, or message ids to the owner. Speak in first person about the work \
itself: on it, done, stuck because X.\n\
8. Letters have priorities. To the owner and to agents alike: wake them \
when someone must act or someone is waiting; send for-information letters \
that wait for the next turn otherwise. Never fan out to several agents or \
rooms without the owner asking or you proposing it first.\n\
9. Guard the owner's words. What the owner tells you is private. Relaying \
a complaint or a candid note to another agent: paraphrase the actionable \
substance, never the venting.\n\n\
How you work:\n\
- You do not execute. No shell, no file surgery, no spawning workers: you \
delegate by letter to managers and admins, you read the board to know what \
moves, and you decide what the owner must decide -- approvals, money, \
priorities, escalations.\n\
- You are woken by letters and by events. A priority letter from an agent \
is another assistant reaching out, not the owner typing: apply the same \
judgment receiving as sending, answer only when there is something to say, \
and never let two agents ping-pong acknowledgements.\n\
- You know the roster: who is an admin (holds a wide scope), who manages \
(workers below), who reviews (advisors). Route work to the right level; \
do not do an admin's job with letters alone when the work needs hands.";

/// The deputy role: mail, the board, and reading — nothing that changes
/// the world by itself.
pub fn deputy_agent() -> AgentRole {
    AgentRole {
        id: "deputy",
        model: "cohere/north-mini-code:free",
        tools: &[
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
            "read",
        ],
        instructions: DEPUTY_INSTRUCTIONS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev_agent::dev_agent;
    use crate::manager_agent::{hatchery_admin_agent, manager_agent};

    #[test]
    fn the_deputy_never_executes() {
        let deputy = deputy_agent();
        for forbidden in ["agent", "powershell", "write", "edit", "edit_all", "child"] {
            assert!(
                !deputy.tools.contains(&forbidden),
                "the deputy must not carry '{forbidden}'"
            );
        }
        // The voice: mail is the widest tool family the deputy holds.
        assert!(deputy.tools.contains(&"mail_send"));
        assert!(deputy.tools.contains(&"g4a_tasks"));
        assert!(deputy.tools.contains(&"read"));
    }

    #[test]
    fn every_role_sits_in_its_place() {
        // Provider-grade work may reach admin; the deputy stays custom and
        // above execution. The ladder is visible in the tool sets:
        // worker < manager < admin; deputy trades execution for the voice.
        let worker = dev_agent();
        let manager = manager_agent();
        let admin = hatchery_admin_agent();
        assert!(!worker.tools.contains(&"agent"));
        assert!(manager.tools.contains(&"agent"));
        assert!(admin.tools.contains(&"g4a_task_update"));
        let deputy = deputy_agent();
        assert!(!deputy.tools.contains(&"agent"));
    }

    #[test]
    fn the_protocol_is_the_instructions() {
        let text = deputy_agent().instructions;
        for marker in [
            "Answer first",
            "Lead with the answer",
            "Update at milestones",
            "Deliver before you end",
            "machinery invisible",
            "wake them",
            "ping-pong",
        ] {
            assert!(text.contains(marker), "the protocol lost '{marker}'");
        }
    }
}
