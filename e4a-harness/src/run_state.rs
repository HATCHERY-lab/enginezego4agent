// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/scheduler.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.
// derived from pi-durable 1.5.2 harness/scheduler, MIT, see LICENSES/pi-MIT.txt.

//! Pure scheduling decisions for one generation.
//!
//! No model call and no store. Upstream `prepare`, then `request`, then `tools`.
//! `Settle` is upstream `finish` and `drain`. `Idle` is before the first checkpoint.
//! `retry` and `poll` stay inside the request attempt and are not phases here.

use std::fmt;

use e4a_types::{AbortReason, TaskRecord};

/// Fields `blocked_reason` reads off a task record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedSubject {
    pub id: i64,
    pub kind: String,
    pub version: i64,
    pub abort_requested: bool,
    pub abort_reason: Option<AbortReason>,
}

/// Copy the block fields. The rest of the record is not kept.
pub fn blocked_subject(record: &TaskRecord) -> BlockedSubject {
    BlockedSubject {
        id: record.id,
        kind: record.kind.clone(),
        version: record.version,
        abort_requested: record.abort_requested,
        abort_reason: record.abort_reason,
    }
}

/// Why no registered definition can take the task. Wire values match `#fit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedReason {
    /// No definition is registered for the kind.
    MissingTask,
    /// The registered definition is older than the stored task.
    TaskTooOld,
    /// The definition is newer and cannot migrate, or migration already failed.
    MigrationFailed,
}

/// Registered definition compared with a stored task version. Not a registry lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisteredDefinition {
    pub version: i64,
    /// The definition has `migrate`.
    pub has_migrate: bool,
}

/// Why `subject` cannot be reserved, if it cannot.
///
/// A running, completing, or terminal task is never blocked; this subject has no
/// status, so the caller skips those. An abort request with no reason is not
/// blocked: its handler runs, or the task is orphaned. A `restart` mark still waits
/// on the definition. `migration_failed` is the earlier failure of this definition,
/// not of some other one.
pub fn blocked_reason(
    subject: &BlockedSubject,
    definition: Option<RegisteredDefinition>,
    migration_failed: bool,
) -> Option<BlockedReason> {
    if subject.abort_requested && subject.abort_reason.is_none() {
        return None;
    }
    let Some(definition) = definition else {
        return Some(BlockedReason::MissingTask);
    };
    if definition.version == subject.version {
        return None;
    }
    if definition.version < subject.version {
        return Some(BlockedReason::TaskTooOld);
    }
    if migration_failed || !definition.has_migrate {
        Some(BlockedReason::MigrationFailed)
    } else {
        None
    }
}

/// Whether restarts have hit a positive limit.
///
/// True when `restart_count >= limit` and `limit > 0`. A non-positive limit never
/// ends the task. The count is the stored `restarts` value, not an `Option`.
pub fn ended_by_crashes(restart_count: i64, limit: i64) -> bool {
    limit > 0 && restart_count >= limit
}

/// One run invocation. `attempt` is the checkpoint attempt, not a restart count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub task_id: i64,
    pub attempt: i64,
}

/// Next checkpoint attempt. `None` is the first attempt (`1`). Otherwise saturating add.
pub fn next_attempt(previous: Option<i64>) -> i64 {
    match previous {
        None => 1,
        Some(attempt) => attempt.saturating_add(1),
    }
}

/// Phase of one generation the scheduler steps. Not a model call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunPhase {
    /// No checkpoint yet.
    Idle,
    /// Upstream `prepare`.
    Prepare,
    /// Upstream `request`.
    Request,
    /// Upstream `tools`.
    Tools,
    /// Upstream `finish` and `drain`.
    Settle,
    /// The phase faulted. `message` is the fault text.
    Failed { message: String },
}

/// What a phase finished. The function does not run that work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEvent {
    /// `prepare` committed `request`.
    PrepareDone,
    /// The request finished. This slice does not read the model message.
    ModelDone,
    /// The tool round finished.
    ToolsDone,
    /// The phase threw, or it returned without durable progress.
    Fault(String),
}

