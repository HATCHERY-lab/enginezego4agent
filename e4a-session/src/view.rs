// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/view.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

use std::collections::BTreeMap;

use e4a_types::{ConversationRecord, EntryRecord, Json};

#[derive(Clone, Debug, PartialEq)]
pub struct ConversationView {
    pub conversation: ConversationRecord,
    pub entries: Vec<EntryRecord>,
    pub before: Option<i64>,
    pub docs: BTreeMap<String, Json>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversationWindow {
    pub from: Option<i64>,
    pub min: usize,
    pub max: usize,
}

impl Default for ConversationWindow {
    fn default() -> Self {
        Self {
            from: None,
            min: 50,
            max: 100,
        }
    }
}

impl ConversationWindow {
    pub fn new(from: Option<i64>, min: usize, max: usize) -> Result<Self, crate::Error> {
        if min < 1 {
            return Err(crate::Error::message("ConversationWindow.min must be at least 1"));
        }
        if max < min {
            return Err(crate::Error::message(
                "ConversationWindow.max must be at least min",
            ));
        }
        Ok(Self { from, min, max })
    }
}

pub fn window_entries(
    entries: &[EntryRecord],
    window: ConversationWindow,
) -> (Vec<EntryRecord>, Option<i64>) {
    if entries.is_empty() {
        return (Vec::new(), None);
    }
    let start = start_index(entries, window);
    let before = if start == 0 {
        None
    } else {
        Some(entries[start - 1].id)
    };
    (entries[start..].to_vec(), before)
}

fn start_index(entries: &[EntryRecord], window: ConversationWindow) -> usize {
    if let Some(from) = window.from {
        let start = entries.partition_point(|entry| entry.id < from);
        if entries.len() - start <= window.max {
            return start;
        }
    }
    entries.len().saturating_sub(window.min)
}
