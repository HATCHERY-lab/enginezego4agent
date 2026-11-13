// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Phases of one tool task.
//!
//! `Parked` is only the hook for a later suspendable port. Unreleased main after
//! tag v1.5.2 implements that work (signals, tools that park, gates, hook tasks);
//! the body is not ported. That main also breaks code-mode namespaces; no
//! namespace is defined here. No process is spawned.
//!
//! A 1.5.2 tool settles from `call` without parking. `finish` and `fail` still
//! accept `Call`. Leaving `Parked` is only `finish` or `fail`.

use crate::Error;

/// Phase of a tool task. `Parked` runs nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolPhase {
    Call,
    /// Hook for the later suspendable port. No signal, gate, or hook task runs.
    Parked,
    Done,
    Failed { message: String },
}

impl ToolPhase {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Failed { .. })
    }

    /// `Call` to `Parked`.
    pub fn park(&mut self) -> Result<(), Error> {
        match self {
            Self::Call => {
                *self = Self::Parked;
                Ok(())
            }
            other => Err(refuse(other, "park")),
        }
    }

    /// `Call` or `Parked` to `Done`.
    pub fn finish(&mut self) -> Result<(), Error> {
        match self {
            Self::Call | Self::Parked => {
                *self = Self::Done;
                Ok(())
            }
            other => Err(refuse(other, "finish")),
        }
    }

    /// `Call` or `Parked` to `Failed`. `message` must not be empty.
    pub fn fail(&mut self, message: impl Into<String>) -> Result<(), Error> {
        match self {
            Self::Call | Self::Parked => {
                let message = message.into();
                if message.is_empty() {
                    return Err(Error::message("tool failure needs a message"));
                }
                *self = Self::Failed { message };
                Ok(())
            }
            other => Err(refuse(other, "fail")),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Parked => "parked",
            Self::Done => "done",
            Self::Failed { .. } => "failed",
        }
    }
}

fn refuse(phase: &ToolPhase, action: &str) -> Error {
    Error::message(format!("tool phase {} cannot {action}", phase.name()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_finishes_or_fails_without_parking() {
        let mut phase = ToolPhase::Call;
        phase.finish().unwrap();
        assert_eq!(phase, ToolPhase::Done);
        assert!(phase.is_terminal());
        assert!(phase.park().is_err());
        assert!(phase.fail("no").is_err());

        let mut phase = ToolPhase::Call;
        phase.fail("blocked").unwrap();
        assert_eq!(
            phase,
            ToolPhase::Failed {
                message: "blocked".into(),
            }
        );
        assert!(phase.finish().is_err());
    }

    #[test]
    fn parked_is_not_terminal_and_can_finish_or_fail() {
        let mut phase = ToolPhase::Call;
        phase.park().unwrap();
        assert_eq!(phase, ToolPhase::Parked);
        assert!(!phase.is_terminal());
        assert!(phase.park().is_err());
        assert_eq!(phase, ToolPhase::Parked);
        phase.fail("interrupted").unwrap();
        assert!(phase.is_terminal());

        let mut phase = ToolPhase::Call;
        phase.park().unwrap();
        phase.finish().unwrap();
        assert_eq!(phase, ToolPhase::Done);
    }

    #[test]
    fn empty_failure_leaves_the_phase() {
        let mut phase = ToolPhase::Call;
        let error = phase.fail("").unwrap_err();
        assert_eq!(error.to_string(), "tool failure needs a message");
        assert_eq!(phase, ToolPhase::Call);
    }
}
