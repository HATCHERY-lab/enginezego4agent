// Portions derived from the scheduler decisions in scheduler_js.rs (pi-durable 1.5.2 harness/scheduler, MIT, © 2025 Mario Zechner / Earendil Works). See LICENSES/pi-MIT.txt.

//! Apply the scheduler decisions in `scheduler_js` to rows the caller read.
//!
//! [`step`], [`on_crash`], and [`on_abort`] call [`pick_ready`], [`note_crash`],
//! and [`request_abort`]. No database call. The caller passes the rows.

use crate::scheduler_js::{note_crash, pick_ready, request_abort, Task};

/// Id from [`pick_ready`], or `None` when no row is ready.
///
/// The slice is not written. `limit` does not change the result. An idle row
/// whose `attempt` is already above zero is still just ready: `Task` has no
/// flag that it just failed, and this function does not add one. Count a
/// crash with [`on_crash`].
pub fn step(tasks: &mut [Task], limit: i64) -> Option<i64> {
    // No "just failed" flag on Task. Crash accounting stays in `on_crash`.
    let _ = limit;
    pick_ready(tasks)
}

/// Replace the first row with `id` by [`note_crash`].
///
/// A missing id is an error and the slice is unchanged. Later rows with the
/// same id stay as they were.
pub fn on_crash(tasks: &mut [Task], id: i64, limit: i64) -> Result<(), String> {
    let task = row_mut(tasks, id)?;
    *task = note_crash(task, limit);
    Ok(())
}

/// Replace the first row with `id` by [`request_abort`].
///
/// An empty `reason` stays empty. A `done` or `failed` row is left as
/// [`request_abort`] leaves it; finding the id is still success. A missing id
/// is an error and the slice is unchanged. Later rows with the same id stay.
pub fn on_abort(tasks: &mut [Task], id: i64, reason: &str) -> Result<(), String> {
    let task = row_mut(tasks, id)?;
    *task = request_abort(task, reason);
    Ok(())
}

/// First row with `id`. A missing id does not write the slice.
fn row_mut(tasks: &mut [Task], id: i64) -> Result<&mut Task, String> {
    tasks
        .iter_mut()
        .find(|task| task.id == id)
        .ok_or_else(|| format!("task {id} is missing"))
}

#[cfg(test)]
mod tests {
    use crate::scheduler_js::{note_crash, request_abort, Task};

    use super::{on_abort, on_crash, step};

    fn task(id: i64, state: &str) -> Task {
        Task {
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
    fn step_returns_pick_ready_and_does_not_write() {
        let mut tasks = [
            task(1, "running"),
            Task {
                abort_requested: true,
                ..task(2, "idle")
            },
            Task {
                attempt: 4,
                ..task(3, "idle")
            },
            task(4, "idle"),
        ];
        let before = tasks.clone();
        assert_eq!(step(&mut tasks, 1), Some(3));
        assert_eq!(tasks, before);

        let mut none: [Task; 0] = [];
        assert_eq!(step(&mut none, i64::MAX), None);
    }

    #[test]
    fn on_crash_replaces_the_first_id_and_errors_when_missing() {
        let mut tasks = [
            task(1, "running"),
            Task {
                attempt: 1,
                abort_requested: true,
                abort_reason: "stop".to_owned(),
                ..task(2, "running")
            },
            task(2, "idle"),
        ];
        let expected = note_crash(&tasks[1], 3);
        let untouched = tasks[2].clone();
        on_crash(&mut tasks, 2, 3).expect("id 2 is in the slice");
        assert_eq!(tasks[0], task(1, "running"));
        assert_eq!(tasks[1], expected);
        assert_eq!(tasks[2], untouched);

        let before = tasks.clone();
        assert_eq!(
            on_crash(&mut tasks, 9, 3).unwrap_err(),
            "task 9 is missing"
        );
        assert_eq!(tasks, before);
    }

    #[test]
    fn on_abort_uses_request_abort_and_errors_when_missing() {
        let mut tasks = [
            task(1, "idle"),
            task(1, "waiting"),
            Task {
                abort_reason: "kept".to_owned(),
                attempt: 4,
                ..task(8, "done")
            },
        ];
        let expected = request_abort(&tasks[0], "");
        let second = tasks[1].clone();
        on_abort(&mut tasks, 1, "").expect("id 1 is in the slice");
        assert_eq!(tasks[0], expected);
        assert_eq!(tasks[0].abort_reason, "");
        assert_eq!(tasks[1], second);

        let done = tasks[2].clone();
        on_abort(&mut tasks, 8, "nope").expect("a terminal id is still present");
        assert_eq!(tasks[2], done);
        assert_eq!(tasks[2], request_abort(&done, "nope"));

        let mut none: [Task; 0] = [];
        assert_eq!(
            on_abort(&mut none, 1, "x").unwrap_err(),
            "task 1 is missing"
        );
    }
}
