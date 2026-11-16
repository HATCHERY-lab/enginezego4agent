//! Finish tool tasks that a dead process left waiting.
//!
//! The gate board is not read. A waiting `pi.generation` in the store is enough.

use e4a_harness::{Generation, TurnKind, TurnOutcome};
use e4a_llm::Llm;
use e4a_session::Session;
use e4a_types::{TaskQuery, TaskStatus};

use crate::{run_round, waiting_tool_id, Error, ToolOutcome, Workspace};

const GENERATION_KIND: &str = "pi.generation";
const ROUNDS: usize = 8;

/// Run every waiting tool and release its generation.
///
/// No model is called: a waiting tool is workspace work. An empty result
/// means nothing was waiting.
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
        let outcomes = run_round(session, task.id, workspace).await?;
        Generation::bind(task.id, task.conversation_id)
            .release_finished_tool(session)
            .await?;
        finished.extend(outcomes);
    }
    Ok(finished)
}

/// Finish waiting tools, then keep going until each generation answers.
///
/// A model that asks for another tool is run, then asked again. The loop
/// stops on an answer or a restart limit. A new user prompt is the caller's
/// job and starts only after this returns.
pub async fn resume_turns<L: Llm>(
    session: &Session,
    workspace: &Workspace,
    llm: &L,
) -> Result<Vec<TurnOutcome>, Error> {
    let finished = resume_waiting(session, workspace).await?;
    let mut turns = Vec::new();
    let mut seen = Vec::new();
    for tool in finished {
        if seen.contains(&tool.generation_task_id) {
            continue;
        }
        seen.push(tool.generation_task_id);
        let task = session
            .task(tool.generation_task_id)
            .await?
            .ok_or_else(|| {
                Error::message(format!("task {} does not exist", tool.generation_task_id))
            })?;
        let generation = Generation::bind(tool.generation_task_id, task.conversation_id);
        let mut outcome = generation.resume(session, llm).await?;
        let mut round = 0;
        while matches!(outcome.kind, TurnKind::WaitingTool { .. }) {
            round += 1;
            if round > ROUNDS {
                return Err(Error::message(
                    "interrupted turn did not answer",
                ));
            }
            let again = resume_waiting(session, workspace).await?;
            if again.is_empty() {
                return Err(Error::message(
                    "generation is waiting but no tool ran",
                ));
            }
            outcome = generation.resume(session, llm).await?;
        }
        turns.push(outcome);
    }
    Ok(turns)
}
