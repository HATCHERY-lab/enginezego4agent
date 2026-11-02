// Portions derived from @earendil-works/chord 1.5.0 (packages/chord/src/delta), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Chord `apply` for decoded document ops. Paths stay inside the value: reserved segments are rejected.
//!
//! Chord `Op` (`set`, `splice`, and the rest) is not RFC 6902, so this module
//! does not use the `json-patch` crate.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Number, Value};

use crate::Json;

const RESERVED: [&str; 3] = ["__proto__", "constructor", "prototype"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    Index(i64),
}

pub type Path = Vec<Seg>;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Replace(Json),
    Set(Path, Json),
    Delete(Path),
    Append(Path, String),
    Truncate(Path, i64),
    Splice {
        path: Path,
        index: i64,
        remove: i64,
        items: Vec<Json>,
    },
    Move {
        path: Path,
        permutation: Vec<i64>,
    },
}

#[derive(Debug)]
pub struct ApplyError(pub String);

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApplyError {}

/// Apply decoded ops. `r` replaces the root. The input value is not mutated.
pub fn apply(target: &Json, ops: &[Op]) -> Result<Json, ApplyError> {
    let mut root = target.clone();
    for op in ops {
        root = apply_one(root, op)?;
    }
    Ok(root)
}

fn apply_one(root: Json, op: &Op) -> Result<Json, ApplyError> {
    match op {
        Op::Replace(value) => Ok(value.clone()),
        Op::Set(path, value) => {
            require_nonempty(path)?;
            assert_safe(path)?;
            let key = path.last().expect("nonempty");
            let parent = &path[..path.len() - 1];
            descend(root, parent, path, |current| {
                write_key(current, key, value.clone(), path)
            })
        }
        Op::Delete(path) => {
            require_nonempty(path)?;
            assert_safe(path)?;
            let key = path.last().expect("nonempty");
            let parent = &path[..path.len() - 1];
            descend(root, parent, path, |current| delete_key(current, key, path))
        }
        Op::Append(path, text) => {
            update_string(root, path, |current| Ok(format!("{current}{text}")))
        }
        Op::Truncate(path, start) => {
            update_string(root, path, |current| Ok(js_slice(current, *start)))
        }
        Op::Splice {
            path,
            index,
            remove,
            items,
        } => {
            assert_safe(path)?;
            descend(root, path, path, |current| {
                let Value::Array(mut items_target) = current else {
                    return Err(ApplyError(unresolvable(path)));
                };
                js_splice(&mut items_target, *index, *remove, items);
                Ok(Value::Array(items_target))
            })
        }
        Op::Move { path, permutation } => {
            assert_safe(path)?;
            descend(root, path, path, |current| {
                let Value::Array(items) = current else {
                    return Err(ApplyError(unresolvable(path)));
                };
                if items.len() != permutation.len() {
                    return Err(ApplyError(unresolvable(path)));
                }
                let mut next = Vec::with_capacity(items.len());
                for slot in permutation {
                    if *slot < 0 || *slot as usize >= items.len() {
                        return Err(ApplyError(unresolvable(path)));
                    }
                    next.push(items[*slot as usize].clone());
                }
                Ok(Value::Array(next))
            })
        }
    }
}

fn update_string(
    root: Json,
    path: &Path,
    edit: impl FnOnce(&str) -> Result<String, ApplyError>,
) -> Result<Json, ApplyError> {
    require_nonempty(path)?;
    assert_safe(path)?;
    let key = path.last().expect("nonempty");
    let parent = &path[..path.len() - 1];
    descend(root, parent, path, |current| {
        let text = read_string(&current, key, path)?;
        let updated = edit(&text)?;
        write_key(current, key, Value::String(updated), path)
    })
}

fn require_nonempty(path: &Path) -> Result<(), ApplyError> {
    if path.is_empty() {
        Err(ApplyError(unresolvable(path)))
    } else {
        Ok(())
    }
}

fn assert_safe(path: &Path) -> Result<(), ApplyError> {
    for seg in path {
        match seg {
            Seg::Key(key) if RESERVED.iter().any(|reserved| reserved == key) => {
                return Err(ApplyError(format!("unsafe path segment: {key}")));
            }
            Seg::Index(index) if *index < 0 => {
                return Err(ApplyError(format!("unsafe path segment: {index}")));
            }
            _ => {}
        }
    }
    Ok(())
}

