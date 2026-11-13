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
}
