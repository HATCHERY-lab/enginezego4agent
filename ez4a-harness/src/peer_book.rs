//! The peer book: the provider sessions the flock keeps.
//!
//! Provider agents are teammates (owner directive 2026-11-18): briefed by
//! letter, never bosses, and managed through the same action vocabulary as
//! everything else. The book is the roster — who is out there, on which
//! provider, with which injection capabilities — and the two operations the
//! shepherd needs on it: steer one peer under its capabilities (the
//! steer → queue → interrupt ladder from `provider_caps`), and stop the
//! whole flock of them at once. No I/O: the book decides, a wire delivers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::provider_caps::{wire_for, ProviderCaps, WireAction};
use crate::shepherd::Order;

/// One provider teammate out there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// The address letters reach: a mail id or a session handle.
    pub id: String,
    /// Provider family, for capability lookup: `openai/codex-7`, `claude`, …
    pub provider: String,
    /// What this peer's session can take; defaults from the provider
    /// family, unknown families degrade to interrupt-only.
    pub caps: ProviderCaps,
}

/// The roster of live provider peers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PeerBook {
    peers: BTreeMap<String, Peer>,
}

impl PeerBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a peer; its caps come from the provider family unless the
    /// caller knows better. Re-registering replaces.
    pub fn register(&mut self, id: &str, provider: &str, caps: Option<ProviderCaps>) -> Peer {
        let peer = Peer {
            id: id.to_owned(),
            provider: provider.to_owned(),
            caps: caps.unwrap_or_else(|| ProviderCaps::of(provider)),
        };
        self.peers.insert(peer.id.clone(), peer.clone());
        peer
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.peers.remove(id).is_some()
    }

    pub fn get(&self, id: &str) -> Option<&Peer> {
        self.peers.get(id)
    }

    /// Everyone out there, in id order.
    pub fn roster(&self) -> Vec<&Peer> {
        self.peers.values().collect()
    }

    /// Steer one peer under its capabilities: the order degrades by the
    /// field's ladder — steer where the wire allows it, else queue, else
    /// interrupt and carry the text as the next prompt.
    pub fn steer(&self, id: &str, text: &str) -> Result<(Peer, WireAction), String> {
        let peer = self
            .peers
            .get(id)
            .ok_or_else(|| format!("unknown peer: {id}"))?;
        let order = Order::Steer {
            session: peer.id.clone(),
            text: text.to_owned(),
        };
        Ok((peer.clone(), wire_for(&order, &peer.caps)))
    }

    /// Stop every peer at once — the cascade stop. Every peer gets the
    /// interrupt its wire always allows; the returned pairs carry the
    /// original steer texts for whoever wants to reprompt after the stop.
    pub fn cascade_stop(&self, reason: &str) -> Vec<(Peer, WireAction)> {
        self.peers
            .values()
            .map(|peer| {
                let order = Order::Steer {
                    session: peer.id.clone(),
                    text: reason.to_owned(),
                };
                let action = match wire_for(&order, &peer.caps) {
                    // A stop is not a steer: whatever the wire could inject,
                    // the order now is to end the turn, everywhere.
                    WireAction::Steer { text } | WireAction::Queue { text }
                    | WireAction::InterruptAndReprompt { text } => WireAction::InterruptAndReprompt {
                        text,
                    },
                };
                (peer.clone(), action)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_come_from_the_family_and_degrade_for_unknowns() {
        let mut book = PeerBook::new();
        let codex = book.register("peer-cx", "openai/codex-7", None);
        assert!(codex.caps.steer && codex.caps.queue);
        let claude = book.register("peer-cl", "anthropic/claude-opus-5", None);
        assert!(!claude.caps.steer && claude.caps.queue);
        let stranger = book.register("peer-??", "someone/new", None);
        assert_eq!(stranger.caps, ProviderCaps::interrupt_only());
    }

    #[test]
    fn steer_degrades_by_the_ladder() {
        let mut book = PeerBook::new();
        book.register("cx", "codex", None);
        book.register("cl", "claude", None);
        book.register("zz", "unknown-co", None);

        assert!(matches!(
            book.steer("cx", "change approach").unwrap().1,
            WireAction::Steer { .. }
        ));
        assert!(matches!(
            book.steer("cl", "change approach").unwrap().1,
            WireAction::Queue { .. }
        ));
        assert!(matches!(
            book.steer("zz", "change approach").unwrap().1,
            WireAction::InterruptAndReprompt { .. }
        ));
        assert!(book.steer("ghost", "x").is_err());
    }

    #[test]
    fn a_cascade_stop_interrupts_everyone_including_the_steerable() {
        let mut book = PeerBook::new();
        book.register("cx", "codex", None);
        book.register("cl", "claude", None);
        let stops = book.cascade_stop("owner said stop");
        assert_eq!(stops.len(), 2);
        for (peer, action) in &stops {
            assert!(matches!(action, WireAction::InterruptAndReprompt { .. }));
            assert_eq!(peer.id, if peer.provider == "codex" { "cx" } else { "cl" });
        }
    }

    #[test]
    fn the_roster_is_id_ordered_and_replaceable() {
        let mut book = PeerBook::new();
        book.register("b", "codex", None);
        book.register("a", "claude", None);
        let ids: Vec<&str> = book.roster().iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        let replaced = book.register("a", "codex", None);
        assert!(replaced.caps.steer);
        assert!(book.get("a").unwrap().caps.steer);
        assert!(book.remove("a"));
        assert!(!book.remove("a"));
        assert_eq!(book.roster().len(), 1);
    }
}
