// engine4agent remaining host protocols. No I/O.

//! Named key broker.
//!
//! The value stores a name. It does not hold a key.

/// A broker identified by name. No key material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyBroker {
    pub name: String,
}

/// Name a key broker.
///
/// An empty name is rejected. The result does not hold a key.
pub fn broker(name: &str) -> Result<KeyBroker, String> {
    if name.is_empty() {
        return Err("broker name must be non-empty".to_owned());
    }
    Ok(KeyBroker {
        name: name.to_owned(),
    })
}

/// Environment value for `name`.
///
/// The variable is `E4A_KEY_` plus `name`, with every character that is not an
/// ASCII letter or digit replaced by `_`. An empty name is rejected. When
/// `name` is `openrouter` and that variable is missing or empty,
/// `E4A_OPENROUTER_KEY` is read instead. A missing or empty value is
/// `key is not set` and that error does not include the value. The value is
/// not logged and is not stored on [`KeyBroker`].
pub fn resolve(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("broker name must be non-empty".to_owned());
    }
    if let Some(value) = nonempty_var(&env_key(name)) {
        return Ok(value);
    }
    if name == "openrouter" {
        if let Some(value) = nonempty_var("E4A_OPENROUTER_KEY") {
            return Ok(value);
        }
    }
    Err("key is not set".to_owned())
}

fn nonempty_var(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn env_key(name: &str) -> String {
    let mut key = String::from("E4A_KEY_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            key.push(ch);
        } else {
            key.push('_');
        }
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_name_and_rejects_empty() {
        assert_eq!(
            broker("desk").unwrap(),
            KeyBroker {
                name: "desk".to_owned(),
            }
        );
        assert_eq!(broker("").unwrap_err(), "broker name must be non-empty");
    }

    #[test]
    fn resolve_reads_the_sanitized_name_and_drops_it() {
        assert_eq!(resolve("").unwrap_err(), "broker name must be non-empty");
        let name = "turnside.resolve.probe";
        let key = "E4A_KEY_turnside_resolve_probe";
        let value = "turnside-probe-value";
        let _restore = Restore(key);
        // Safety: unique test key. Restore removes it, including on panic.
        unsafe {
            std::env::set_var(key, value);
        }
        let got = resolve(name);
        unsafe {
            std::env::set_var(key, "");
        }
        let empty = resolve(name).unwrap_err();
        unsafe {
            std::env::remove_var(key);
        }
        let missing = resolve(name).unwrap_err();
        assert_eq!(got, Ok(value.to_owned()));
        assert_eq!(empty, "key is not set");
        assert_eq!(missing, "key is not set");
        assert!(!missing.contains(value));
    }

    #[test]
    fn resolve_openrouter_uses_fallback_when_primary_is_unset() {
        let primary = "E4A_KEY_openrouter";
        let fallback = "E4A_OPENROUTER_KEY";
        let _saved_primary = SavedEnv::capture(primary);
        let _saved_fallback = SavedEnv::capture(fallback);
        let value = "openrouter-fallback-probe";
        // Safety: test slots. SavedEnv puts the prior values back, including on panic.
        unsafe {
            std::env::remove_var(primary);
            std::env::set_var(fallback, value);
        }
        let got = resolve("openrouter");
        unsafe {
            std::env::set_var(primary, "");
        }
        let empty_primary = resolve("openrouter");
        unsafe {
            std::env::set_var(fallback, "");
        }
        let empty = resolve("openrouter").unwrap_err();
        unsafe {
            std::env::remove_var(fallback);
        }
        let missing = resolve("openrouter").unwrap_err();
        assert_eq!(got, Ok(value.to_owned()));
        assert_eq!(empty_primary, Ok(value.to_owned()));
        assert_eq!(empty, "key is not set");
        assert_eq!(missing, "key is not set");
        assert!(!empty.contains(value));
        assert!(!missing.contains(value));
    }

    /// Prior value of one variable. Drop writes it back and does not log it.
    struct SavedEnv {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl SavedEnv {
        fn capture(name: &'static str) -> Self {
            Self {
                name,
                previous: std::env::var_os(name),
            }
        }
    }

    impl Drop for SavedEnv {
        fn drop(&mut self) {
            // Safety: this test owns the slot until drop, then restores the prior value.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(self.name, value),
                    None => std::env::remove_var(self.name),
                }
            }
        }
    }

    struct Restore(&'static str);

    impl Drop for Restore {
        fn drop(&mut self) {
            // Safety: unique test key, cleared when the test ends.
            unsafe {
                std::env::remove_var(self.0);
            }
        }
    }
}
