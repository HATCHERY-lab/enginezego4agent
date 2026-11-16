// engine zero for agents remaining host protocols. No I/O.

//! SSH target text.
//!
//! `user@host` is split into owned strings. No socket is opened.

/// User and host from a `user@host` target. No port and no key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub user: String,
}

/// Parse `user@host`.
///
/// Exactly one `@` is required. An empty user or host is an error.
/// No network lookup is performed.
pub fn parse_target(text: &str) -> Result<SshTarget, String> {
    let Some((user, host)) = text.split_once('@') else {
        return Err("target must be user@host".to_owned());
    };
    if host.contains('@') {
        return Err("target must be user@host".to_owned());
    }
    if user.is_empty() {
        return Err("ssh user must be non-empty".to_owned());
    }
    if host.is_empty() {
        return Err("ssh host must be non-empty".to_owned());
    }
    Ok(SshTarget {
        host: host.to_owned(),
        user: user.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_user_at_host_and_rejects_empty_sides() {
        assert_eq!(
            parse_target("ada@example").unwrap(),
            SshTarget {
                host: "example".to_owned(),
                user: "ada".to_owned(),
            }
        );
        assert_eq!(
            parse_target("example").unwrap_err(),
            "target must be user@host"
        );
        assert_eq!(
            parse_target("ada@host@extra").unwrap_err(),
            "target must be user@host"
        );
        assert_eq!(parse_target("@host").unwrap_err(), "ssh user must be non-empty");
        assert_eq!(
            parse_target("ada@").unwrap_err(),
            "ssh host must be non-empty"
        );
    }
}
