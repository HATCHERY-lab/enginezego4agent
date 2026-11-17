// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Telemetry schema identity.
//!
//! Upstream publishes a content-free span, event, and metric vocabulary. This
//! port keeps the schema name only.

/// Schema id for harness telemetry documents.
pub const TELEMETRY_SCHEMA: &str = "ez4a.telemetry/v1";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_id_is_the_v1_name() {
        assert_eq!(TELEMETRY_SCHEMA, "ez4a.telemetry/v1");
    }
}
