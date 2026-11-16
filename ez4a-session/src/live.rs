// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/live.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Why a run ended without an answer, and whether a cut-off count has hit its limit.
//! Pure: no storage, no model.

use serde::{Deserialize, Serialize};

/// Why a run ended without an answer. Wire names match the pi-durable union.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunFailureReason {
    /// The model's response failed after retries, or a needed compaction failed.
    ModelError,
    /// No model is configured, or the configured one is not available.
    NoModel,
    /// The run was aborted.
    Aborted,
    /// A queued reset started a new context before an answer.
    Reset,
    /// The run's task threw where it cannot recover, or storage rejected its commit.
    Faulted,
    /// The run's task was aborted while no registered definition could take it.
    Orphaned,
    /// The host stopped while the model was answering, past the retry budget.
    Interrupted,
    /// The generation was cut off the restart limit times in a row.
    RestartLimit,
    /// The context did not end in a user message or tool results, so no request was sent.
    NothingToAnswer,
    /// A tool round reached the conversation's turn cap.
    TurnLimit,
}

/// Matches harness `RESTART_CAP`. Duplicated so this crate does not depend on `ez4a-harness`.
pub const LIMIT_RESTARTS: i64 = 5;

/// How many times a task was cut off. `None` and any negative count are `0`.
pub fn cut_off_count(restarts: Option<i64>) -> i64 {
    match restarts {
        Some(count) if count >= 0 => count,
        _ => 0,
    }
}

/// Whether the cut-off count has reached `limit`. A non-positive `limit` is never too often.
pub fn cut_off_too_often(restarts: Option<i64>, limit: i64) -> bool {
    limit > 0 && cut_off_count(restarts) >= limit
}

/// `true` only for [`RunFailureReason::Interrupted`] and [`RunFailureReason::Orphaned`].
pub fn salvageable(reason: RunFailureReason) -> bool {
    matches!(
        reason,
        RunFailureReason::Interrupted | RunFailureReason::Orphaned
    )
}

#[cfg(test)]
mod tests {
    use super::{
        cut_off_count, cut_off_too_often, salvageable, RunFailureReason, LIMIT_RESTARTS,
    };

    #[test]
    fn cut_off_count_clamps_missing_and_negative() {
        assert_eq!(cut_off_count(None), 0);
        assert_eq!(cut_off_count(Some(0)), 0);
        assert_eq!(cut_off_count(Some(-1)), 0);
        assert_eq!(cut_off_count(Some(i64::MIN)), 0);
        assert_eq!(cut_off_count(Some(3)), 3);
        assert_eq!(cut_off_count(Some(i64::MAX)), i64::MAX);
    }

    #[test]
    fn cut_off_too_often_needs_a_positive_limit() {
        assert!(!cut_off_too_often(None, LIMIT_RESTARTS));
        assert!(!cut_off_too_often(Some(LIMIT_RESTARTS - 1), LIMIT_RESTARTS));
        assert!(cut_off_too_often(Some(LIMIT_RESTARTS), LIMIT_RESTARTS));
        assert!(cut_off_too_often(Some(LIMIT_RESTARTS + 1), LIMIT_RESTARTS));
        assert!(!cut_off_too_often(Some(-4), 1));
        assert!(!cut_off_too_often(Some(1), 0));
        assert!(!cut_off_too_often(Some(1), -1));
        assert!(!cut_off_too_often(Some(0), 0));
        assert!(!cut_off_too_often(None, i64::MIN));
    }

    #[test]
    fn only_interrupted_and_orphaned_are_salvageable() {
        let cases = [
            (RunFailureReason::ModelError, false),
            (RunFailureReason::NoModel, false),
            (RunFailureReason::Aborted, false),
            (RunFailureReason::Reset, false),
            (RunFailureReason::Faulted, false),
            (RunFailureReason::Orphaned, true),
            (RunFailureReason::Interrupted, true),
            (RunFailureReason::RestartLimit, false),
            (RunFailureReason::NothingToAnswer, false),
            (RunFailureReason::TurnLimit, false),
        ];
        for (reason, expected) in cases {
            assert_eq!(salvageable(reason), expected);
        }
    }

    #[test]
    fn reasons_round_trip_as_snake_case() -> Result<(), serde_json::Error> {
        let cases = [
            (RunFailureReason::ModelError, "model_error"),
            (RunFailureReason::NoModel, "no_model"),
            (RunFailureReason::Aborted, "aborted"),
            (RunFailureReason::Reset, "reset"),
            (RunFailureReason::Faulted, "faulted"),
            (RunFailureReason::Orphaned, "orphaned"),
            (RunFailureReason::Interrupted, "interrupted"),
            (RunFailureReason::RestartLimit, "restart_limit"),
            (RunFailureReason::NothingToAnswer, "nothing_to_answer"),
            (RunFailureReason::TurnLimit, "turn_limit"),
        ];
        for (reason, name) in cases {
            let encoded = serde_json::to_value(reason)?;
            assert_eq!(encoded, serde_json::Value::String(name.to_owned()));
            let decoded: RunFailureReason = serde_json::from_value(encoded)?;
            assert_eq!(decoded, reason);
        }
        Ok(())
    }
}
