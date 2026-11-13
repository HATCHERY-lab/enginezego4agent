//! Generation harness. The model call sits between commits and does not hold the store writer.

mod generation;
mod usage;

pub use generation::{Generation, TurnInput, TurnKind, TurnOutcome, RESTART_CAP};
pub use usage::usage_doc;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(String);

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<e4a_session::Error> for Error {
    fn from(error: e4a_session::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<e4a_llm::Error> for Error {
    fn from(error: e4a_llm::Error) -> Self {
        Self(error.to_string())
    }
}
