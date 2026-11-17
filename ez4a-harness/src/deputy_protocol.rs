//! The deputy's turn, checked by code instead of by hope.
//!
//! The chat-feel protocol makes promises a model cannot be trusted to keep
//! on its own: letters are the only channel that reaches the owner, a turn
//! woken by a letter owes an answer, and a hidden wake with nothing to say
//! owes silence. This lint reads the checkable facts of one finished deputy
//! turn — was it woken by a letter, how many letters went out, did the
//! deputy answer into private text instead — and returns a verdict. One
//! verdict is a violation; the wire loop (the deputy service woken by m4a)
//! records each as an audit line, like the shepherd's decisions.

use serde::{Deserialize, Serialize};

/// What one finished turn actually did, as the runtime can observe it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnFacts {
    /// The turn began with an incoming letter that expects an answer or an
    /// acknowledgement. A hidden wake (routine, background completion,
    /// event) is `false`.
    pub woken_by_letter: bool,
    /// Letters the deputy sent during this turn (any priority).
    pub letters_sent: usize,
    /// Non-empty final assistant text. Under the protocol plain text is
    /// private: it reaches nobody.
    pub final_text_chars: usize,
}

/// The lint's verdict on one turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum TurnVerdict {
    /// Woken by a letter, answered by letter. The protocol held.
    Delivered,
    /// A hidden wake that sent letters anyway — right when the result is
    /// worth surfacing.
    DeliveredOnWake,
    /// A hidden wake that said nothing — correct for a no-change routine.
    QuietWake,
    /// Woken by a letter and silent everywhere. Allowed only for a pure
    /// FYI the deputy judged needs nothing; the audit shows it so a pattern
    /// of ignoring letters is visible.
    SilentAfterLetter,
    /// The deputy answered into private text: the owner hears nothing. The
    /// one hard violation.
    SpokeWithoutSending,
}

impl TurnVerdict {
    pub fn is_violation(&self) -> bool {
        matches!(self, TurnVerdict::SpokeWithoutSending)
    }

    /// The audit line: one verdict, one JSON object.
    pub fn audit_line(&self, facts: &TurnFacts, at_unix_ms: u64) -> String {
        serde_json::to_string(&json_line(self, facts, at_unix_ms))
            .unwrap_or_else(|_| "\"unserializable\"".into())
    }
}

fn json_line(verdict: &TurnVerdict, facts: &TurnFacts, at_unix_ms: u64) -> serde_json::Value {
    serde_json::json!({
        "at_unix_ms": at_unix_ms,
        "verdict": verdict,
        "facts": facts,
    })
}

/// Judge one finished turn.
pub fn lint(facts: &TurnFacts) -> TurnVerdict {
    match (facts.woken_by_letter, facts.letters_sent, facts.final_text_chars) {
        (true, 1.., _) => TurnVerdict::Delivered,
        (true, 0, 1..) => TurnVerdict::SpokeWithoutSending,
        (true, 0, 0) => TurnVerdict::SilentAfterLetter,
        (false, 1.., _) => TurnVerdict::DeliveredOnWake,
        (false, 0, _) => TurnVerdict::QuietWake,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(woken: bool, letters: usize, text: usize) -> TurnFacts {
        TurnFacts {
            woken_by_letter: woken,
            letters_sent: letters,
            final_text_chars: text,
        }
    }

    #[test]
    fn a_letter_woken_turn_answers_by_letter() {
        assert_eq!(lint(&facts(true, 1, 0)), TurnVerdict::Delivered);
        assert_eq!(lint(&facts(true, 2, 40)), TurnVerdict::Delivered);
    }

    #[test]
    fn answering_into_private_text_is_the_violation() {
        let verdict = lint(&facts(true, 0, 120));
        assert_eq!(verdict, TurnVerdict::SpokeWithoutSending);
        assert!(verdict.is_violation());
    }

    #[test]
    fn hidden_wakes_are_free_to_stay_quiet() {
        assert_eq!(lint(&facts(false, 0, 0)), TurnVerdict::QuietWake);
        assert!(!lint(&facts(false, 0, 0)).is_violation());
        assert_eq!(lint(&facts(false, 1, 0)), TurnVerdict::DeliveredOnWake);
        // Text on a hidden wake is still private, but nothing was owed:
        // not a violation, the audit line still shows it.
        assert!(!lint(&facts(false, 0, 80)).is_violation());
    }

    #[test]
    fn silence_after_a_letter_is_a_warning_not_a_crime() {
        let verdict = lint(&facts(true, 0, 0));
        assert_eq!(verdict, TurnVerdict::SilentAfterLetter);
        assert!(!verdict.is_violation());
    }

    #[test]
    fn the_audit_line_carries_verdict_and_facts() {
        let facts = facts(true, 0, 120);
        let line = lint(&facts).audit_line(&facts, 1731840000000);
        assert!(line.contains("\"at_unix_ms\":1731840000000"), "{line}");
        assert!(line.contains("spoke_without_sending"), "{line}");
        assert!(line.contains("\"letters_sent\":0"), "{line}");
    }
}