fn descend(
    current: Json,
    path: &[Seg],
    full: &[Seg],
    write: impl FnOnce(Json) -> Result<Json, ApplyError>,
) -> Result<Json, ApplyError> {
    if path.is_empty() {
        return write(current);
    }
    let (seg, rest) = path.split_first().expect("nonempty");
    match (current, seg) {
        (Value::Object(mut map), Seg::Key(key)) => {
            let child = map
                .remove(key)
                .ok_or_else(|| ApplyError(unresolvable(full)))?;
            let child = descend(child, rest, full, write)?;
            map.insert(key.clone(), child);
            Ok(Value::Object(map))
        }
        (Value::Object(mut map), Seg::Index(index)) => {
            let key = index.to_string();
            let child = map
                .remove(&key)
                .ok_or_else(|| ApplyError(unresolvable(full)))?;
            let child = descend(child, rest, full, write)?;
            map.insert(key, child);
            Ok(Value::Object(map))
        }
        (Value::Array(mut items), Seg::Index(index)) => {
            let idx = *index as usize;
            if idx >= items.len() {
                return Err(ApplyError(unresolvable(full)));
            }
            let child = items[idx].take();
            items[idx] = descend(child, rest, full, write)?;
            Ok(Value::Array(items))
        }
        (Value::Array(_), Seg::Key(key)) => Err(ApplyError(format!("unsafe path segment: {key}"))),
        _ => Err(ApplyError(unresolvable(full))),
    }
}

fn write_key(current: Json, key: &Seg, value: Json, path: &[Seg]) -> Result<Json, ApplyError> {
    match (current, key) {
        (Value::Object(mut map), Seg::Key(name)) => {
            map.insert(name.clone(), value);
            Ok(Value::Object(map))
        }
        (Value::Object(mut map), Seg::Index(index)) => {
            map.insert(index.to_string(), value);
            Ok(Value::Object(map))
        }
        (Value::Array(mut items), Seg::Index(index)) => {
            if *index > items.len() as i64 {
                return Err(ApplyError(format!("unsafe path segment: {index}")));
            }
            if *index < 0 {
                return Err(ApplyError(format!("unsafe path segment: {index}")));
            }
            let idx = *index as usize;
            if idx == items.len() {
                items.push(value);
            } else {
                items[idx] = value;
            }
            Ok(Value::Array(items))
        }
        (Value::Array(_), Seg::Key(name)) => {
            Err(ApplyError(format!("unsafe path segment: {name}")))
        }
        _ => Err(ApplyError(unresolvable(path))),
    }
}

fn delete_key(current: Json, key: &Seg, path: &[Seg]) -> Result<Json, ApplyError> {
    match (current, key) {
        (Value::Object(mut map), Seg::Key(name)) => {
            map.remove(name);
            Ok(Value::Object(map))
        }
        (Value::Object(mut map), Seg::Index(index)) => {
            map.remove(&index.to_string());
            Ok(Value::Object(map))
        }
        (Value::Array(mut items), Seg::Index(index)) => {
            if *index > items.len() as i64 {
                return Err(ApplyError(format!("unsafe path segment: {index}")));
            }
            if *index < 0 || *index >= items.len() as i64 {
                return Err(ApplyError(unresolvable(path)));
            }
            items.remove(*index as usize);
            Ok(Value::Array(items))
        }
        (Value::Array(_), Seg::Key(name)) => {
            Err(ApplyError(format!("unsafe path segment: {name}")))
        }
        _ => Err(ApplyError(unresolvable(path))),
    }
}

fn read_string(current: &Json, key: &Seg, path: &[Seg]) -> Result<String, ApplyError> {
    let value = match (current, key) {
        (Value::Object(map), Seg::Key(name)) => map.get(name),
        (Value::Object(map), Seg::Index(index)) => map.get(&index.to_string()),
        (Value::Array(items), Seg::Index(index)) => {
            if *index > items.len() as i64 {
                return Err(ApplyError(format!("unsafe path segment: {index}")));
            }
            if *index < 0 || *index >= items.len() as i64 {
                return Err(ApplyError(unresolvable(path)));
            }
            items.get(*index as usize)
        }
        (Value::Array(_), Seg::Key(name)) => {
            return Err(ApplyError(format!("unsafe path segment: {name}")));
        }
        _ => return Err(ApplyError(unresolvable(path))),
    };
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(ApplyError(unresolvable(path))),
    }
}

fn js_splice(items: &mut Vec<Json>, start: i64, remove: i64, insert: &[Json]) {
    let len = items.len() as i64;
    let start = if start < 0 {
        (len + start).max(0)
    } else {
        start.min(len)
    } as usize;
    let remove = if remove < 0 {
        0
    } else {
        remove.min(len - start as i64).max(0) as usize
    };
    items.drain(start..start + remove);
    for (offset, item) in insert.iter().enumerate() {
        items.insert(start + offset, item.clone());
    }
}

