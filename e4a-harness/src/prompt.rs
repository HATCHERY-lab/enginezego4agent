// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/prompt.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/prompt, MIT, see LICENSES/pi-MIT.txt.

//! Diff of system sections. Empty text retracts a key; this module does not call a model.

use std::collections::hash_map::Entry;
use std::collections::HashMap;

/// One system section. Empty `text` retracts `key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub key: String,
    pub text: String,
}

/// Sections to write so `already` matches `desired` in value.
///
/// Both arguments are `(key, text)` pairs in order. A repeated key keeps its
/// first position and its last text. A section is emitted only when the key is
/// missing or the text differs. A key present in `already` and absent from
/// `desired` is emitted with empty text so the caller can retract it. Changed
/// and new keys follow `desired` order; retractions follow in `already` order.
pub fn plan_system_entries(
    desired: &[(String, String)],
    already: &[(String, String)],
) -> Vec<Section> {
    let (desired_order, desired_text) = last_by_key(desired);
    let (already_order, already_text) = last_by_key(already);
    let mut planned = Vec::new();
    for key in desired_order {
        let Some(text) = desired_text.get(key).copied() else {
            continue;
        };
        if already_text.get(key).copied() == Some(text) {
            continue;
        }
        planned.push(Section {
            key: key.to_owned(),
            text: text.to_owned(),
        });
    }
    for key in already_order {
        if desired_text.contains_key(key) {
            continue;
        }
        planned.push(Section {
            key: key.to_owned(),
            text: String::new(),
        });
    }
    planned
}

/// Owned copy of `(key, text)` pairs, in the same order.
pub fn replay_sections(lines: &[(&str, &str)]) -> Vec<(String, String)> {
    lines
        .iter()
        .copied()
        .map(|(key, text)| (key.to_owned(), text.to_owned()))
        .collect()
}

/// First-seen order, last text. Matches a map updated by assigning each pair.
fn last_by_key(pairs: &[(String, String)]) -> (Vec<&str>, HashMap<&str, &str>) {
    let mut order = Vec::new();
    let mut text = HashMap::with_capacity(pairs.len());
    for (key, value) in pairs {
        let key = key.as_str();
        match text.entry(key) {
            Entry::Vacant(slot) => {
                order.push(key);
                slot.insert(value.as_str());
            }
            Entry::Occupied(mut slot) => {
                slot.insert(value.as_str());
            }
        }
    }
    (order, text)
}

#[cfg(test)]
mod tests {
    use super::{plan_system_entries, replay_sections, Section};

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .copied()
            .map(|(key, text)| (key.to_owned(), text.to_owned()))
            .collect()
    }

    fn section(key: &str, text: &str) -> Section {
        Section {
            key: key.to_owned(),
            text: text.to_owned(),
        }
    }

    #[test]
    fn identical_sections_emit_nothing() {
        let desired = pairs(&[("a", "1"), ("b", "2")]);
        let already = pairs(&[("a", "1"), ("b", "2")]);
        assert!(plan_system_entries(&desired, &already).is_empty());
    }

    #[test]
    fn changes_and_additions_keep_desired_order() {
        let desired = pairs(&[("c", "3"), ("a", "9"), ("b", "2")]);
        let already = pairs(&[("a", "1"), ("b", "2")]);
        assert_eq!(
            plan_system_entries(&desired, &already),
            vec![section("c", "3"), section("a", "9")]
        );
    }

    #[test]
    fn dropped_keys_are_empty_and_follow_the_changes() {
        let desired = pairs(&[("c", "3"), ("a", "9")]);
        let already = pairs(&[("a", "1"), ("b", "2"), ("z", "old")]);
        assert_eq!(
            plan_system_entries(&desired, &already),
            vec![
                section("c", "3"),
                section("a", "9"),
                section("b", ""),
                section("z", "")
            ]
        );
    }

    #[test]
    fn same_text_in_a_different_order_is_not_rewritten() {
        let desired = pairs(&[("b", "2"), ("a", "1")]);
        let already = pairs(&[("a", "1"), ("b", "2")]);
        assert!(plan_system_entries(&desired, &already).is_empty());
    }

    #[test]
    fn later_pair_wins_when_a_key_repeats() {
        let desired = pairs(&[("a", "old"), ("a", "new"), ("a", "final")]);
        let already = pairs(&[("a", "old"), ("a", "kept")]);
        assert_eq!(
            plan_system_entries(&desired, &already),
            vec![section("a", "final")]
        );
    }

    #[test]
    fn repeated_key_whose_last_text_matches_is_omitted() {
        let desired = pairs(&[("a", "nope"), ("a", "1")]);
        let already = pairs(&[("a", "1")]);
        assert!(plan_system_entries(&desired, &already).is_empty());
    }

    #[test]
    fn empty_desired_retracts_every_shown_key() {
        let already = pairs(&[("a", "1"), ("b", "2")]);
        assert_eq!(
            plan_system_entries(&[], &already),
            vec![section("a", ""), section("b", "")]
        );
    }

    #[test]
    fn replay_copies_pairs_in_order() {
        assert_eq!(
            replay_sections(&[("a", "1"), ("b", "2"), ("a", "3")]),
            vec![
                ("a".to_owned(), "1".to_owned()),
                ("b".to_owned(), "2".to_owned()),
                ("a".to_owned(), "3".to_owned())
            ]
        );
        assert!(replay_sections(&[]).is_empty());
    }
}
