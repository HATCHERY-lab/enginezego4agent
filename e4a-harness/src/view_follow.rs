// engine4agent view follow. No extra thread.

//! One borrow of the current conversation view, published as a line tail.

use e4a_session::{EntryRecord, Json};

use crate::view_sub::on_lines;

/// Tail of the current view, as [`on_lines`] would publish from an empty previous list.
///
/// `session.watch` is borrowed once. No extra thread is started.
pub async fn follow_lines(
    session: &e4a_session::Session,
    conversation_id: i64,
    limit: usize,
) -> Result<Vec<String>, String> {
    let receiver = session
        .watch(conversation_id)
        .await
        .map_err(|error| error.to_string())?;
    let current = line_strings(&receiver.borrow());
    Ok(on_lines(&[], &current, limit))
}

fn line_strings(view: &e4a_session::ConversationView) -> Vec<String> {
    view.entries.iter().map(entry_line).collect()
}

fn entry_line(entry: &EntryRecord) -> String {
    let text = model_text(entry.model.as_ref());
    if text.is_empty() {
        entry.kind.clone()
    } else {
        format!("{}: {text}", entry.kind)
    }
}

fn model_text(model: Option<&Json>) -> String {
    let Some(message) = model.and_then(Json::as_array).and_then(|items| items.first()) else {
        return String::new();
    };
    match message.get("content") {
        Some(Json::String(text)) => text.clone(),
        Some(Json::Array(parts)) => {
            let mut text = String::new();
            for part in parts {
                if part.get("type").and_then(Json::as_str) == Some("text") {
                    if let Some(piece) = part.get("text").and_then(Json::as_str) {
                        text.push_str(piece);
                    }
                }
            }
            text
        }
        _ => String::new(),
    }
}