fn js_slice(text: &str, start: i64) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let len = units.len() as i64;
    let start = if start < 0 {
        (len + start).max(0)
    } else {
        start.min(len)
    } as usize;
    String::from_utf16_lossy(&units[start..])
}

fn unresolvable(path: &[Seg]) -> String {
    let encoded = Value::Array(path.iter().map(seg_json).collect());
    format!("unresolvable path: {encoded}")
}

fn seg_json(seg: &Seg) -> Json {
    match seg {
        Seg::Key(key) => Value::String(key.clone()),
        Seg::Index(index) => Value::Number(Number::from(*index)),
    }
}

impl Op {
    fn to_json(&self) -> Json {
        match self {
            Op::Replace(value) => Value::Array(vec![Value::String("r".into()), value.clone()]),
            Op::Set(path, value) => Value::Array(vec![
                Value::String("s".into()),
                Value::Array(path.iter().map(seg_json).collect()),
                value.clone(),
            ]),
            Op::Delete(path) => Value::Array(vec![
                Value::String("d".into()),
                Value::Array(path.iter().map(seg_json).collect()),
            ]),
            Op::Append(path, text) => Value::Array(vec![
                Value::String("a".into()),
                Value::Array(path.iter().map(seg_json).collect()),
                Value::String(text.clone()),
            ]),
            Op::Truncate(path, count) => Value::Array(vec![
                Value::String("t".into()),
                Value::Array(path.iter().map(seg_json).collect()),
                Value::Number(Number::from(*count)),
            ]),
            Op::Splice {
                path,
                index,
                remove,
                items,
            } => Value::Array(vec![
                Value::String("p".into()),
                Value::Array(path.iter().map(seg_json).collect()),
                Value::Number(Number::from(*index)),
                Value::Number(Number::from(*remove)),
                Value::Array(items.clone()),
            ]),
            Op::Move { path, permutation } => Value::Array(vec![
                Value::String("m".into()),
                Value::Array(path.iter().map(seg_json).collect()),
                Value::Array(
                    permutation
                        .iter()
                        .map(|index| Value::Number(Number::from(*index)))
                        .collect(),
                ),
            ]),
        }
    }
}

impl Serialize for Op {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Op {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Json::deserialize(deserializer)?;
        op_from_value(&value).map_err(de::Error::custom)
    }
}

fn op_from_value(value: &Json) -> Result<Op, String> {
    let items = value.as_array().ok_or("op must be an array")?;
    let verb = items.first().and_then(Value::as_str).ok_or("op verb")?;
    match verb {
        "r" if items.len() == 2 => Ok(Op::Replace(items[1].clone())),
        "s" if items.len() == 3 => Ok(Op::Set(path_from(&items[1])?, items[2].clone())),
        "d" if items.len() == 2 => Ok(Op::Delete(path_from(&items[1])?)),
        "a" if items.len() == 3 => Ok(Op::Append(
            path_from(&items[1])?,
            items[2].as_str().ok_or("append text")?.to_string(),
        )),
        "t" if items.len() == 3 => Ok(Op::Truncate(path_from(&items[1])?, number_i64(&items[2])?)),
        "p" if items.len() == 5 => Ok(Op::Splice {
            path: path_from(&items[1])?,
            index: number_i64(&items[2])?,
            remove: number_i64(&items[3])?,
            items: items[4].as_array().cloned().ok_or("splice items")?,
        }),
        "m" if items.len() == 3 => Ok(Op::Move {
            path: path_from(&items[1])?,
            permutation: items[2]
                .as_array()
                .ok_or("permutation")?
                .iter()
                .map(number_i64)
                .collect::<Result<Vec<_>, _>>()?,
        }),
        _ => Err(format!("unknown op verb: {verb}")),
    }
}

fn path_from(value: &Json) -> Result<Path, String> {
    let items = value.as_array().ok_or("path")?;
    items.iter().map(seg_from).collect()
}

fn seg_from(value: &Json) -> Result<Seg, String> {
    match value {
        Value::String(key) => Ok(Seg::Key(key.clone())),
        Value::Number(number) => Ok(Seg::Index(number_i64(&Value::Number(number.clone()))?)),
        _ => Err("path segment".into()),
    }
}

fn number_i64(value: &Json) -> Result<i64, String> {
    value.as_i64().ok_or_else(|| "expected integer".into())
}
