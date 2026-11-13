// Portions derived from pi-durable 1.5.2, MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Shallow JSON object merge.
//!
//! Upstream `assignJson` writes a document leaf by leaf and deletes missing keys.
//! This port overlays one object onto another and does not recurse.

use serde_json::Value;

/// Overlay `patch` onto `base` when both are objects.
///
/// Keys in `patch` replace the same key in `base`. Keys only in `base` stay.
/// Nested objects are replaced, not merged. Any non-object returns `patch` cloned.
pub fn merge_object(base: &Value, patch: &Value) -> Value {
    let (Some(base_map), Some(patch_map)) = (base.as_object(), patch.as_object()) else {
        return patch.clone();
    };
    let mut merged = base_map.clone();
    for (key, value) in patch_map {
        merged.insert(key.clone(), value.clone());
    }
    Value::Object(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn shallow_merge_overlays_object_keys() {
        let base = json!({"a": 1, "b": {"x": 1}, "keep": true});
        let patch = json!({"b": {"y": 2}, "c": 3});
        assert_eq!(
            merge_object(&base, &patch),
            json!({"a": 1, "b": {"y": 2}, "keep": true, "c": 3})
        );
    }

    #[test]
    fn non_objects_return_the_patch() {
        assert_eq!(merge_object(&json!(1), &json!("x")), json!("x"));
        assert_eq!(merge_object(&json!({"a": 1}), &json!(null)), json!(null));
        assert_eq!(merge_object(&json!([1]), &json!({"a": 1})), json!({"a": 1}));
        assert_eq!(merge_object(&json!({}), &json!({})), json!({}));
    }
}
