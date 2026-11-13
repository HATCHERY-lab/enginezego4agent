// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Error text.
//!
//! Upstream formats thrown values and names session failures. This port only
//! joins a message with an optional source.

/// Join `message` with `source` using `": "` when a source is present.
pub fn error_chain(message: &str, source: Option<&str>) -> String {
    match source {
        Some(source) => format!("{message}: {source}"),
        None => message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_a_source_and_keeps_a_bare_message() {
        assert_eq!(
            error_chain("Session failed", Some("storage")),
            "Session failed: storage"
        );
        assert_eq!(error_chain("Session failed", None), "Session failed");
        assert_eq!(error_chain("empty source", Some("")), "empty source: ");
    }
}
