// Portions derived from pi-durable 1.5.2 (packages/durable/src/session/transaction.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 session/transaction, MIT, see LICENSES/pi-MIT.txt.

//! Builders for the operations one session transaction commits, in caller order.
//! Pure: no storage, no I/O. These do not write.

/// One staged change inside a session transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TxOp {
    /// Append `text` as `role` on `conversation_id`.
    AppendEntry {
        conversation_id: i64,
        role: String,
        text: String,
    },
    /// Set `task_id` to `state`.
    SetTaskState { task_id: i64, state: String },
    /// Record token counts for `conversation_id`.
    NoteUsage {
        conversation_id: i64,
        input: i64,
        output: i64,
    },
}

/// Stage an entry append. `conversation_id` must be positive.
///
/// `role` must be `user`, `assistant`, `tool`, or `system`. An empty role is rejected.
/// `text` may be empty.
pub fn append_entry(conversation_id: i64, role: &str, text: &str) -> Result<TxOp, String> {
    if conversation_id <= 0 {
        return Err(format!(
            "conversation id must be positive, got {conversation_id}"
        ));
    }
    if role.is_empty() {
        return Err("role is empty".to_owned());
    }
    if !matches!(role, "user" | "assistant" | "tool" | "system") {
        return Err(format!(
            "role must be user, assistant, tool, or system, got {role}"
        ));
    }
    Ok(TxOp::AppendEntry {
        conversation_id,
        role: role.to_owned(),
        text: text.to_owned(),
    })
}

/// Stage a task-state write. `task_id` must be positive.
///
/// `state` must be `idle`, `running`, `waiting`, `done`, or `failed`.
pub fn set_task_state(task_id: i64, state: &str) -> Result<TxOp, String> {
    if task_id <= 0 {
        return Err(format!("task id must be positive, got {task_id}"));
    }
    if !matches!(state, "idle" | "running" | "waiting" | "done" | "failed") {
        return Err(format!(
            "task state must be idle, running, waiting, done, or failed, got {state}"
        ));
    }
    Ok(TxOp::SetTaskState {
        task_id,
        state: state.to_owned(),
    })
}

/// Stage a usage note. `input` and `output` must be zero or positive.
pub fn note_usage(conversation_id: i64, input: i64, output: i64) -> Result<TxOp, String> {
    if input < 0 || output < 0 {
        return Err(format!(
            "usage count must be non-negative, got input {input} output {output}"
        ));
    }
    Ok(TxOp::NoteUsage {
        conversation_id,
        input,
        output,
    })
}

/// The operations of one transaction, in the order the caller built them.
///
/// One transaction keeps caller order. This returns `ops` as a vec of references
/// and does not sort, merge, or drop.
pub fn apply_order(ops: &[TxOp]) -> Vec<&TxOp> {
    ops.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{append_entry, apply_order, note_usage, set_task_state, TxOp};

    #[test]
    fn append_entry_accepts_the_four_roles() {
        let cases = ["user", "assistant", "tool", "system"];
        for role in cases {
            let op = append_entry(1, role, "hello").expect("role");
            assert_eq!(
                op,
                TxOp::AppendEntry {
                    conversation_id: 1,
                    role: role.to_owned(),
                    text: "hello".to_owned(),
                }
            );
        }
        let empty = append_entry(i64::MAX, "user", "").expect("empty text");
        assert_eq!(
            empty,
            TxOp::AppendEntry {
                conversation_id: i64::MAX,
                role: "user".to_owned(),
                text: String::new(),
            }
        );
    }

    #[test]
    fn append_entry_rejects_bad_id_and_role() {
        assert!(append_entry(0, "user", "x").is_err());
        assert!(append_entry(-1, "user", "x").is_err());
        assert!(append_entry(i64::MIN, "assistant", "x").is_err());
        assert!(append_entry(1, "", "x").is_err());
        assert!(append_entry(1, "User", "x").is_err());
        assert!(append_entry(1, "developer", "x").is_err());
        assert!(append_entry(1, "user ", "x").is_err());
    }

    #[test]
    fn set_task_state_accepts_the_five_states() {
        let cases = ["idle", "running", "waiting", "done", "failed"];
        for state in cases {
            let op = set_task_state(7, state).expect("state");
            assert_eq!(
                op,
                TxOp::SetTaskState {
                    task_id: 7,
                    state: state.to_owned(),
                }
            );
        }
    }

    #[test]
    fn set_task_state_rejects_bad_id_and_state() {
        assert!(set_task_state(0, "idle").is_err());
        assert!(set_task_state(-3, "running").is_err());
        assert!(set_task_state(i64::MIN, "done").is_err());
        assert!(set_task_state(1, "").is_err());
        assert!(set_task_state(1, "Idle").is_err());
        assert!(set_task_state(1, "pending").is_err());
        assert!(set_task_state(1, "terminal").is_err());
        assert!(set_task_state(1, "sleeping").is_err());
    }

    #[test]
    fn note_usage_rejects_a_negative_count() {
        let ok = note_usage(4, 0, 0).expect("zero");
        assert_eq!(
            ok,
            TxOp::NoteUsage {
                conversation_id: 4,
                input: 0,
                output: 0,
            }
        );
        let max = note_usage(1, i64::MAX, i64::MAX).expect("max");
        assert_eq!(
            max,
            TxOp::NoteUsage {
                conversation_id: 1,
                input: i64::MAX,
                output: i64::MAX,
            }
        );
        assert!(note_usage(1, -1, 0).is_err());
        assert!(note_usage(1, 0, -1).is_err());
        assert!(note_usage(1, -1, -1).is_err());
        assert!(note_usage(1, i64::MIN, 1).is_err());
        assert!(note_usage(1, 1, i64::MIN).is_err());
    }

    #[test]
    fn apply_order_keeps_caller_order() {
        assert!(apply_order(&[]).is_empty());
        let ops = vec![
            append_entry(1, "system", "a").expect("entry"),
            set_task_state(2, "waiting").expect("state"),
            note_usage(9, 3, 4).expect("usage"),
            append_entry(1, "tool", "b").expect("tool"),
        ];
        let ordered = apply_order(&ops);
        assert_eq!(ordered.len(), ops.len());
        for (index, op) in ordered.iter().enumerate() {
            assert!(std::ptr::eq(*op, &ops[index]));
        }
    }
}
