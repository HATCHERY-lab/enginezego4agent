// engine zero for agents remaining host protocols. No I/O.

//! Advisor tool allowlist.
//!
//! The list is the name `read` only. No tool is invoked.

/// Tool names the advisor may call.
pub fn advisor_tools() -> &'static [&'static str] {
    &["read"]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_read_only() {
        assert_eq!(advisor_tools(), &["read"]);
    }
}