/// Next phase. Illegal edges return `Err` and call nothing.
///
/// `Prepare` + [`RunEvent::PrepareDone`] → [`RunPhase::Request`].
/// `Request` + [`RunEvent::ModelDone`] → [`RunPhase::Tools`].
/// `Tools` + [`RunEvent::ToolsDone`] → [`RunPhase::Settle`].
/// `Idle`, `Prepare`, `Request`, or `Tools` + [`RunEvent::Fault`] → [`RunPhase::Failed`].
/// `Settle` and `Failed` accept no event.
pub fn advance(phase: RunPhase, event: RunEvent) -> Result<RunPhase, String> {
    match (phase, event) {
        (RunPhase::Prepare, RunEvent::PrepareDone) => Ok(RunPhase::Request),
        (RunPhase::Request, RunEvent::ModelDone) => Ok(RunPhase::Tools),
        (RunPhase::Tools, RunEvent::ToolsDone) => Ok(RunPhase::Settle),
        (
            RunPhase::Idle | RunPhase::Prepare | RunPhase::Request | RunPhase::Tools,
            RunEvent::Fault(message),
        ) => Ok(RunPhase::Failed { message }),
        (phase, event) => Err(format!("illegal transition from {phase} on {event}")),
    }
}

impl fmt::Display for RunPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            RunPhase::Idle => "idle",
            RunPhase::Prepare => "prepare",
            RunPhase::Request => "request",
            RunPhase::Tools => "tools",
            RunPhase::Settle => "settle",
            RunPhase::Failed { .. } => "failed",
        })
    }
}

impl fmt::Display for RunEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            RunEvent::PrepareDone => "prepare_done",
            RunEvent::ModelDone => "model_done",
            RunEvent::ToolsDone => "tools_done",
            RunEvent::Fault(_) => "fault",
        })
    }
}

#[cfg(test)]
mod tests {
    use e4a_types::{AbortReason, Json, TaskRecord, TaskState};

    use super::*;

    fn record(abort_requested: bool, abort_reason: Option<AbortReason>) -> TaskRecord {
        TaskRecord {
            trace: None,
            links: None,
            id: 9,
            conversation_id: 1,
            kind: "pi.generation".to_owned(),
            version: 2,
            input: Json::Null,
            owner: None,
            background: false,
            abort_requested,
            abort_reason,
            abandon_on_restart: None,
            started_at: None,
            ended_at: None,
            restarts: None,
            state: TaskState::Pending {
                checkpoint: Json::Null,
            },
            memos: None,
        }
    }

    fn subject() -> BlockedSubject {
        blocked_subject(&record(false, None))
    }

    #[test]
    fn blocked_subject_keeps_only_the_fit_fields() {
        let taken = blocked_subject(&record(true, Some(AbortReason::Restart)));
        assert_eq!(
            taken,
            BlockedSubject {
                id: 9,
                kind: "pi.generation".to_owned(),
                version: 2,
                abort_requested: true,
                abort_reason: Some(AbortReason::Restart),
            }
        );
    }

    #[test]
    fn blocked_reason_matches_fit() {
        let open = subject();
        assert_eq!(blocked_reason(&open, None, false), Some(BlockedReason::MissingTask));
        assert_eq!(
            blocked_reason(&open, Some(RegisteredDefinition { version: 1, has_migrate: true }), false),
            Some(BlockedReason::TaskTooOld)
        );
        assert_eq!(
            blocked_reason(&open, Some(RegisteredDefinition { version: 2, has_migrate: false }), true),
            None
        );
        assert_eq!(
            blocked_reason(&open, Some(RegisteredDefinition { version: 3, has_migrate: false }), false),
            Some(BlockedReason::MigrationFailed)
        );
        assert_eq!(
            blocked_reason(&open, Some(RegisteredDefinition { version: 3, has_migrate: true }), true),
            Some(BlockedReason::MigrationFailed)
        );
        assert_eq!(
            blocked_reason(&open, Some(RegisteredDefinition { version: 3, has_migrate: true }), false),
            None
        );

        let restart = blocked_subject(&record(true, Some(AbortReason::Restart)));
        assert_eq!(blocked_reason(&restart, None, false), Some(BlockedReason::MissingTask));

        let request = blocked_subject(&record(true, None));
        assert_eq!(
            blocked_reason(
                &request,
                Some(RegisteredDefinition { version: 1, has_migrate: false }),
                true
            ),
            None
        );
    }

