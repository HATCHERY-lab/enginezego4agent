// engine4agent remaining host protocols. No I/O.

//! Child spawn record.
//!
//! The record is data only. No process is started.

/// A child id, its parent, and the tool name it would run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Child {
    pub id: i64,
    pub parent: i64,
    pub tool: String,
}

/// Record a child of `parent` identified by `next_id` that names `tool`.
///
/// `parent` and `next_id` must be positive. `tool` must be non-empty.
/// Nothing is spawned.
pub fn spawn_child(parent: i64, tool: &str, next_id: i64) -> Result<Child, String> {
    if parent <= 0 {
        return Err("parent must be positive".to_owned());
    }
    if tool.is_empty() {
        return Err("tool must be non-empty".to_owned());
    }
    if next_id <= 0 {
        return Err("next id must be positive".to_owned());
    }
    Ok(Child {
        id: next_id,
        parent,
        tool: tool.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_a_positive_child_and_rejects_bad_inputs() {
        let child = spawn_child(7, "read", 8).unwrap();
        assert_eq!(
            child,
            Child {
                id: 8,
                parent: 7,
                tool: "read".to_owned(),
            }
        );
        assert_eq!(
            spawn_child(0, "read", 1).unwrap_err(),
            "parent must be positive"
        );
        assert_eq!(
            spawn_child(-1, "read", 1).unwrap_err(),
            "parent must be positive"
        );
        assert_eq!(
            spawn_child(1, "", 1).unwrap_err(),
            "tool must be non-empty"
        );
        assert_eq!(
            spawn_child(1, "read", 0).unwrap_err(),
            "next id must be positive"
        );
        assert_eq!(
            spawn_child(1, "read", -4).unwrap_err(),
            "next id must be positive"
        );
    }
}
