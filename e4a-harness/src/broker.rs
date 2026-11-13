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
/// ASCII letter or digit replaced by `_`. An empty name is rejected. A missing
/// or empty value is `key is not set` and that error does not include the
/// value. The value is not logged and is not stored on [`KeyBroker`].
pub fn resolve(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("broker name must be non-empty".to_owned());
    }
    match std::env::var(env_key(name)) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err("key is not set".to_owned()),
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
