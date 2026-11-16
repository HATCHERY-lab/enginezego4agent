//! The advisor branch. A second conversation reviews the worker's delta and
//! leaves `pi.advisory` steer records in the main conversation.
//!
//! omp's watchdog shape (specs/omp-study-2026-10-15.md §2, plan §7): the
//! advisor is its own agent, sees only the transcript delta, speaks through
//! one `advise(note, severity)` tool, and an emission guard keeps it quiet
//! when there is nothing to say. The worker never calls the advisor; the
//! harness runs this loop after a turn answers.

use e4a_harness::advisor_agent;
use e4a_llm::{ChatMessage, Llm, LlmEvent, ToolSchema, TurnRequest};
use e4a_session::{Checkpoint, ConversationOwnership, DocDef, DocumentFork, DocumentHistory, EntryDraft, Session, Tx};
use e4a_types::{Json, Op, Seg};
use serde_json::{json, Value};

use crate::{Error, Workspace};

const USER_KIND: &str = "pi.user";
const ASSISTANT_KIND: &str = "pi.assistant";
const TOOL_RESULT_KIND: &str = "pi.toolResult";
const ADVISORY_KIND: &str = "pi.advisory";

const ROUNDS: usize = 3;
const READ_CAP: usize = 2_000;
const DELTA_CAP: usize = 12_000;
const MAX_NOTES: usize = 4;

/// One emitted advisor note. Severity follows omp: `nit`, `concern`, `blocker`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdvisoryNote {
    pub severity: String,
    pub note: String,
}

/// Cursor document `e4a.advisor` on the main conversation: how far the
/// advisor has read, and which conversation is its own transcript.
pub fn advisor_doc() -> DocDef {
    DocDef::conversation(
        "e4a.advisor",
        1,
        DocumentHistory::Latest,
        DocumentFork::Initial,
        || json!({ "cursor": 0, "advisor_conversation": 0 }),
        Checkpoint::Always,
    )
}

