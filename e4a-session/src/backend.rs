//! engine4agent store selection.
//!
//! Names the opener a kind would call. This module does not open a store.

/// Which durable store a session would open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    /// SQLite file store.
    Sqlite,
    /// JSONL directory store.
    Jsonl,
    /// IndexedDB stand-in (`e4a_store::open`).
    Idb,
}

/// Accepts `sqlite`, `jsonl`, or `idb`. Any other text is an error.
pub fn parse_kind(text: &str) -> Result<StoreKind, String> {
    match text {
        "sqlite" => Ok(StoreKind::Sqlite),
        "jsonl" => Ok(StoreKind::Jsonl),
        "idb" => Ok(StoreKind::Idb),
        _ => Err(format!("unknown store kind `{text}`")),
    }
}

/// Opener to call for `kind`. Does not open anything.
pub fn describe(kind: StoreKind) -> &'static str {
    match kind {
        StoreKind::Sqlite => "SqliteStorage::open",
        StoreKind::Jsonl => "JsonlWriter::open",
        StoreKind::Idb => "e4a_store::open",
    }
}

#[cfg(test)]
mod tests {
    use super::{describe, parse_kind, StoreKind};

    #[test]
    fn parse_kind_accepts_the_three_names() {
        assert_eq!(parse_kind("sqlite"), Ok(StoreKind::Sqlite));
        assert_eq!(parse_kind("jsonl"), Ok(StoreKind::Jsonl));
        assert_eq!(parse_kind("idb"), Ok(StoreKind::Idb));
    }

    #[test]
    fn parse_kind_rejects_anything_else() {
        for text in [
            "",
            "SQLite",
            "JSONL",
            "IDB",
            "sqlite ",
            " idb",
            "memory",
            "cloudflare",
        ] {
            assert!(parse_kind(text).is_err(), "{text}");
        }
    }

    #[test]
    fn describe_names_the_opener() {
        assert_eq!(describe(StoreKind::Sqlite), "SqliteStorage::open");
        assert_eq!(describe(StoreKind::Jsonl), "JsonlWriter::open");
        assert_eq!(describe(StoreKind::Idb), "e4a_store::open");
    }
}
