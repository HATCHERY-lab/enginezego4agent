//! The shepherd: the deciding floor above the overseer.
//!
//! The overseer (deterministic) says what HAPPENED; the shepherd says what
//! to DO about it. Two kinds of decisions live here, and they stay separate:
//! a policy table decides approvals without words (allow / deny / ask, the
//! field's vibe-kanban grid), everything else maps from the overseer's
//! directive to an order in the field's action vocabulary. Every decision
//! becomes one auditable [`Decision`] line — the equivalent of omp's
//! `retry_fallback_applied`: one place to watch the flock from.
//!
//! The LLM floor sits ABOVE this: `Review` orders hand the delta to the
//! advisor, `Escalate` letters carry gray zones to a human. No model call
//! happens in this module.

use serde::{Deserialize, Serialize};

use crate::overseer::{Action, Directive, SessionId};

/// What to do with an approval dialog, no words needed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalVerdict {
    Allow,
    Deny(&'static str),
    Ask,
}

/// The gatekeeper's grid: tool class → verdict. The field's three politics
/// (vibe-kanban's supervised list, claude-squad's yes-to-all, herd's
/// routine-vs-risky) are all just different fills of this table.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    /// Classes approved without words.
    pub allow: Vec<String>,
    /// Classes refused without words, each with its standing reason.
    pub deny: Vec<(String, &'static str)>,
    /// The verdict for everything the table does not name. A prudent flock
    /// asks a human; a bold one allows.
    pub unnamed: ApprovalVerdict,
}

impl Default for Policy {
    fn default() -> Self {
        // Prudent start: reads and searches pass, money and system-shape
        // refuse, everything else asks.
        Self {
            allow: ["read", "search", "list", "plan"].into_iter().map(str::to_owned).collect(),
            deny: vec![
                ("purchase".into(), "spending is a human decision"),
                ("delete-account".into(), "irreversible account change"),
            ],
            unnamed: ApprovalVerdict::Ask,
        }
    }
}

/// One order in the field's vocabulary, addressed and reasoned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "order", rename_all = "snake_case")]
pub enum Order {
    /// Push a prompt into the live turn (steer where the wire allows it,
    /// else queue).
    Steer { session: String, text: String },
    /// Buffer a prompt for after the turn.
    Queue { session: String, text: String },
    /// Answer an approval dialog.
    Approve { session: String, correlation_id: String },
    Deny { session: String, correlation_id: String, reason: String },
    /// A letter for a human: gray zones, blocker reviews, budget news.
    Escalate { session: String, letter: String },
    /// Hand the delta to the advisor branch.
    Review { session: String, since_sequence: u64 },
    /// Recorded, not yet wired: park, switch_model, restart, interrupt,
    /// compact wait for the node's control surface.
    Deferred { session: String, action: String, reason: String },
}

impl Order {
    /// The audit line: one decision, one JSON object, in order.
    pub fn decision_line(&self, at_unix_ms: u64) -> String {
        let record = DecisionLine {
            at_unix_ms,
            order: self.clone(),
        };
        serde_json::to_string(&record).unwrap_or_else(|_| "\"unserializable\"".into())
    }
}

#[derive(Serialize)]
struct DecisionLine {
    at_unix_ms: u64,
    order: Order,
}

/// Turn one overseer directive into shepherd orders. One directive may
/// become one order (the common case) or, for approvals, the verdict of the
/// policy grid.
pub fn decide(directive: &Directive, policy: &Policy) -> Vec<Order> {
    let session = directive.session.0.clone();
    match &directive.action {
        Action::Nudge { note } => vec![Order::Steer {
            session,
            text: note.clone(),
        }],
        Action::Interrupt { reason } => vec![Order::Deferred {
            session,
            action: "interrupt".into(),
            reason: reason.clone(),
        }],
        Action::Park { reason } => vec![Order::Deferred {
            session,
            action: "park".into(),
            reason: reason.clone(),
        }],
        Action::SwitchModel { to } => vec![Order::Deferred {
            session,
            action: "switch_model".into(),
            reason: format!("fallback chain next: {to}"),
        }],
        Action::NeedsDecision {
            correlation_id,
            tool_class,
        } => {
            let verdict = if policy.allow.iter().any(|class| class == tool_class) {
                ApprovalVerdict::Allow
            } else if let Some((_, reason)) = policy.deny.iter().find(|(class, _)| class == tool_class) {
                ApprovalVerdict::Deny(reason)
            } else {
                policy.unnamed.clone()
            };
            match verdict {
                ApprovalVerdict::Allow => vec![Order::Approve {
                    session,
                    correlation_id: correlation_id.clone(),
                }],
                ApprovalVerdict::Deny(reason) => vec![Order::Deny {
                    session,
                    correlation_id: correlation_id.clone(),
                    reason: reason.to_owned(),
                }],
                ApprovalVerdict::Ask => vec![Order::Escalate {
                    session,
                    letter: format!(
                        "session asks to use '{tool_class}' (request {correlation_id}); \
the policy table does not name it — approve or deny"
                    ),
                }],
            }
        }
        Action::ReviewDelta { since_sequence } => vec![Order::Review {
            session,
            since_sequence: *since_sequence,
        }],
        Action::Restart { resume, reason } => vec![Order::Deferred {
            session,
            action: "restart".into(),
            reason: format!("resume={resume}: {reason}"),
        }],
        Action::Compact {
            used_tokens,
            capacity_tokens,
        } => vec![Order::Deferred {
            session,
            action: "compact".into(),
            reason: format!("context {used_tokens}/{capacity_tokens}"),
        }],
    }
}

