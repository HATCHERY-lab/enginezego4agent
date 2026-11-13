// Portions derived from pi-durable 1.5.2 (packages/durable/src/env/node-watch.ts, packages/durable/src/env/line-scan.ts, packages/durable/src/env/decode.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 env/node-watch, line-scan, and decode, MIT, see LICENSES/pi-MIT.txt.

//! Coalesced path events, lossy UTF-8, and line splits.
//!
//! Nothing here watches the filesystem or starts a thread.

use std::collections::HashMap;

/// One reported path change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchEvent {
    pub path: String,
    pub kind: WatchKind,
}

/// Kind of change for one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchKind {
    Create,
    Change,
    Remove,
}

/// Keep the last event for each path.
///
/// Paths stay in first-seen order. A later event for the same path replaces
/// the earlier one. A [`WatchKind::Remove`] after a [`WatchKind::Create`] is
/// only that remove.
pub fn coalesce(events: &[WatchEvent]) -> Vec<WatchEvent> {
    let mut index_of: HashMap<&str, usize> = HashMap::new();
    let mut kept: Vec<WatchEvent> = Vec::new();
    for event in events {
        if let Some(&index) = index_of.get(event.path.as_str()) {
            kept[index].kind = event.kind;
        } else {
            index_of.insert(event.path.as_str(), kept.len());
            kept.push(event.clone());
        }
    }
    kept
}

/// UTF-8 text. Each invalid sequence becomes U+FFFD.
pub fn decode_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Split on `\n` and drop one trailing `\r` from each piece.
///
/// A final newline keeps the empty line after it.
pub fn scan_lines(text: &str) -> Vec<&str> {
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{coalesce, decode_lossy, scan_lines, WatchEvent, WatchKind};

    fn event(path: &str, kind: WatchKind) -> WatchEvent {
        WatchEvent {
            path: path.to_owned(),
            kind,
        }
    }

    #[test]
    fn last_event_keeps_first_seen_order() {
        let events = [
            event("a", WatchKind::Create),
            event("b", WatchKind::Change),
            event("a", WatchKind::Change),
            event("c", WatchKind::Create),
            event("b", WatchKind::Remove),
        ];
        assert_eq!(
            coalesce(&events),
            vec![
                event("a", WatchKind::Change),
                event("b", WatchKind::Remove),
                event("c", WatchKind::Create),
            ]
        );
    }

    #[test]
    fn remove_after_create_is_only_remove() {
        let events = [
            event("a", WatchKind::Create),
            event("b", WatchKind::Change),
            event("a", WatchKind::Remove),
        ];
        assert_eq!(
            coalesce(&events),
            vec![event("a", WatchKind::Remove), event("b", WatchKind::Change)]
        );
    }

    #[test]
    fn empty_batch_stays_empty() {
        let kept = coalesce(&[]);
        assert!(kept.is_empty());
    }

    #[test]
    fn invalid_utf8_is_replacement_character() {
        assert_eq!(decode_lossy(b"ok"), "ok");
        assert_eq!(decode_lossy(&[]), "");
        assert_eq!(decode_lossy(&[0x68, 0xff, 0x69]), "h\u{FFFD}i");
        assert_eq!(decode_lossy(&[0xc3, 0x28]), "\u{FFFD}(");
        assert_eq!(decode_lossy(&[0xe2, 0x82]), "\u{FFFD}");
        assert_eq!(decode_lossy("é".as_bytes()), "é");
    }

    #[test]
    fn lines_strip_cr_and_keep_trailing_empty() {
        assert_eq!(scan_lines("a\nb\n"), vec!["a", "b", ""]);
        assert_eq!(scan_lines("a\r\nb\r\n"), vec!["a", "b", ""]);
        assert_eq!(scan_lines("a\nb"), vec!["a", "b"]);
        assert_eq!(scan_lines(""), vec![""]);
        assert_eq!(scan_lines("\n"), vec!["", ""]);
        assert_eq!(scan_lines("a\r"), vec!["a"]);
        assert_eq!(scan_lines("a\r\r\n"), vec!["a\r", ""]);
    }
}
