//! The deterministic core of the manager lineup.
//!
//! The node publishes bounded observations (hatchery-observation-protocol
//! V1); the field's consensus (research 2026-11-17) is two floors: a
//! deterministic state tracker below, an LLM manager above. This module is
//! the lower floor: it parses the observation wire (ez4a keeps its own copy
//! of the V1 types, the same convention every V1 wire crate follows), tracks
//! per-session state, and turns events into [`Directive`]s for the lineup's
//! roles — medic, gatekeeper, looper, undertaker, reviewer. No I/O, no
//  decisions that need words: those belong to the LLM floor above.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Which session an observation is about. Opaque to the tracker.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SessionId(pub String);

/// ez4a's copy of the observation wire, subset ez4a routes on. Unknown kinds
/// parse as [`Kind::Unknown`] and the router skips them: the protocol is
/// versioned and grows without asking us.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub source_sequence: u64,
    #[serde(default)]
    pub observed_at_unix_ms: Option<u64>,
    pub kind: Kind,
    #[serde(default)]
    pub truncated: bool,
}

/// The kinds the lineup reacts to, in the node's own tag shape
/// (`{"kind": "rate-limited"}`, kebab-case).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Kind {
    SessionStarted,
    Ready,
    Stopped,
    #[serde(rename_all = "kebab-case")]
    Exited {
        #[serde(default)]
        success: Option<bool>,
    },
    TurnStarted,
    Working,
    TurnCompleted,
    TurnInterrupted,
    #[serde(rename_all = "kebab-case")]
    ToolStarted {
        correlation_id: String,
        class: String,
    },
    #[serde(rename_all = "kebab-case")]
    ToolCompleted {
        correlation_id: String,
        class: String,
        success: bool,
        #[serde(default)]
        duration_ms: Option<u64>,
    },
    #[serde(rename_all = "kebab-case")]
    ApprovalRequested {
        correlation_id: String,
        tool_class: String,
    },
    RateLimited,
    #[serde(rename_all = "kebab-case")]
    ContextWindowUsage {
        used_tokens: u64,
        capacity_tokens: u64,
    },
    /// A kind this build does not know. Never routed.
    #[serde(other)]
    Unknown,
}

/// One order from the deterministic floor to the lineup. The action
/// vocabulary is the field's (research 2026-11-17): interrupt, steer, queue,
/// nudge, approve, deny, restart, park, switch_model, compact — plus
/// escalate, which hands a human or the LLM floor the decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// The turn is spinning the same tool class; redirect it in place.
    Nudge { note: String },
    /// The redirect did not take; abort the turn, keep the transcript.
    Interrupt { reason: String },
    /// A provider limit hit; park the session until its reset.
    Park { reason: String },
    /// The fallback chain's next model, when parking is already in force.
    SwitchModel { to: String },
    /// An approval dialog is open; the gatekeeper decides above this floor.
    NeedsDecision { correlation_id: String, tool_class: String },
    /// The turn answered; the reviewer reads the delta above this floor.
    ReviewDelta { since_sequence: u64 },
    /// The session died unhappy; bring it back, resuming the transcript.
    Restart { resume: bool, reason: String },
    /// The context window is nearly full.
    Compact { used_tokens: u64, capacity_tokens: u64 },
}

/// What the deterministic floor decided, for whom, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Directive {
    pub session: SessionId,
    pub action: Action,
}

/// Knobs of the deterministic floor.
#[derive(Clone, Debug, PartialEq)]
pub struct OverseerConfig {
    /// Same tool class this many times in one turn → redirect (looper,
    /// stage one). The wire carries no tool arguments, so the count is the
    /// signal we have.
    pub loop_redirect_at: usize,
    /// This many → interrupt (looper, stage two). omp's shape: redirect
    /// first, abort on the repeat.
    pub loop_interrupt_at: usize,
    /// Used/capacity at which the medic asks for compaction.
    pub context_compact_at: f64,
}

