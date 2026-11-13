// Portions derived from pi-durable 1.5.2 (packages/durable/src/storage/sqlite/cloudflare.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2, MIT, see LICENSES/pi-MIT.txt.

//! Cloudflare Durable Object SQLite driver name. No network and no SDK.

/// Driver `openDurableObjectSqliteStorage` selects: `DurableObjectSqliteDatabase`.
pub fn sqlite_driver() -> &'static str {
    "DurableObjectSqliteDatabase"
}

#[cfg(test)]
mod tests {
    use super::sqlite_driver;

    #[test]
    fn selects_the_durable_object_sqlite_driver() {
        assert_eq!(sqlite_driver(), "DurableObjectSqliteDatabase");
    }
}
