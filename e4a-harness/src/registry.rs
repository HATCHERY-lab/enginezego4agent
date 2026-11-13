// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Name registry.
//!
//! Upstream installs extensions and rejects colliding task names. This port keeps
//! only the name set: empty and duplicate names are refused.

/// Registered names, in insertion order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Registry {
    pub names: Vec<String>,
}

impl Registry {
    /// Record `name`. An empty name, or one already present, is an error.
    pub fn register(&mut self, name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("name is empty".to_owned());
        }
        if self.names.iter().any(|existing| existing == name) {
            return Err(format!("duplicate name: {name}"));
        }
        self.names.push(name.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_rejects_empty_and_duplicate_names() {
        let mut registry = Registry::default();
        assert_eq!(registry.register("").unwrap_err(), "name is empty");
        assert!(registry.names.is_empty());

        registry.register("read").unwrap();
        registry.register("edit").unwrap();
        assert_eq!(registry.names, ["read", "edit"]);

        assert_eq!(
            registry.register("read").unwrap_err(),
            "duplicate name: read"
        );
        assert_eq!(registry.names.len(), 2);
    }
}
