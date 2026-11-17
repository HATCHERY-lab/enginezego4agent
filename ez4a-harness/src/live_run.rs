// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/live.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/live, MIT, see LICENSES/pi-MIT.txt.

//! In-memory presentation of one run: tool slots and compaction statuses.
//!
//! No store writes. Nested calls are slots whose `call_id` contains `/`
//! (`{parent_call_id}/{key}`). A failure reason is plain text on the run or
//! the compaction status; this module does not define a reason enum.

/// One tool call of the current run, model-issued or nested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolSlot {
    pub task_id: i64,
    pub call_id: String,
    pub name: String,
    pub progress: Option<String>,
    /// Set by [`finish_slot`]. A faulted call is finished with no progress.
    pub finished: bool,
}

/// One compaction task. `error` is set once it has failed and is still listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionStatus {
    pub task_id: i64,
    pub error: Option<String>,
}

/// Presentation of the run that owns `task_id`.
///
/// `ended` is `None` while the run is open. [`end_run`] sets it and clears
/// slots. Compaction statuses are not part of that clear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRun {
    pub task_id: i64,
    pub slots: Vec<ToolSlot>,
    pub compactions: Vec<CompactionStatus>,
    pub ended: Option<String>,
}

impl LiveRun {
    /// An open run: no slots, no compactions, not ended.
    pub fn new(task_id: i64) -> Self {
        Self {
            task_id,
            slots: Vec::new(),
            compactions: Vec::new(),
            ended: None,
        }
    }
}

/// The slot of tool task `task_id`, model-issued or nested, if it is still listed.
pub fn tool_slot(run: &LiveRun, task_id: i64) -> Option<&ToolSlot> {
    run.slots.iter().find(|slot| slot.task_id == task_id)
}

/// Mutable slot of tool task `task_id`, if it is still listed.
pub fn tool_slot_mut(run: &mut LiveRun, task_id: i64) -> Option<&mut ToolSlot> {
    run.slots.iter_mut().find(|slot| slot.task_id == task_id)
}

/// Mark `slot` finished and drop the output it published while running.
pub fn finish_slot(slot: &mut ToolSlot) {
    slot.finished = true;
    clear_progress(slot);
}

/// Drop running output. The result that replaces it is not stored on the slot.
pub fn clear_progress(slot: &mut ToolSlot) {
    slot.progress = None;
}

/// Append a compaction status.
///
/// Callers append in task id order. Removal keeps the relative order of the rest.
pub fn add_compaction_status(run: &mut LiveRun, status: CompactionStatus) {
    run.compactions.push(status);
}

/// Record that compaction `task_id` failed with `error`, when it is listed.
///
/// The status stays in [`LiveRun::compactions`] with `error` set, so the text
/// remains visible. [`remove_compaction_status`] drops it. There is no second
/// failure record and no store write.
pub fn fail_compaction_status(run: &mut LiveRun, task_id: i64, error: impl Into<String>) {
    if let Some(status) = run
        .compactions
        .iter_mut()
        .find(|status| status.task_id == task_id)
    {
        status.error = Some(error.into());
    }
}

/// Remove the first compaction status with `task_id`. Other statuses stay.
pub fn remove_compaction_status(run: &mut LiveRun, task_id: i64) {
    if let Some(index) = run
        .compactions
        .iter()
        .position(|status| status.task_id == task_id)
    {
        run.compactions.remove(index);
    }
}

/// Remove nested slots below the tool task `task_id`, transitively.
///
/// A nested slot is one whose `call_id` contains `/`. A call sits below an
/// ancestor when its id is `{ancestor_call_id}/{key...}`. The ancestor slot
/// itself stays. No change when `task_id` is not listed: the parent call id
/// is what the children extend.
pub fn remove_nested_slots(run: &mut LiveRun, task_id: i64) {
    let Some(root) = run
        .slots
        .iter()
        .find(|slot| slot.task_id == task_id)
        .map(|slot| slot.call_id.clone())
    else {
        return;
    };
    run.slots
        .retain(|slot| !(slot.call_id.contains('/') && nested_below(&slot.call_id, &root)));
}

/// Clear tool slots and record `ended`.
///
/// `None` means the run ended with an answer. Compaction statuses stay.
/// Nothing is written to the store.
pub fn end_run(run: &mut LiveRun, ended: Option<String>) {
    run.slots.clear();
    run.ended = ended;
}