/// Review everything the main conversation gained since the last review.
///
/// The advisor model gets the delta as text, one real jailed `read`, and the
/// `advise` tool as its only way to speak. Notes that survive the emission
/// guard are committed as `pi.advisory` entries in the main conversation —
/// the worker sees them on its next turn — and the cursor moves to the last
/// reviewed entry. An empty delta reviews nothing.
pub async fn review<L: Llm>(
    session: &Session,
    llm: &L,
    workspace: &Workspace,
    main: i64,
) -> Result<Vec<AdvisoryNote>, Error> {
    let saved = session
        .snapshot_doc(&advisor_doc(), Some(main))
        .await?
        .unwrap_or_else(|| json!({ "cursor": 0, "advisor_conversation": 0 }));
    let cursor = saved.get("cursor").and_then(Json::as_i64).unwrap_or(0);
    let advisor_conversation = saved
        .get("advisor_conversation")
        .and_then(Json::as_i64)
        .unwrap_or(0);

    let view = session.watch(main).await?;
    let delta = delta_lines(&view.borrow().entries, cursor);
    let last_id = delta.last().map(|(id, _)| *id);
    drop(view);
    let Some(last_id) = last_id else {
        return Ok(Vec::new());
    };
    let delta_text = clip(
        &delta
            .iter()
            .map(|(_, line)| line.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        DELTA_CAP,
    );

    let notes = ask_advisor(llm, workspace, &delta_text).await?;
    let spoken = emission_guard(notes);

    session
        .commit(|tx| {
            let advisor_conversation =
                ensure_advisor_conversation(tx, advisor_conversation, &delta_text)?;
            tx.append_entry(
                advisor_conversation,
                EntryDraft {
                    kind: ASSISTANT_KIND.into(),
                    model: Some(json!([{
                        "role": "assistant",
                        "content": render_notes(&spoken),
                    }])),
                    data: None,
                    head: None,
                    edits: None,
                },
            )?;
            for note in &spoken {
                tx.append_entry(
                    main,
                    EntryDraft {
                        kind: ADVISORY_KIND.into(),
                        model: Some(json!([{
                            "role": "user",
                            "content": advisory_text(note),
                        }])),
                        data: Some(json!({ "severity": note.severity })),
                        head: None,
                        edits: None,
                    },
                )?;
            }
            let doc = tx.doc(&advisor_doc(), Some(main))?;
            let ops = vec![
                Op::Set(vec![Seg::Key("cursor".into())], json!(last_id)),
                Op::Set(
                    vec![Seg::Key("advisor_conversation".into())],
                    json!(advisor_conversation),
                ),
            ];
            tx.apply_doc_ops(doc, &ops)?;
            Ok(())
        })
        .await?;
    Ok(spoken)
}

/// `role: text` rows for the worker kinds, past `cursor`, in order.
fn delta_lines(entries: &[e4a_session::EntryRecord], cursor: i64) -> Vec<(i64, String)> {
    entries
        .iter()
        .filter(|entry| entry.id > cursor)
        .filter_map(|entry| match entry.kind.as_str() {
            USER_KIND => Some((entry.id, format!("user: {}", entry_text(entry)))),
            ASSISTANT_KIND => Some((entry.id, format!("assistant: {}", entry_text(entry)))),
            TOOL_RESULT_KIND => Some((entry.id, format!("tool: {}", entry_text(entry)))),
            _ => None,
        })
        .collect()
}

/// The text of `model[0].content`, or the empty string.
fn entry_text(entry: &e4a_session::EntryRecord) -> String {
    entry
        .model
        .as_ref()
        .and_then(|model| model.as_array())
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(Json::as_str)
        .unwrap_or("")
        .to_string()
}

/// The advisor's own conversation, created on the first review. Its request
/// side is recorded so the review reads back from the journal.
fn ensure_advisor_conversation(
    tx: &mut Tx,
    saved: i64,
    delta: &str,
) -> Result<i64, e4a_session::Error> {
    if saved > 0 {
        return Ok(saved);
    }
    let conversation = tx.create_conversation(ConversationOwnership::Ownerless)?;
    tx.append_entry(
        conversation.id,
        EntryDraft {
            kind: USER_KIND.into(),
            model: Some(json!([{
                "role": "user",
                "content": delta,
            }])),
            data: None,
            head: None,
            edits: None,
        },
    )?;
    Ok(conversation.id)
}

/// One advisor pass: the advisor system prompt, the delta, a real jailed
/// `read`, and `advise` as the only other tool. At most [`ROUNDS`] model
/// calls; a model that never advises says nothing.
async fn ask_advisor<L: Llm>(
    llm: &L,
    workspace: &Workspace,
    delta: &str,
) -> Result<Vec<AdvisoryNote>, Error> {
    let role = advisor_agent();
    let mut messages = vec![
        ChatMessage {
            role: "system".into(),
            text: format!(
                "{} Review the worker transcript delta below. Call advise(note, severity) \
with severity nit, concern, or blocker for every real problem you see in what the \
worker did. No praise, no summaries, no restating the work. If nothing is wrong, \
call advise with a one-word note \"none\". You may read one file first.",
                role.instructions
            ),
            tool_calls: Vec::new(),
            tool_call_id: None,
            is_error: false,
        },
        ChatMessage {
            role: "user".into(),
            text: delta.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            is_error: false,
        },
    ];
    let tools = vec![read_schema(), advise_schema()];
    let mut notes = Vec::new();
    for _ in 0..ROUNDS {
        let request = TurnRequest {
            messages: messages.clone(),
            tools: tools.clone(),
        };
        let mut stream = llm
            .stream(&request)
            .await
            .map_err(|error| Error::message(error.to_string()))?;
        let mut text = String::new();
        let mut call = None;
        while let Some(event) = stream.next().await {
            match event {
                LlmEvent::Text(chunk) => text.push_str(&chunk),
                LlmEvent::ToolCall(next) => call = Some(next),
                LlmEvent::Usage(_) => {}
            }
        }
        let Some(call) = call else {
            return Ok(notes);
        };
        match call.name.as_str() {
            "advise" => {
                if let Some(note) = parse_advise(&call.arguments) {
                    notes.push(note);
                }
                return Ok(notes);
            }
            "read" => {
                let path = call
                    .arguments
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let body = match workspace.read(path) {
                    Ok(text) => clip(&text, READ_CAP),
                    Err(error) => error.to_string(),
                };
                let call_id = call.id.clone();
                messages.push(ChatMessage {
                    role: "assistant".into(),
                    text: String::new(),
                    tool_calls: vec![call],
                    tool_call_id: None,
                    is_error: false,
                });
                messages.push(ChatMessage {
                    role: "tool".into(),
                    text: body,
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call_id),
                    is_error: false,
                });
            }
            _ => return Ok(notes),
        }
    }
    Ok(notes)
}

