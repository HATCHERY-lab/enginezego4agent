// Portions derived from pi-durable issue #11676 / #11961, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Provider session ids owned by one harness.
//!
//! Close releases only the pi-ai session ids that harness used, after its tasks
//! have joined. A second live harness keeps its ids. No network and no provider.

use std::collections::{BTreeMap, BTreeSet};

/// Session ids recorded against the harness that used them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProviderSessions {
    by_harness: BTreeMap<String, BTreeSet<String>>,
}

impl ProviderSessions {
    /// Record `provider_session_id` for `harness_id`.
    ///
    /// An empty harness id or an empty session id is an error. The same session
    /// id on the same harness is stored once.
    pub fn remember(
        &mut self,
        harness_id: &str,
        provider_session_id: &str,
    ) -> Result<(), String> {
        if harness_id.is_empty() {
            return Err("harness id is empty".to_owned());
        }
        if provider_session_id.is_empty() {
            return Err("provider session id is empty".to_owned());
        }
        self.by_harness
            .entry(harness_id.to_owned())
            .or_default()
            .insert(provider_session_id.to_owned());
        Ok(())
    }

    /// That harness's session ids, sorted, then forgotten.
    ///
    /// An unknown harness returns an empty vec. Other harnesses stay as they are.
    pub fn release(&mut self, harness_id: &str) -> Vec<String> {
        self.by_harness
            .remove(harness_id)
            .map(|ids| ids.into_iter().collect())
            .unwrap_or_default()
    }

    /// Whether `harness_id` still holds `provider_session_id`.
    pub fn contains(&self, harness_id: &str, provider_session_id: &str) -> bool {
        self.by_harness
            .get(harness_id)
            .is_some_and(|ids| ids.contains(provider_session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_returns_only_that_harness_sorted() {
        let mut sessions = ProviderSessions::default();
        sessions.remember("a", "prov-b").unwrap();
        sessions.remember("a", "prov-a").unwrap();
        sessions.remember("a", "prov-b").unwrap();
        sessions.remember("b", "prov-kept").unwrap();

        assert_eq!(
            sessions.release("a"),
            vec!["prov-a".to_owned(), "prov-b".to_owned()]
        );
        assert!(sessions.contains("b", "prov-kept"));
        assert!(!sessions.contains("a", "prov-a"));
        assert!(sessions.release("a").is_empty());
        assert!(sessions.contains("b", "prov-kept"));
    }

    #[test]
    fn empty_ids_error() {
        let mut sessions = ProviderSessions::default();
        assert_eq!(
            sessions.remember("", "prov-a").unwrap_err(),
            "harness id is empty"
        );
        assert_eq!(
            sessions.remember("a", "").unwrap_err(),
            "provider session id is empty"
        );
        assert!(!sessions.contains("", "prov-a"));
        assert!(!sessions.contains("a", ""));
        assert!(sessions.release("a").is_empty());
    }
}
