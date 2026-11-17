// Portions derived from pi-durable unreleased main after v1.5.2, issue #11892, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! One scheduler pass over the notify set, issue #11892.
//!
//! The pass snapshots runnable ids, lets a notify land in the fresh set, then
//! hands the stale drop list to [`PendingPass`]. That type already keeps an id
//! notified again during the pass. No async and no store.

use crate::notify_pass::PendingPass;

/// Snapshot runnable ids, run `during`, pick a ready task, then finish.
///
/// Ids that were in the snapshot and are not the ready id are the drop list.
/// [`PendingPass::finish_runnable`] keeps an id `during` notified again.
pub fn drive_pass(
    pending: &mut PendingPass,
    tasks: &[crate::scheduler_js::Task],
    during: impl FnOnce(&mut PendingPass),
) -> Option<i64> {
    let pass = pending.begin_runnable();
    during(pending);
    let ready = crate::scheduler_js::pick_ready(tasks);
    let dropped: Vec<i64> = pass
        .ids
        .iter()
        .copied()
        .filter(|&id| ready != Some(id))
        .collect();
    pending.finish_runnable(pass, &dropped);
    ready
}

/// Count a crash on the first row with `id` via [`crate::scheduler_drive::on_crash`].
pub fn crash_task(
    tasks: &mut [crate::scheduler_js::Task],
    id: i64,
    limit: i64,
) -> Result<(), String> {
    crate::scheduler_drive::on_crash(tasks, id, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: i64, state: &str) -> crate::scheduler_js::Task {
        crate::scheduler_js::Task {
            id,
            kind: "work".to_owned(),
            state: state.to_owned(),
            version: 2,
            abort_requested: false,
            abort_reason: String::new(),
            attempt: 0,
        }
    }

    #[test]
    fn re_notified_snapshot_id_stays_runnable_when_finish_drops_it() {
        let mut pending = PendingPass::default();
        pending.notify_runnable(7);
        pending.notify_runnable(3);
        let tasks = [task(4, "idle")];
        let ready = drive_pass(&mut pending, &tasks, |pending| {
            pending.notify_runnable(7);
        });
        assert_eq!(ready, Some(4));
        let left = pending.begin_runnable();
        assert!(left.ids.contains(&7));
        assert!(!left.ids.contains(&3));
    }

    #[test]
    fn crash_task_moves_the_row_like_on_crash() {
        let row = crate::scheduler_js::Task {
            attempt: 1,
            abort_requested: true,
            abort_reason: "stop".to_owned(),
            ..task(2, "running")
        };
        let mut via_pass = [row.clone()];
        let mut via_drive = [row];
        crash_task(&mut via_pass, 2, 3).expect("id 2 is in the slice");
        crate::scheduler_drive::on_crash(&mut via_drive, 2, 3).expect("id 2 is in the slice");
        assert_eq!(via_pass, via_drive);

        let before = via_pass.clone();
        assert_eq!(
            crash_task(&mut via_pass, 9, 3).unwrap_err(),
            crate::scheduler_drive::on_crash(&mut via_drive, 9, 3).unwrap_err()
        );
        assert_eq!(via_pass, before);
        assert_eq!(via_drive, before);
    }
}
