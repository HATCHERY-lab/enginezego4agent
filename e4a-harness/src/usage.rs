// Portions derived from pi-durable 1.5.2 (packages/durable/src/harness/usage.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Conversation spend ledger `pi.usage`. Counters move by a chord op in the response commit.

use e4a_llm::Usage;
use e4a_session::{Checkpoint, DocDef, DocumentFork, DocumentHistory, Error, Tx};
use e4a_types::{apply, Json, Op, Seg};
use serde_json::json;

pub fn usage_doc() -> DocDef {
    DocDef::conversation(
        "pi.usage",
        1,
        DocumentHistory::Latest,
        DocumentFork::Initial,
        || json!({ "models": {}, "tools": {} }),
        Checkpoint::Always,
    )
}

/// Add `usage` to `models["provider/model"]` in the commit that stores the response.
pub fn record_usage(
    tx: &mut Tx,
    conversation_id: i64,
    usage: &Usage,
) -> Result<(), Error> {
    if usage.provider.is_empty() || usage.model.is_empty() {
        return Err(Error::message("usage requires a provider and a model"));
    }
    let doc = tx.doc(&usage_doc(), Some(conversation_id))?;
    let current = tx.doc_value(doc)?.clone();
    let key = format!("{}/{}", usage.provider, usage.model);
    let bucket = merged_bucket(&current, &key, usage);
    let ops = vec![Op::Set(
        vec![Seg::Key("models".into()), Seg::Key(key)],
        bucket,
    )];
    let validated = apply(&current, &ops).map_err(|err| Error::message(err.to_string()))?;
    tx.apply_doc_ops(doc, &ops)?;
    if tx.doc_value(doc)? != &validated {
        return Err(Error::message("usage apply diverged from the committed op"));
    }
    Ok(())
}

fn merged_bucket(current: &Json, key: &str, usage: &Usage) -> Json {
    let previous = current
        .get("models")
        .and_then(|models| models.get(key));
    let input = previous
        .and_then(|bucket| bucket.get("input"))
        .and_then(Json::as_i64)
        .unwrap_or(0)
        .saturating_add(usage.input_tokens);
    let output = previous
        .and_then(|bucket| bucket.get("output"))
        .and_then(Json::as_i64)
        .unwrap_or(0)
        .saturating_add(usage.output_tokens);
    let cost = previous
        .and_then(|bucket| bucket.get("cost"))
        .and_then(Json::as_f64)
        .unwrap_or(0.0)
        + usage.cost_usd;
    json!({
        "input": input,
        "output": output,
        "cost": (cost * 1e6).round() / 1e6
    })
}
