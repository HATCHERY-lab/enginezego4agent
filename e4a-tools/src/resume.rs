//! Finish tool tasks that a dead process left waiting.
//!
//! The gate board is not read. A waiting `pi.generation` in the store is enough.

use e4a_harness::Generation;
use e4a_llm::FauxLlm;
use e4a_session::Session;
use e4a_types::{TaskQuery, TaskStatus};

use crate::{run_waiting_gated, waiting_tool_id, Error, ToolOutcome, Workspace};

const GENERATION_KIND: &str = "pi.generation";

/// Run every waiting tool and release its generation.
///
/// No model is called. A tool that needs one (`jev`, `advise`) returns that
/// error and stops the scan. An empty result means nothing was waiting.
pub async fn resume_waiting(
    session: &Session,
    workspace: &Workspace,
) -> Result<Vec<ToolOutcome>, Error> {
    let waiting = session
        .scan_tasks(TaskQuery {
            status: Some(TaskStatus::Waiting),
            ..TaskQuery::default()
        })
        .await?;
    let mut finished = Vec::new();
    for task in waiting {
        if task.kind != GENERATION_KIND {
            continue;
        }
        if waiting_tool_id(task.id, &task.state).is_err() {
            continue;
        }
        let outcome =
            run_waiting_gated::<FauxLlm>(session, task.id, workspace, None).await?;
        Generation::bind(task.id, task.conversation_id)
            .release_finished_tool(session)
            .await?;
        finished.push(outcome);
    }
    Ok(finished)
}