impl Default for OverseerConfig {
    fn default() -> Self {
        Self {
            loop_redirect_at: 12,
            loop_interrupt_at: 24,
            context_compact_at: 0.9,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Phase {
    Working,
    Idle,
    Stopped,
}

#[derive(Clone, Debug, PartialEq)]
struct SessionState {
    phase: Phase,
    tool_counts: BTreeMap<String, usize>,
    nudged: BTreeMap<String, bool>,
    interrupted: BTreeMap<String, bool>,
    parked: bool,
    last_reviewed_sequence: u64,
}

impl SessionState {
    fn new() -> Self {
        Self {
            phase: Phase::Idle,
            tool_counts: BTreeMap::new(),
            nudged: BTreeMap::new(),
            interrupted: BTreeMap::new(),
            parked: false,
            last_reviewed_sequence: 0,
        }
    }
}

/// The tracker: feed it observations per session, collect directives.
#[derive(Clone, Debug, Default)]
pub struct Overseer {
    config: OverseerConfig,
    sessions: BTreeMap<SessionId, SessionState>,
}

impl Overseer {
    pub fn new(config: OverseerConfig) -> Self {
        Self {
            config,
            sessions: BTreeMap::new(),
        }
    }

    /// Route one observation. The returned directives are in order; an empty
    /// vec means this floor has nothing to say.
    pub fn route(&mut self, session: &SessionId, observation: &Observation) -> Vec<Directive> {
        let state = self.sessions.entry(session.clone()).or_default();
        let mut out = Vec::new();
        match &observation.kind {
            Kind::TurnStarted => {
                state.phase = Phase::Working;
                state.tool_counts.clear();
                state.nudged.clear();
                state.interrupted.clear();
            }
            Kind::Working => state.phase = Phase::Working,
            Kind::TurnCompleted => {
                state.phase = Phase::Idle;
                out.push(Directive {
                    session: session.clone(),
                    action: Action::ReviewDelta {
                        since_sequence: state.last_reviewed_sequence,
                    },
                });
                state.last_reviewed_sequence = observation.source_sequence;
            }
            Kind::TurnInterrupted => {
                state.phase = Phase::Idle;
                // An interrupted turn is not reviewed: whatever the operator
                // did next is the verdict.
                state.last_reviewed_sequence = observation.source_sequence;
            }
            Kind::ToolStarted { class, .. } => {
                let count = state.tool_counts.entry(class.clone()).or_insert(0);
                *count += 1;
                if *count == self.config.loop_redirect_at
                    && !state.nudged.get(class).copied().unwrap_or(false)
                {
                    state.nudged.insert(class.clone(), true);
                    out.push(Directive {
                        session: session.clone(),
                        action: Action::Nudge {
                            note: format!(
                                "the same {class} tool has run {} times this turn; change approach",
                                *count
                            ),
                        },
                    });
                } else if *count >= self.config.loop_interrupt_at
                    && !state.interrupted.get(class).copied().unwrap_or(false)
                {
                    state.interrupted.insert(class.clone(), true);
                    out.push(Directive {
                        session: session.clone(),
                        action: Action::Interrupt {
                            reason: format!("{class} ran {} times in one turn", *count),
                        },
                    });
                }
            }
            Kind::ApprovalRequested {
                correlation_id,
                tool_class,
            } => {
                out.push(Directive {
                    session: session.clone(),
                    action: Action::NeedsDecision {
                        correlation_id: correlation_id.clone(),
                        tool_class: tool_class.clone(),
                    },
                });
            }
            Kind::RateLimited => {
                if state.parked {
                    out.push(Directive {
                        session: session.clone(),
                        action: Action::SwitchModel {
                            to: String::new(),
                        },
                    });
                } else {
                    state.parked = true;
                    out.push(Directive {
                        session: session.clone(),
                        action: Action::Park {
                            reason: "provider rate limit".into(),
                        },
                    });
                }
            }
            Kind::ContextWindowUsage {
                used_tokens,
                capacity_tokens,
            } => {
                if *capacity_tokens > 0
                    && *used_tokens as f64 / *capacity_tokens as f64
                        >= self.config.context_compact_at
                {
                    out.push(Directive {
                        session: session.clone(),
                        action: Action::Compact {
                            used_tokens: *used_tokens,
                            capacity_tokens: *capacity_tokens,
                        },
                    });
                }
            }
            Kind::Exited { success } => {
                state.phase = Phase::Stopped;
                if *success == Some(false) {
                    out.push(Directive {
                        session: session.clone(),
                        action: Action::Restart {
                            resume: true,
                            reason: "session exited unsuccessfully".into(),
                        },
                    });
                }
            }
            Kind::SessionStarted | Kind::Ready | Kind::Stopped | Kind::ToolCompleted { .. }
            | Kind::Unknown => {}
        }
        out
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session() -> SessionId {
        SessionId("claude/s-1".into())
    }

    fn obs(seq: u64, kind: Kind) -> Observation {
        Observation {
            source_sequence: seq,
            observed_at_unix_ms: Some(seq * 1000),
            kind,
            truncated: false,
        }
    }

    #[test]
    fn the_wire_parses_in_the_nodes_own_shape() {
        let raw = json!({
            "source_sequence": 7,
            "observed_at_unix_ms": 1731840000000u64,
            "kind": { "kind": "rate-limited" },
            "truncated": false
        });
        let parsed: Observation = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.kind, Kind::RateLimited);

        let raw = json!({
            "source_sequence": 8,
            "observed_at_unix_ms": null,
            "kind": { "kind": "approval-requested", "correlation-id": "c1", "tool-class": "execute" },
            "truncated": false
        });
        let parsed: Observation = serde_json::from_value(raw).unwrap();
        assert!(matches!(parsed.kind, Kind::ApprovalRequested { .. }));

        // A kind this build does not know still parses, and never routes.
        let raw = json!({
            "source_sequence": 9,
            "observed_at_unix_ms": null,
            "kind": { "kind": "something-new", "detail": "x" },
            "truncated": false
        });
        let parsed: Observation = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.kind, Kind::Unknown);
        let mut overseer = Overseer::default();
        assert!(overseer.route(&session(), &parsed).is_empty());
    }

    #[test]
    fn a_loop_redirects_once_then_interrupts() {
        let mut overseer = Overseer::default();
        let session = session();
        overseer.route(&session, &obs(1, Kind::TurnStarted));
        let mut directives = Vec::new();
        for i in 0..30 {
            directives.extend(overseer.route(
                &session,
                &obs(2 + i, Kind::ToolStarted {
                    correlation_id: format!("c{i}"),
                    class: "read".into(),
                }),
            ));
        }
        let nudges = directives
            .iter()
            .filter(|d| matches!(d.action, Action::Nudge { .. }))
            .count();
        let interrupts = directives
            .iter()
            .filter(|d| matches!(d.action, Action::Interrupt { .. }))
            .count();
        assert_eq!(nudges, 1, "redirect fires once");
        assert_eq!(interrupts, 1, "abort fires once, after the redirect");
    }

    #[test]
    fn a_new_turn_resets_the_loop_counters() {
        let mut overseer = Overseer::default();
        let session = session();
        overseer.route(&session, &obs(1, Kind::TurnStarted));
        for i in 0..11 {
            overseer.route(
                &session,
                &obs(2 + i, Kind::ToolStarted {
                    correlation_id: format!("c{i}"),
                    class: "read".into(),
                }),
            );
        }
        overseer.route(&session, &obs(14, Kind::TurnCompleted));
        overseer.route(&session, &obs(15, Kind::TurnStarted));
        let directives = overseer.route(
            &session,
            &obs(16, Kind::ToolStarted {
                correlation_id: "c0".into(),
                class: "read".into(),
            }),
        );
        assert!(directives.is_empty(), "the second turn starts clean");
    }

    #[test]
    fn rate_limits_park_once_then_switch() {
        let mut overseer = Overseer::default();
        let session = session();
        let first = overseer.route(&session, &obs(1, Kind::RateLimited));
        assert!(matches!(first[0].action, Action::Park { .. }));
        let second = overseer.route(&session, &obs(2, Kind::RateLimited));
        assert!(matches!(second[0].action, Action::SwitchModel { .. }));
    }

    #[test]
    fn a_completed_turn_asks_for_review_and_moves_the_cursor() {
        let mut overseer = Overseer::default();
        let session = session();
        overseer.route(&session, &obs(1, Kind::TurnStarted));
        let done = overseer.route(&session, &obs(5, Kind::TurnCompleted));
        match &done[0].action {
            Action::ReviewDelta { since_sequence } => assert_eq!(*since_sequence, 0),
            other => panic!("{other:?}"),
        }
        overseer.route(&session, &obs(6, Kind::TurnStarted));
        let done = overseer.route(&session, &obs(9, Kind::TurnCompleted));
        match &done[0].action {
            Action::ReviewDelta { since_sequence } => assert_eq!(*since_sequence, 5),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_full_context_and_a_bad_exit_act() {
        let mut overseer = Overseer::default();
        let session = session();
        let compact = overseer.route(
            &session,
            &obs(1, Kind::ContextWindowUsage {
                used_tokens: 91_000,
                capacity_tokens: 100_000,
            }),
        );
        assert!(matches!(compact[0].action, Action::Compact { .. }));
        let quiet = overseer.route(
            &session,
            &obs(2, Kind::ContextWindowUsage {
                used_tokens: 10_000,
                capacity_tokens: 100_000,
            }),
        );
        assert!(quiet.is_empty());
        let restart = overseer.route(&session, &obs(3, Kind::Exited { success: Some(false) }));
        assert!(matches!(
            restart[0].action,
            Action::Restart { resume: true, .. }
        ));
        let fine = overseer.route(&session, &obs(4, Kind::Exited { success: Some(true) }));
        assert!(fine.is_empty());
    }
}
