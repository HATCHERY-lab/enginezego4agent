// Portions derived from pi-durable 1.5.2 harness, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Provider-facing session identity.
//!
//! A persisted non-empty id is reused. Otherwise the caller-minted id is taken.
//! This function does not read or write a document.

/// Keep `current` when it is a non-empty id. Otherwise return `minted`.
///
/// `None` and `Some("")` both fall through to `minted`. An empty mint is an error.
pub fn ensure_provider_session_id(current: Option<&str>, minted: &str) -> Result<String, String> {
    if let Some(id) = current {
        if !id.is_empty() {
            return Ok(id.to_owned());
        }
    }
    if minted.is_empty() {
        return Err("minted provider session id is empty".to_owned());
    }
    Ok(minted.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_persisted_id_and_rejects_an_empty_mint() {
        assert_eq!(
            ensure_provider_session_id(Some("persisted"), "minted").unwrap(),
            "persisted"
        );
        assert_eq!(
            ensure_provider_session_id(Some("persisted"), "").unwrap(),
            "persisted"
        );
        assert_eq!(
            ensure_provider_session_id(None, "minted").unwrap(),
            "minted"
        );
        assert_eq!(
            ensure_provider_session_id(Some(""), "minted").unwrap(),
            "minted"
        );
        assert_eq!(
            ensure_provider_session_id(None, "").unwrap_err(),
            "minted provider session id is empty"
        );
        assert_eq!(
            ensure_provider_session_id(Some(""), "").unwrap_err(),
            "minted provider session id is empty"
        );
    }
}
