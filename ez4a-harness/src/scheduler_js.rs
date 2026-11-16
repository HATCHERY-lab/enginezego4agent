// Portions derived from pi-durable 1.5.2 harness/scheduler, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Scheduler decisions that do not touch storage.
//!
//! Ready pick, crash accounting, abort marks, and version migration. No I/O.
//! Accepted states are `idle`, `running`, `waiting`, `done`, and `failed`.

/// One task the scheduler can decide about without a store.
///
/// `abort_reason` is the mark's reason. An empty string is kept as empty; it is
/// not an absent reason and it is not replaced with a default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: i64,
    pub kind: String,
    pub state: String,
    pub version: i64,
    pub abort_requested: bool,
    pub abort_reason: String,
    pub attempt: i64,
}

/// Id of the first `idle` task that is not abort-requested, in input order.
pub fn pick_ready(tasks: &[Task]) -> Option<i64> {
    tasks
        .iter()
        .find(|task| task.state == "idle" && !task.abort_requested)
        .map(|task| task.id)
}

/// Count a crash: `attempt` grows by one, saturating at `i64::MAX`.
///
/// When the new attempt is at or past `limit` and `limit` is positive, the
/// state becomes `failed`. Otherwise the state becomes `idle`.
pub fn note_crash(task: &Task, limit: i64) -> Task {
    let mut next = task.clone();
    next.attempt = task.attempt.saturating_add(1);
    if limit > 0 && next.attempt >= limit {
        next.state = "failed".to_owned();
    } else {
        next.state = "idle".to_owned();
    }
    next
}

/// Set the abort mark and store `reason` as given, including when it is empty.
///
/// A `done` or `failed` task is returned unchanged.
pub fn request_abort(task: &Task, reason: &str) -> Task {
    if task.state == "done" || task.state == "failed" {
        return task.clone();
    }
    let mut next = task.clone();
    next.abort_requested = true;
    next.abort_reason = reason.to_owned();
    next
}

/// Raise `version` to `to_version` when it is still behind that version.
///
/// A version below `from_version` is `task_too_old`. A version already at or
/// above `to_version` is left unchanged.
pub fn migrate(task: &Task, from_version: i64, to_version: i64) -> Result<Task, String> {
    if task.version < from_version {
        return Err("task_too_old".to_owned());
    }
    if task.version >= to_version {
        return Ok(task.clone());
    }
    let mut next = task.clone();
    next.version = to_version;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn pick_ready_is_the_first_idle_task_not_marked() {
        let tasks = [
            task(1, "running"),
            task(2, "waiting"),
            task(3, "done"),
            task(4, "failed"),
            Task {
                abort_requested: true,
                ..task(5, "idle")
            },
            task(6, "idle"),
            task(7, "idle"),
        ];
        assert_eq!(pick_ready(&tasks), Some(6));
        assert_eq!(pick_ready(&tasks[5..]), Some(6));
        assert_eq!(pick_ready(&[]), None);
        assert_eq!(pick_ready(&[task(8, "running")]), None);
        assert_eq!(
            pick_ready(&[Task {
                abort_requested: true,
                ..task(9, "idle")
            }]),
            None
        );
    }

    #[test]
    fn note_crash_counts_and_fails_only_at_a_positive_limit() {
        let running = Task {
            attempt: 1,
            abort_requested: true,
            abort_reason: "stop".to_owned(),
            ..task(4, "running")
        };
        let under = note_crash(&running, 3);
        assert_eq!(under.attempt, 2);
        assert_eq!(under.state, "idle");
        assert!(under.abort_requested);
        assert_eq!(under.abort_reason, "stop");
        assert_eq!(under.id, 4);
        assert_eq!(under.kind, "work");
        assert_eq!(under.version, 2);

        let failed = note_crash(&under, 3);
        assert_eq!(failed.attempt, 3);
        assert_eq!(failed.state, "failed");

        assert_eq!(note_crash(&task(1, "waiting"), 1).state, "failed");
        assert_eq!(note_crash(&task(1, "done"), 0).state, "idle");
        assert_eq!(note_crash(&task(1, "failed"), -5).state, "idle");
        assert_eq!(note_crash(&task(1, "idle"), i64::MIN).attempt, 1);

        let saturated = note_crash(
            &Task {
                attempt: i64::MAX,
                ..task(1, "running")
            },
            1,
        );
        assert_eq!(saturated.attempt, i64::MAX);
        assert_eq!(saturated.state, "failed");
    }

    #[test]
    fn request_abort_keeps_an_empty_reason_and_skips_terminals() {
        let idle = task(1, "idle");
        let empty = request_abort(&idle, "");
        assert!(empty.abort_requested);
        assert_eq!(empty.abort_reason, "");
        assert_eq!(empty.state, "idle");
        assert_eq!(empty.attempt, 0);

        let named = request_abort(&idle, "restart");
        assert_eq!(named.abort_reason, "restart");
        assert_eq!(request_abort(&task(2, "running"), "x").state, "running");
        assert_eq!(request_abort(&task(3, "waiting"), "x").state, "waiting");

        let done = Task {
            abort_reason: "kept".to_owned(),
            attempt: 4,
            ..task(8, "done")
        };
        assert_eq!(request_abort(&done, ""), done);
        let failed = task(9, "failed");
        assert_eq!(request_abort(&failed, "nope"), failed);
    }

    #[test]
    fn migrate_rejects_old_versions_and_bumps_only_the_gap() {
        let current = task(1, "idle");
        assert_eq!(
            migrate(&Task { version: 1, ..current.clone() }, 2, 5).unwrap_err(),
            "task_too_old"
        );

        let same = migrate(&current, 2, 2).expect("already there");
        assert_eq!(same, current);
        let ahead = migrate(&Task { version: 6, ..current.clone() }, 2, 5).expect("ahead");
        assert_eq!(ahead.version, 6);

        let bumped = migrate(&current, 2, 5).expect("bump");
        assert_eq!(bumped.version, 5);
        assert_eq!(bumped.state, "idle");
        assert_eq!(bumped.id, 1);
        assert!(!bumped.abort_requested);

        let between = migrate(&Task { version: 3, ..current.clone() }, 2, 5).expect("between");
        assert_eq!(between.version, 5);
        let from_above_to = migrate(&current, 4, 3).unwrap_err();
        assert_eq!(from_above_to, "task_too_old");
        let held = migrate(&Task { version: 4, ..current }, 4, 3).expect("not behind to");
        assert_eq!(held.version, 4);
    }
}
