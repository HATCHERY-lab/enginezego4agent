use e4a_types::TaskRecord;

/// A task's conversation is [`TaskRecord::conversation_id`].
///
/// Walking owners was O(depth) per change (upstream fix on main, 2026-11-11).
pub fn task_conversation_id(task: &TaskRecord) -> i64 {
    task.conversation_id
}
