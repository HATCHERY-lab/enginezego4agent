//! Our fix for pi-durable issue 12098. Not a copy of a single upstream file.
//!
//! A model can send a numeric tool argument as the string `"20"`. The value
//! stored for replay has to be the schema type, or a later resend does not
//! match what the server bound.

use serde_json::{Map, Number, Value};

/// Largest magnitude at which every integer is exact in `f64`.
const MAX_EXACT_INT: f64 = 9_007_199_254_740_992.0;

/// Coerce `value` toward the JSON Schema type in `schema`.
///
/// An object schema walks each property that is present. A numeric or boolean
/// string becomes that type when the text parses. A number or boolean becomes
/// its JSON text when the schema asks for a string. Anything a rule does not
/// cover is returned unchanged, including keys the schema does not name.
pub fn coerce(schema: &Value, value: &Value) -> Value {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => coerce_object(schema, value),
        Some("number") => coerce_numeric(value, false),
        Some("integer") => coerce_numeric(value, true),
        Some("boolean") => coerce_boolean(value),
        Some("string") => coerce_string(value),
        _ => value.clone(),
    }
}

fn coerce_object(schema: &Value, value: &Value) -> Value {
    let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) else {
        return value.clone();
    };
    let mut coerced = Map::new();
    for (key, child) in object {
        let next = match properties.get(key) {
            Some(child_schema) => coerce(child_schema, child),
            None => child.clone(),
        };
        coerced.insert(key.clone(), next);
    }
    Value::Object(coerced)
}

fn coerce_numeric(value: &Value, integer: bool) -> Value {
    let Value::String(text) = value else {
        return value.clone();
    };
    let Some(number) = parse_json_number(text) else {
        return value.clone();
    };
    if integer {
        return match exact_integer(number) {
            Some(whole) => Value::Number(whole),
            None => value.clone(),
        };
    }
    Value::Number(number)
}

fn parse_json_number(text: &str) -> Option<Number> {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Number(number)) => Some(number),
        _ => None,
    }
}

/// Integer tokens pass through. A float token is an integer only when its
/// value is a whole number that `f64` can represent exactly (`"20.0"`, `"2e1"`).
/// A fractional string such as `"20.5"` is rejected.
fn exact_integer(number: Number) -> Option<Number> {
    if number.is_i64() || number.is_u64() {
        return Some(number);
    }
    let float = number.as_f64()?;
    if !float.is_finite() || float.fract() != 0.0 || float.abs() > MAX_EXACT_INT {
        return None;
    }
    Some(Number::from(float as i64))
}

fn coerce_boolean(value: &Value) -> Value {
    match value.as_str() {
        Some("true") => Value::Bool(true),
        Some("false") => Value::Bool(false),
        _ => value.clone(),
    }
}

fn coerce_string(value: &Value) -> Value {
    match value {
        Value::Number(number) => Value::String(number.to_string()),
        Value::Bool(flag) => Value::String(flag.to_string()),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::coerce;
    use serde_json::json;

    #[test]
    fn integer_string_twenty_becomes_number() {
        assert_eq!(coerce(&json!({"type": "integer"}), &json!("20")), json!(20));
    }

    #[test]
    fn string_schema_keeps_numeric_text() {
        assert_eq!(coerce(&json!({"type": "string"}), &json!("20")), json!("20"));
    }

    #[test]
    fn integer_rejects_fractional_string() {
        assert_eq!(
            coerce(&json!({"type": "integer"}), &json!("20.5")),
            json!("20.5")
        );
    }

    #[test]
    fn integer_accepts_whole_float_text() {
        assert_eq!(
            coerce(&json!({"type": "integer"}), &json!("20.0")),
            json!(20)
        );
    }

    #[test]
    fn number_string_becomes_json_number() {
        assert_eq!(coerce(&json!({"type": "number"}), &json!("20")), json!(20));
        assert_eq!(
            coerce(&json!({"type": "number"}), &json!("20.5")),
            json!(20.5)
        );
    }

    #[test]
    fn unparsed_number_stays_text() {
        assert_eq!(
            coerce(&json!({"type": "integer"}), &json!("nope")),
            json!("nope")
        );
        assert_eq!(
            coerce(&json!({"type": "number"}), &json!(true)),
            json!(true)
        );
    }

    #[test]
    fn boolean_strings_become_bools() {
        assert_eq!(coerce(&json!({"type": "boolean"}), &json!("true")), json!(true));
        assert_eq!(
            coerce(&json!({"type": "boolean"}), &json!("false")),
            json!(false)
        );
        assert_eq!(
            coerce(&json!({"type": "boolean"}), &json!("TRUE")),
            json!("TRUE")
        );
    }

    #[test]
    fn number_and_bool_become_json_text() {
        assert_eq!(coerce(&json!({"type": "string"}), &json!(20)), json!("20"));
        assert_eq!(coerce(&json!({"type": "string"}), &json!(true)), json!("true"));
        assert_eq!(
            coerce(&json!({"type": "string"}), &json!(false)),
            json!("false")
        );
    }

    #[test]
    fn object_walks_present_properties_and_keeps_unknown_keys() {
        let schema = json!({
            "type": "object",
            "properties": {
                "timeout": {"type": "number"},
                "command": {"type": "string"},
                "force": {"type": "boolean"},
                "nested": {
                    "type": "object",
                    "properties": {
                        "limit": {"type": "integer"}
                    }
                }
            }
        });
        let value = json!({
            "timeout": "20",
            "command": true,
            "force": "false",
            "nested": {"limit": "20.5", "keep": 1},
            "extra": "stay"
        });
        assert_eq!(
            coerce(&schema, &value),
            json!({
                "timeout": 20,
                "command": "true",
                "force": false,
                "nested": {"limit": "20.5", "keep": 1},
                "extra": "stay"
            })
        );
    }
}
