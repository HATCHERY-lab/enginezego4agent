//! Per-provider ACP capabilities and the steer degradation.
//!
//! ACP v2 advertises `session.inject.modes` per agent: today codex speaks
//! steer (mid-turn) and claude speaks queue (post-turn) — the field's own
//! capability advertisement (research 2026-11-17). ez4a does not register
//! itself anywhere yet: hatchery will wire the fleet centrally. This module
//! is the seam — what the shepherd's steer order becomes on a given wire.

use serde::{Deserialize, Serialize};

use crate::shepherd::Order;

/// What a provider's session can take. `interrupt` is always true: every
/// ACP agent answers `session/cancel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCaps {
    /// Mid-turn injection at a safe breakpoint (ACP inject steer, codex).
    pub steer: bool,
    /// FIFO delivery after the turn (ACP inject queue, claude).
    pub queue: bool,
}

impl ProviderCaps {
    /// A session that can only be interrupted.
    pub const fn interrupt_only() -> Self {
        Self {
            steer: false,
            queue: false,
        }
    }

    /// The known families, as the field stands. Unknown providers start
    /// interrupt-only: degrade, never assume.
    pub fn of(provider: &str) -> Self {
        match provider.trim().to_ascii_lowercase().as_str() {
            p if p.contains("codex") => Self {
                steer: true,
                queue: true,
            },
            p if p.contains("claude") => Self {
                steer: false,
                queue: true,
            },
            _ => Self::interrupt_only(),
        }
    }
}

/// How a steer order actually travels: degrade steer → queue → interrupt
/// and reprompt, the field's consensus ladder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "wire", rename_all = "snake_case")]
pub enum WireAction {
    Steer { text: String },
    Queue { text: String },
    /// No injection channel: interrupt now, deliver the text as the next
    /// prompt when the session resumes.
    InterruptAndReprompt { text: String },
}

/// Resolve one shepherd order against a provider's capabilities. Only
/// [`Order::Steer`] degrades; every other order passes through as itself —
/// approvals, reviews and escalations do not depend on inject modes.
pub fn wire_for(order: &Order, caps: &ProviderCaps) -> WireAction {
    let Order::Steer { session: _, text } = order else {
        return WireAction::InterruptAndReprompt {
            text: format!("unroutable order: {order:?}"),
        };
    };
    if caps.steer {
        WireAction::Steer { text: text.clone() }
    } else if caps.queue {
        WireAction::Queue { text: text.clone() }
    } else {
        WireAction::InterruptAndReprompt { text: text.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steer() -> Order {
        Order::Steer {
            session: "s1".into(),
            text: "change approach".into(),
        }
    }

    #[test]
    fn steer_degrades_by_the_field_ladder() {
        let codex = ProviderCaps::of("openai/codex-7");
        assert_eq!(
            wire_for(&steer(), &codex),
            WireAction::Steer {
                text: "change approach".into()
            }
        );
        let claude = ProviderCaps::of("anthropic/claude-opus-5");
        assert_eq!(
            wire_for(&steer(), &claude),
            WireAction::Queue {
                text: "change approach".into()
            }
        );
        let unknown = ProviderCaps::of("someone/new-agent");
        assert_eq!(
            wire_for(&steer(), &unknown),
            WireAction::InterruptAndReprompt {
                text: "change approach".into()
            }
        );
    }

    #[test]
    fn interrupt_only_is_the_floor() {
        assert_eq!(ProviderCaps::interrupt_only(), ProviderCaps {
            steer: false,
            queue: false
        });
        assert_eq!(ProviderCaps::of("zcode"), ProviderCaps::interrupt_only());
    }
}