/// `call_id` extends `ancestor` as `{ancestor}/...`, not as a raw string prefix.
fn nested_below(call_id: &str, ancestor: &str) -> bool {
    match call_id.strip_prefix(ancestor) {
        Some(rest) => rest.starts_with('/'),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        add_compaction_status, clear_progress, end_run, fail_compaction_status, finish_slot,
        remove_compaction_status, remove_nested_slots, tool_slot, tool_slot_mut, CompactionStatus,
        LiveRun, ToolSlot,
    };

    fn slot(task_id: i64, call_id: &str, name: &str) -> ToolSlot {
        ToolSlot {
            task_id,
            call_id: call_id.to_owned(),
            name: name.to_owned(),
            progress: None,
            finished: false,
        }
    }

    fn status(task_id: i64) -> CompactionStatus {
        CompactionStatus {
            task_id,
            error: None,
        }
    }

    #[test]
    fn tool_slot_finds_model_and_nested_calls() {
        let mut run = LiveRun::new(1);
        run.slots.push(slot(10, "call-a", "read"));
        run.slots.push(slot(11, "call-a/0", "bash"));
        assert_eq!(tool_slot(&run, 10).map(|slot| slot.name.as_str()), Some("read"));
        assert_eq!(tool_slot(&run, 11).map(|slot| slot.call_id.as_str()), Some("call-a/0"));
        assert!(tool_slot(&run, 12).is_none());
        assert!(tool_slot_mut(&mut run, 9).is_none());
    }

    #[test]
    fn finish_slot_marks_done_and_clears_progress() {
        let mut slot = slot(10, "call-a", "read");
        slot.progress = Some("partial".into());
        finish_slot(&mut slot);
        assert!(slot.finished);
        assert_eq!(slot.progress, None);
        assert_eq!(slot.name, "read");
        clear_progress(&mut slot);
        assert!(slot.finished);
        assert_eq!(slot.progress, None);
    }

    #[test]
    fn clear_progress_leaves_an_unfinished_slot_open() {
        let mut slot = slot(10, "call-a", "read");
        slot.progress = Some("tick".into());
        clear_progress(&mut slot);
        assert!(!slot.finished);
        assert_eq!(slot.progress, None);
        assert_eq!(slot.task_id, 10);
    }

    #[test]
    fn compaction_status_is_appended_failed_and_removed() {
        let mut run = LiveRun::new(1);
        add_compaction_status(&mut run, status(4));
        add_compaction_status(&mut run, status(7));
        fail_compaction_status(&mut run, 4, "summarizer down");
        fail_compaction_status(&mut run, 99, "missing");
        assert_eq!(
            run.compactions,
            vec![
                CompactionStatus {
                    task_id: 4,
                    error: Some("summarizer down".into()),
                },
                status(7),
            ]
        );
        remove_compaction_status(&mut run, 4);
        remove_compaction_status(&mut run, 4);
        assert_eq!(run.compactions, vec![status(7)]);
    }

    #[test]
    fn remove_nested_slots_drops_descendants_only() {
        let mut run = LiveRun::new(1);
        run.slots.extend([
            slot(10, "call-a", "read"),
            slot(11, "call-a/0", "bash"),
            slot(12, "call-a/0/1", "bash"),
            slot(13, "call-b", "read"),
            slot(14, "call-b/0", "bash"),
            slot(15, "reader/1", "bash"),
            slot(16, "call-a2", "read"),
        ]);
        remove_nested_slots(&mut run, 10);
        let ids: Vec<i64> = run.slots.iter().map(|slot| slot.task_id).collect();
        assert_eq!(ids, vec![10, 13, 14, 15, 16]);

        remove_nested_slots(&mut run, 14);
        let ids: Vec<i64> = run.slots.iter().map(|slot| slot.task_id).collect();
        assert_eq!(ids, vec![10, 13, 14, 15, 16]);

        remove_nested_slots(&mut run, 13);
        let ids: Vec<i64> = run.slots.iter().map(|slot| slot.task_id).collect();
        assert_eq!(ids, vec![10, 13, 15, 16]);

        remove_nested_slots(&mut run, 99);
        assert_eq!(run.slots.len(), 4);
    }

    #[test]
    fn nested_parent_keeps_itself_when_its_children_go() {
        let mut run = LiveRun::new(1);
        run.slots.extend([
            slot(11, "call-a/0", "bash"),
            slot(12, "call-a/0/1", "bash"),
            slot(13, "call-a/1", "bash"),
        ]);
        remove_nested_slots(&mut run, 11);
        let ids: Vec<i64> = run.slots.iter().map(|slot| slot.task_id).collect();
        assert_eq!(ids, vec![11, 13]);
    }

    #[test]
    fn end_run_clears_slots_and_records_ended() {
        let mut run = LiveRun::new(7);
        run.slots.push(slot(10, "call-a", "read"));
        run.slots.push(slot(11, "call-a/0", "bash"));
        add_compaction_status(&mut run, status(3));
        end_run(&mut run, Some("faulted".into()));
        assert!(run.slots.is_empty());
        assert_eq!(run.ended.as_deref(), Some("faulted"));
        assert_eq!(run.compactions, vec![status(3)]);
        assert_eq!(run.task_id, 7);

        end_run(&mut run, None);
        assert_eq!(run.ended, None);
        assert_eq!(run.compactions, vec![status(3)]);
    }
}