fn read_schema() -> ToolSchema {
    ToolSchema {
        name: "read".into(),
        description: "Read one UTF-8 file under the workspace root. One path only.".into(),
        parameters: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }
}

fn advise_schema() -> ToolSchema {
    ToolSchema {
        name: "advise".into(),
        description: "Emit one note. severity is nit, concern, or blocker.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "note": { "type": "string" },
                "severity": { "type": "string", "enum": ["nit", "concern", "blocker"] }
            },
            "required": ["note", "severity"]
        }),
    }
}

/// `advise` arguments to a note. An unknown severity reads as `nit`; a note
/// without text is nothing.
fn parse_advise(arguments: &Value) -> Option<AdvisoryNote> {
    let note = arguments
        .get("note")
        .and_then(Value::as_str)?
        .trim()
        .to_string();
    if note.is_empty() {
        return None;
    }
    let severity = match arguments.get("severity").and_then(Value::as_str) {
        Some("concern") => "concern",
        Some("blocker") => "blocker",
        _ => "nit",
    };
    Some(AdvisoryNote {
        severity: severity.into(),
        note,
    })
}

/// omp emission guard, first cut: no empty praise, no "none", no repeats, at
/// most [`MAX_NOTES`] notes per update.
fn emission_guard(notes: Vec<AdvisoryNote>) -> Vec<AdvisoryNote> {
    let mut seen = std::collections::BTreeSet::new();
    let mut kept = Vec::new();
    for note in notes {
        let flat = note.note.to_ascii_lowercase();
        if flat == "none" || flat == "lgtm" || flat == "done" || flat == "ok" {
            continue;
        }
        if !seen.insert(flat) {
            continue;
        }
        kept.push(note);
        if kept.len() == MAX_NOTES {
            break;
        }
    }
    kept
}

fn advisory_text(note: &AdvisoryNote) -> String {
    format!(
        "<advisory advisor=\"advisor\" severity=\"{}\" guidance=\"weigh, don't blindly obey\">{}</advisory>",
        note.severity, note.note
    )
}

fn render_notes(notes: &[AdvisoryNote]) -> String {
    if notes.is_empty() {
        return "no notes".into();
    }
    notes
        .iter()
        .map(|note| format!("{}: {}", note.severity, note.note))
        .collect::<Vec<_>>()
        .join("\n")
}

fn clip(text: &str, cap: usize) -> String {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guard_drops_praise_and_repeats_and_caps_the_count() {
        let notes = vec![
            AdvisoryNote { severity: "nit".into(), note: "none".into() },
            AdvisoryNote { severity: "nit".into(), note: "LGTM".into() },
            AdvisoryNote { severity: "concern".into(), note: "note.txt has no trailing newline".into() },
            AdvisoryNote { severity: "nit".into(), note: "Note.txt has no trailing newline".into() },
            AdvisoryNote { severity: "blocker".into(), note: "the loop never ends".into() },
            AdvisoryNote { severity: "nit".into(), note: "third real note".into() },
            AdvisoryNote { severity: "nit".into(), note: "fourth real note".into() },
            AdvisoryNote { severity: "nit".into(), note: "fifth real note".into() },
        ];
        let kept = emission_guard(notes);
        assert_eq!(kept.len(), 4);
        assert_eq!(kept[0].note, "note.txt has no trailing newline");
        assert_eq!(kept[1].note, "the loop never ends");
        assert_eq!(kept[3].note, "fourth real note");
    }

    #[test]
    fn advise_arguments_become_notes() {
        assert_eq!(
            parse_advise(&json!({"note": "  fix the loop ", "severity": "concern"})),
            Some(AdvisoryNote { severity: "concern".into(), note: "fix the loop".into() })
        );
        assert_eq!(
            parse_advise(&json!({"note": "x", "severity": "weird"})),
            Some(AdvisoryNote { severity: "nit".into(), note: "x".into() })
        );
        assert_eq!(parse_advise(&json!({"note": "", "severity": "nit"})), None);
    }

    #[test]
    fn an_advisory_entry_reads_as_a_steer_record() {
        let note = AdvisoryNote { severity: "concern".into(), note: "the loop never ends".into() };
        let text = advisory_text(&note);
        assert!(text.starts_with("<advisory advisor=\"advisor\" severity=\"concern\""));
        assert!(text.contains("weigh, don't blindly obey"));
        assert!(text.ends_with("the loop never ends</advisory>"));
    }
}