    #[test]
    fn ended_by_crashes_needs_a_positive_limit() {
        assert!(!ended_by_crashes(0, 0));
        assert!(!ended_by_crashes(1, 0));
        assert!(!ended_by_crashes(1, -1));
        assert!(!ended_by_crashes(i64::MAX, i64::MIN));
        assert!(!ended_by_crashes(4, 5));
        assert!(ended_by_crashes(5, 5));
        assert!(ended_by_crashes(6, 5));
        assert!(ended_by_crashes(i64::MAX, 1));
        assert!(!ended_by_crashes(-1, 1));
        assert!(!ended_by_crashes(i64::MIN, i64::MAX));
    }

    #[test]
    fn next_attempt_starts_at_one_and_saturates() {
        assert_eq!(next_attempt(None), 1);
        assert_eq!(next_attempt(Some(0)), 1);
        assert_eq!(next_attempt(Some(1)), 2);
        assert_eq!(next_attempt(Some(-1)), 0);
        assert_eq!(next_attempt(Some(i64::MAX)), i64::MAX);
        let invocation = Invocation {
            task_id: 9,
            attempt: next_attempt(None),
        };
        assert_eq!(invocation.task_id, 9);
        assert_eq!(next_attempt(Some(invocation.attempt)), 2);
    }

    #[test]
    fn advance_walks_prepare_request_tools_settle() {
        let phase = advance(RunPhase::Prepare, RunEvent::PrepareDone).unwrap();
        assert_eq!(phase, RunPhase::Request);
        let phase = advance(phase, RunEvent::ModelDone).unwrap();
        assert_eq!(phase, RunPhase::Tools);
        let phase = advance(phase, RunEvent::ToolsDone).unwrap();
        assert_eq!(phase, RunPhase::Settle);
        assert_eq!(
            advance(phase, RunEvent::PrepareDone).unwrap_err(),
            "illegal transition from settle on prepare_done"
        );
    }

    #[test]
    fn fault_ends_an_open_phase_and_nowhere_else() {
        let failed = advance(RunPhase::Idle, RunEvent::Fault("cut off".to_owned())).unwrap();
        assert_eq!(
            failed,
            RunPhase::Failed {
                message: "cut off".to_owned(),
            }
        );
        assert_eq!(
            advance(RunPhase::Request, RunEvent::Fault(String::new())).unwrap(),
            RunPhase::Failed { message: String::new() }
        );
        assert_eq!(
            advance(
                RunPhase::Failed { message: "cut off".to_owned() },
                RunEvent::Fault("again".to_owned())
            )
            .unwrap_err(),
            "illegal transition from failed on fault"
        );
        assert_eq!(
            advance(RunPhase::Settle, RunEvent::Fault("late".to_owned())).unwrap_err(),
            "illegal transition from settle on fault"
        );
    }

    #[test]
    fn other_edges_are_illegal() {
        let refused = [
            advance(RunPhase::Idle, RunEvent::PrepareDone),
            advance(RunPhase::Idle, RunEvent::ModelDone),
            advance(RunPhase::Idle, RunEvent::ToolsDone),
            advance(RunPhase::Prepare, RunEvent::ModelDone),
            advance(RunPhase::Prepare, RunEvent::ToolsDone),
            advance(RunPhase::Request, RunEvent::PrepareDone),
            advance(RunPhase::Request, RunEvent::ToolsDone),
            advance(RunPhase::Tools, RunEvent::PrepareDone),
            advance(RunPhase::Tools, RunEvent::ModelDone),
            advance(RunPhase::Settle, RunEvent::ModelDone),
            advance(RunPhase::Settle, RunEvent::ToolsDone),
        ];
        for result in refused {
            assert!(result.unwrap_err().starts_with("illegal transition from "));
        }
    }
}
