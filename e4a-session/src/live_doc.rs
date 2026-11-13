// engine4agent pi.live document.

//! `pi.live` value for one run. No store write.

use serde_json::{json, Value};

/// `{ kind: "pi.live", taskId, slots: [{taskId, name, finished}], ended }`.
///
/// Slot order is the input order. `finished` is the bool. An empty `name` is
/// kept. `ended` is null when the run is still open. This does not write the
/// store. The caller commits it.
pub fn live_document(task_id: i64, slots: &[(i64, String, bool)], ended: Option<&str>) -> Value {
    let slots = slots
        .iter()
        .map(|(slot_task_id, name, finished)| {
            json!({
                "taskId": slot_task_id,
                "name": name,
                "finished": finished,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "pi.live",
        "taskId": task_id,
        "slots": slots,
        "ended": ended,
    })
}

#[cfg(test)]
mod tests {
    use super::live_document;
    use serde_json::json;

    #[test]
    fn maps_slots_and_keeps_an_empty_name() {
        let slots = [
            (10, "read".to_owned(), false),
            (11, String::new(), true),
        ];
        let doc = live_document(7, &slots, Some("faulted"));
        assert_eq!(
            doc,
            json!({
                "kind": "pi.live",
                "taskId": 7,
                "slots": [
                    { "taskId": 10, "name": "read", "finished": false },
                    { "taskId": 11, "name": "", "finished": true },
                ],
                "ended": "faulted",
            })
        );
    }

    #[test]
    fn open_run_has_null_ended_and_may_have_no_slots() {
        let doc = live_document(1, &[], None);
        assert_eq!(
            doc,
            json!({
                "kind": "pi.live",
                "taskId": 1,
                "slots": [],
                "ended": null,
            })
        );
    }
}