/// Convenience: route + decide in one pass, with the audit lines.
pub fn flock(
    overseer: &mut crate::overseer::Overseer,
    session: &SessionId,
    observation: &crate::overseer::Observation,
    policy: &Policy,
) -> Vec<Order> {
    overseer
        .route(session, observation)
        .iter()
        .flat_map(|directive| decide(directive, policy))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overseer::{Kind, Observation, Overseer};

    fn obs(seq: u64, kind: Kind) -> Observation {
        Observation {
            source_sequence: seq,
            observed_at_unix_ms: Some(seq * 1000),
            kind,
            truncated: false,
        }
    }

    #[test]
    fn the_policy_grid_decides_without_words() {
        let policy = Policy::default();
        let directive = Directive {
            session: SessionId("s1".into()),
            action: Action::NeedsDecision {
                correlation_id: "c1".into(),
                tool_class: "read".into(),
            },
        };
        assert_eq!(
            decide(&directive, &policy),
            vec![Order::Approve {
                session: "s1".into(),
                correlation_id: "c1".into()
            }]
        );

        let directive = Directive {
            session: SessionId("s1".into()),
            action: Action::NeedsDecision {
                correlation_id: "c2".into(),
                tool_class: "purchase".into(),
            },
        };
        assert_eq!(
            decide(&directive, &policy),
            vec![Order::Deny {
                session: "s1".into(),
                correlation_id: "c2".into(),
                reason: "spending is a human decision".into()
            }]
        );

        let directive = Directive {
            session: SessionId("s1".into()),
            action: Action::NeedsDecision {
                correlation_id: "c3".into(),
                tool_class: "shell".into(),
            },
        };
        let orders = decide(&directive, &policy);
        assert!(matches!(orders[0], Order::Escalate { .. }));
    }

    #[test]
    fn a_loop_becomes_a_steer_and_then_a_deferred_interrupt() {
        let policy = Policy::default();
        let mut overseer = Overseer::default();
        let session = SessionId("s1".into());
        let mut orders = Vec::new();
        orders.extend(flock(&mut overseer, &session, &obs(1, Kind::TurnStarted), &policy));
        for i in 0..24 {
            orders.extend(flock(
                &mut overseer,
                &session,
                &obs(2 + i, Kind::ToolStarted {
                    correlation_id: format!("c{i}"),
                    class: "read".into(),
                }),
                &policy,
            ));
        }
        assert!(matches!(&orders[0], Order::Steer { text, .. } if text.contains("change approach")));
        assert!(orders
            .iter()
            .any(|o| matches!(o, Order::Deferred { action, .. } if action == "interrupt")));
    }

    #[test]
    fn every_decision_leaves_an_audit_line() {
        let order = Order::Deny {
            session: "s1".into(),
            correlation_id: "c9".into(),
            reason: "spending is a human decision".into(),
        };
        let line = order.decision_line(1731840000000);
        assert!(line.contains("\"at_unix_ms\":1731840000000"));
        assert!(line.contains("\"order\":\"deny\""));
        assert!(line.contains("spending is a human decision"));
    }

    #[test]
    fn a_completed_turn_hands_the_delta_to_the_reviewer() {
        let policy = Policy::default();
        let mut overseer = Overseer::default();
        let session = SessionId("s1".into());
        flock(&mut overseer, &session, &obs(1, Kind::TurnStarted), &policy);
        let orders = flock(&mut overseer, &session, &obs(2, Kind::TurnCompleted), &policy);
        assert_eq!(
            orders,
            vec![Order::Review {
                session: "s1".into(),
                since_sequence: 0
            }]
        );
    }
}
