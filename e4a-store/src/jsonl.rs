// Portions derived from pi-durable 1.5.2 (packages/durable/src/storage/jsonl/storage.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! JSONL directory storage (format 1). IndexedDB is not part of this port.
//!
//! `main.jsonl` holds one commit marker per line. Live document and task bodies sit in
//! `doc-{id}.jsonl` and `task-{id}.jsonl`. A writable open drops torn tails and leftover
//! `*.jsonl.reclaim` files; reclamation after that is best-effort.
//!
//! [`JsonlStorage`] is the session backend: it replays confirmed commits into the
//! memory engine and appends sidecar records before each marker.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use e4a_types::{
    CommitResult, ConversationQuery, ConversationRecord, Cursor, DocumentAddress, DocumentContent,
    DocumentCreate, DocumentPoint, DocumentQuery, DocumentRecord, DocumentScope, EntryHit,
    EntryQuery, EntryRecord, Page, StorageWrite, StoredDocument, SubmissionQuery, SubmissionRecord,
    TaskQuery, TaskRecord, TaskStatus,
};
use serde_json::{Map, Value};

use crate::engine::Engine;
use crate::{Error, Storage};

/// On-disk format. Markers and sidecar records carry this `format` number.
pub const FORMAT_VERSION: i64 = 1;

/// Commit log inside the storage directory.
pub const MAIN_FILE: &str = "main.jsonl";

/// Temporary suffix for a sidecar rewrite (`doc-1.jsonl.reclaim`).
pub const RECLAIM_SUFFIX: &str = ".reclaim";

const KIND_DOC: &str = "doc";
const KIND_TASK: &str = "task";

/// Corrupt bytes, a refused read-only commit, or a writer that must be reopened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JsonlError {
    /// The directory or a line breaks the JSONL format. `cause` is the parser or IO detail.
    #[error("{message}")]
    Corruption {
        message: String,
        cause: Option<String>,
    },
    /// [`JsonlWriter::commit`] on a read-only writer. Nothing was written.
    #[error("JSONL storage is open read-only")]
    ReadOnly,
    /// An earlier corruption poisoned this writer. Drop it and open the directory again.
    #[error("JSONL storage is poisoned and must be reopened")]
    Poisoned { cause: String },
}

/// `doc-{id}.jsonl` or `task-{id}.jsonl`. `kind` is only `doc` or `task`, and `id` is `>= 0`.
pub fn sidecar_name(kind: &str, id: i64) -> Result<String, String> {
    if kind != KIND_DOC && kind != KIND_TASK {
        return Err(format!("sidecar kind must be doc or task, got {kind}"));
    }
    if id < 0 {
        return Err(format!("sidecar id must be >= 0, got {id}"));
    }
    Ok(format!("{kind}-{id}.jsonl"))
}

/// `true` when `name` is `doc-{id}.jsonl` or `task-{id}.jsonl` with a canonical non-negative id.
pub fn is_sidecar_name(name: &str) -> bool {
    sidecar_stem(name).is_some()
}

/// `true` when `name` is a sidecar plus [`.reclaim`](RECLAIM_SUFFIX).
pub fn is_reclaim_name(name: &str) -> bool {
    name.strip_suffix(RECLAIM_SUFFIX).is_some_and(is_sidecar_name)
}

/// One JSON value. Empty input and trailing non-whitespace are rejected.
pub fn parse_line(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Err("empty JSONL line".to_string());
    }
    serde_json::from_str(text).map_err(|error| error.to_string())
}

/// Compact JSON plus a trailing newline, the on-disk line encoding.
pub fn append_line(value: &Value) -> String {
    // `serde_json::Value` holds only finite JSON; its serializer does not fail.
    let mut line = serde_json::to_string(value).expect("serde_json::Value serializes to JSON");
    line.push('\n');
    line
}

/// A JSONL directory. `commit` appends one marker line to [`MAIN_FILE`].
#[derive(Debug)]
pub struct JsonlWriter {
    directory: PathBuf,
    read_only: bool,
    poison: Option<JsonlError>,
    last_seq: i64,
    main_line_count: usize,
}

impl JsonlWriter {
    /// Open `directory`. `read_only` does not create the directory, truncate torn lines, or delete reclaim files.
    pub fn open(directory: impl AsRef<Path>, read_only: bool) -> Result<Self, JsonlError> {
        let directory = directory.as_ref();
        if !read_only {
            std::fs::create_dir_all(directory)
                .map_err(|error| io_corruption("directory creation", error))?;
        }
        let main_path = directory.join(MAIN_FILE);
        let main_lines = read_text_lines(&main_path, MAIN_FILE, read_only, parse_main_line)?;
        let last_seq = ensure_increasing_seq(&main_lines)?;
        let names = list_files(directory)?;
        if !read_only {
            remove_reclaim_files(directory, &names)?;
        }
        recover_sidecars(directory, read_only, &main_lines, &names)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            read_only,
            poison: None,
            last_seq,
            main_line_count: main_lines.len(),
        })
    }

    /// Append one validated commit marker to `main.jsonl`.
    ///
    /// Read-only returns [`JsonlError::ReadOnly`] and writes nothing. A format failure returns
    /// [`JsonlError::Corruption`], poisons the writer, and later calls return [`JsonlError::Poisoned`].
    pub fn commit(&mut self, value: &Value) -> Result<(), JsonlError> {
        if let Some(poisoned) = &self.poison {
            return Err(poisoned.clone());
        }
        if self.read_only {
            return Err(JsonlError::ReadOnly);
        }
        let description = format!("{MAIN_FILE} line {}", self.main_line_count + 1);
        let seq = match validate_commit_marker(value, &description) {
            Ok(seq) => seq,
            Err(error) => {
                self.mark_poisoned(&error);
                return Err(error);
            }
        };
        if seq <= self.last_seq {
            let error = corruption(
                format!("Commit sequence does not strictly increase in {MAIN_FILE}"),
                None,
            );
            self.mark_poisoned(&error);
            return Err(error);
        }
        let line = append_line(value);
        let path = self.directory.join(MAIN_FILE);
        if let Err(error) = append_bytes(&path, line.as_bytes(), &format!("append to {MAIN_FILE}"))
        {
            self.mark_poisoned(&error);
            return Err(error);
        }
        self.last_seq = seq;
        self.main_line_count += 1;
        Ok(())
    }

    /// Sidecar bytes first, then one marker per commit. Read-only writes nothing.
    fn append_encoded(&mut self, commits: &[EncodedCommit]) -> Result<(), JsonlError> {
        if let Some(poisoned) = &self.poison {
            return Err(poisoned.clone());
        }
        if self.read_only {
            return Err(JsonlError::ReadOnly);
        }
        for commit in commits {
            for (name, content) in &commit.sidecars {
                let path = self.directory.join(name);
                if let Err(error) =
                    append_bytes(&path, content.as_bytes(), &format!("append to {name}"))
                {
                    self.mark_poisoned(&error);
                    return Err(error);
                }
            }
            self.commit(&commit.marker)?;
        }
        Ok(())
    }

    fn mark_poisoned(&mut self, error: &JsonlError) {
        if self.poison.is_none() {
            self.poison = Some(JsonlError::Poisoned {
                cause: poison_cause(error),
            });
        }
    }
}

fn poison_cause(error: &JsonlError) -> String {
    match error {
        JsonlError::Corruption {
            message,
            cause: Some(cause),
        } => format!("{message}: {cause}"),
        JsonlError::Corruption { message, cause: None } => message.clone(),
        JsonlError::Poisoned { cause } => cause.clone(),
        JsonlError::ReadOnly => error.to_string(),
    }
}

fn corruption(message: impl Into<String>, cause: Option<String>) -> JsonlError {
    JsonlError::Corruption {
        message: message.into(),
        cause,
    }
}

fn io_corruption(action: &str, error: std::io::Error) -> JsonlError {
    corruption(
        format!("JSONL {action} failed: {error}"),
        Some(error.to_string()),
    )
}

fn safe_i64(value: &Value) -> Option<i64> {
    let number = value.as_i64()?;
    if e4a_types::is_safe_integer(number) {
        Some(number)
    } else {
        None
    }
}

fn sidecar_stem(name: &str) -> Option<(&str, &str)> {
    let stem = name.strip_suffix(".jsonl")?;
    let (kind, id) = stem.split_once('-')?;
    if kind != KIND_DOC && kind != KIND_TASK {
        return None;
    }
    if !canonical_id(id) {
        return None;
    }
    Some((kind, id))
}

fn canonical_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    match bytes {
        [b'0'] => true,
        [first, rest @ ..] if (b'1'..=b'9').contains(first) => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// JS `sidecarFileName`: negative ids are not canonical names, but the lookup key still uses them.
fn sidecar_file(kind: &str, id: i64) -> String {
    match sidecar_name(kind, id) {
        Ok(name) => name,
        Err(_) => format!("{kind}-{id}.jsonl"),
    }
}

fn parse_described(text: &str, description: &str) -> Result<Value, JsonlError> {
    parse_line(text).map_err(|cause| corruption(format!("Malformed complete {description}"), Some(cause)))
}

fn validate_commit_marker(value: &Value, description: &str) -> Result<i64, JsonlError> {
    let bad = || corruption(format!("Invalid commit marker in {description}"), None);
    let Some(obj) = value.as_object() else {
        return Err(bad());
    };
    if obj.get("format").and_then(safe_i64) != Some(FORMAT_VERSION)
        || obj.get("type").and_then(Value::as_str) != Some("commit")
        || obj.get("writes").and_then(Value::as_array).is_none()
    {
        return Err(bad());
    }
    let Some(seq) = obj.get("seq").and_then(safe_i64).filter(|seq| *seq >= 1) else {
        return Err(bad());
    };
    let writes = obj.get("writes").and_then(Value::as_array).ok_or_else(bad)?;
    for write in writes {
        validate_main_operation(write, description)?;
    }
    Ok(seq)
}

fn validate_main_operation(value: &Value, description: &str) -> Result<(), JsonlError> {
    let Some(obj) = value.as_object() else {
        return Err(corruption(format!("Invalid write in {description}"), None));
    };
    let Some(kind) = obj.get("type").and_then(Value::as_str) else {
        return Err(corruption(format!("Invalid write in {description}"), None));
    };
    match kind {
        "conversation" | "entry" | "submission" => {
            let id_ok = obj
                .get("value")
                .and_then(Value::as_object)
                .and_then(|record| record.get("id"))
                .and_then(safe_i64)
                .is_some();
            if !id_ok {
                return Err(corruption(
                    format!("Invalid {kind} write in {description}"),
                    None,
                ));
            }
        }
        "task" => {
            if !is_terminal_task(obj.get("value")) {
                return Err(corruption(
                    format!("Invalid terminal task write in {description}"),
                    None,
                ));
            }
        }
        "document.retire" => {
            if obj.get("id").and_then(safe_i64).is_none() {
                return Err(corruption(
                    format!("Invalid document retirement in {description}"),
                    None,
                ));
            }
        }
        "conversation.delete" => {
            if obj.get("id").and_then(safe_i64).is_none() {
                return Err(corruption(
                    format!("Invalid conversation delete in {description}"),
                    None,
                ));
            }
        }
        "task.sidecar" => {
            if !id_and_ordinal(obj) {
                return Err(corruption(
                    format!("Invalid task sidecar write in {description}"),
                    None,
                ));
            }
        }
        "document.create" => {
            let record_ok = obj
                .get("record")
                .and_then(Value::as_object)
                .and_then(|record| record.get("id"))
                .and_then(safe_i64)
                .is_some();
            if !record_ok || obj.get("ordinal").and_then(safe_i64).filter(|n| *n >= 0).is_none() {
                return Err(corruption(
                    format!("Invalid document creation in {description}"),
                    None,
                ));
            }
        }
        "document.change" => {
            if !id_and_ordinal(obj) {
                return Err(corruption(
                    format!("Invalid document change in {description}"),
                    None,
                ));
            }
        }
        _ => {
            return Err(corruption(
                format!("Unknown write type in {description}"),
                None,
            ));
        }
    }
    Ok(())
}

fn id_and_ordinal(obj: &Map<String, Value>) -> bool {
    obj.get("id").and_then(safe_i64).is_some()
        && obj.get("ordinal").and_then(safe_i64).is_some_and(|ordinal| ordinal >= 0)
}

fn is_terminal_task(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(Value::as_object) else {
        return false;
    };
    if value.get("id").and_then(safe_i64).is_none() {
        return false;
    }
    let Some(state) = value.get("state").and_then(Value::as_object) else {
        return false;
    };
    state.get("status").and_then(Value::as_str) == Some("terminal")
}

fn validate_sidecar_record(value: &Value, description: &str) -> Result<(i64, i64), JsonlError> {
    let bad = || corruption(format!("Invalid sidecar record in {description}"), None);
    let Some(obj) = value.as_object() else {
        return Err(bad());
    };
    if obj.get("format").and_then(safe_i64) != Some(FORMAT_VERSION)
        || obj.get("type").and_then(Value::as_str) != Some("record")
    {
        return Err(bad());
    }
    let Some(seq) = obj.get("seq").and_then(safe_i64).filter(|seq| *seq >= 1) else {
        return Err(bad());
    };
    let Some(ordinal) = obj.get("ordinal").and_then(safe_i64).filter(|n| *n >= 0) else {
        return Err(bad());
    };
    let Some(payload) = obj.get("payload").and_then(Value::as_object) else {
        return Err(bad());
    };
    match payload.get("type").and_then(Value::as_str) {
        Some("task") => validate_live_task(payload, description)?,
        Some("document") => {
            if payload.get("id").and_then(safe_i64).is_none() {
                return Err(corruption(
                    format!("Invalid document record in {description}"),
                    None,
                ));
            }
            let content = payload.get("content").ok_or_else(|| {
                corruption(format!("Invalid document content in {description}"), None)
            })?;
            validate_document_content(content, description)?;
        }
        _ => {
            return Err(corruption(
                format!("Unknown sidecar record type in {description}"),
                None,
            ));
        }
    }
    Ok((seq, ordinal))
}

fn validate_live_task(payload: &Map<String, Value>, description: &str) -> Result<(), JsonlError> {
    let bad = || corruption(format!("Invalid live task record in {description}"), None);
    let Some(value) = payload.get("value").and_then(Value::as_object) else {
        return Err(bad());
    };
    if value.get("id").and_then(safe_i64).is_none() {
        return Err(bad());
    }
    let Some(state) = value.get("state").and_then(Value::as_object) else {
        return Err(bad());
    };
    if state.get("status").and_then(Value::as_str) == Some("terminal") {
        return Err(bad());
    }
    Ok(())
}

fn validate_document_content(value: &Value, description: &str) -> Result<(), JsonlError> {
    let bad = || corruption(format!("Invalid document content in {description}"), None);
    let Some(obj) = value.as_object() else {
        return Err(bad());
    };
    let Some(version) = obj.get("version").and_then(safe_i64) else {
        return Err(bad());
    };
    if version < 1 {
        return Err(bad());
    }
    let kind = obj.get("kind").and_then(Value::as_str);
    if kind == Some("base") && obj.get("value").is_some_and(Value::is_object) {
        return Ok(());
    }
    if kind == Some("delta") && obj.get("ops").is_some_and(Value::is_array) {
        return Ok(());
    }
    Err(bad())
}

/// `scope.kind !== "conversation"` or `history === "latest"`. A missing scope is corrupt.
fn is_current_only(record: &Value) -> Result<bool, JsonlError> {
    let Some(scope) = record.get("scope").filter(|scope| scope.is_object()) else {
        return Err(corruption(
            format!("Invalid document record scope in {MAIN_FILE}"),
            None,
        ));
    };
    let conversation = scope.get("kind").and_then(Value::as_str) == Some("conversation");
    let latest = record.get("history").and_then(Value::as_str) == Some("latest");
    Ok(!conversation || latest)
}

struct TextLine {
    value: Value,
    start: u64,
    seq: i64,
    ordinal: i64,
}

struct SidecarFile {
    name: String,
    path: PathBuf,
    lines: Vec<TextLine>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RecKey {
    file: String,
    seq: i64,
    ordinal: i64,
}

#[derive(Clone, Copy)]
struct BasePos {
    seq: i64,
    ordinal: i64,
}

struct MarkerIndex {
    current_only: HashSet<i64>,
    retired: HashSet<i64>,
    task_live: HashMap<i64, bool>,
}

fn parse_main_line(text: &str, line_number: usize) -> Result<(Value, i64, i64), JsonlError> {
    let description = format!("{MAIN_FILE} line {line_number}");
    let value = parse_described(text, &description)?;
    let seq = validate_commit_marker(&value, &description)?;
    Ok((value, seq, 0))
}

fn parse_sidecar_line(
    file: &str,
    text: &str,
    line_number: usize,
) -> Result<(Value, i64, i64), JsonlError> {
    let description = format!("{file} line {line_number}");
    let value = parse_described(text, &description)?;
    let (seq, ordinal) = validate_sidecar_record(&value, &description)?;
    Ok((value, seq, ordinal))
}

fn ensure_increasing_seq(lines: &[TextLine]) -> Result<i64, JsonlError> {
    let mut previous = 0i64;
    for line in lines {
        if line.seq <= previous {
            return Err(corruption(
                format!("Commit sequence does not strictly increase in {MAIN_FILE}"),
                None,
            ));
        }
        previous = line.seq;
    }
    Ok(previous)
}

fn ensure_sidecar_order(file: &str, lines: &[TextLine]) -> Result<(), JsonlError> {
    let mut previous: Option<(i64, i64)> = None;
    for line in lines {
        if let Some((seq, ordinal)) = previous {
            if line.seq < seq || (line.seq == seq && line.ordinal <= ordinal) {
                return Err(corruption(
                    format!("Sidecar records are out of order in {file}"),
                    None,
                ));
            }
        }
        previous = Some((line.seq, line.ordinal));
    }
    Ok(())
}

fn recover_sidecars(
    directory: &Path,
    read_only: bool,
    main_lines: &[TextLine],
    names: &[String],
) -> Result<(), JsonlError> {
    let files = load_sidecars(directory, names, read_only)?;
    let records = index_records(&files);
    let index = classify_markers(main_lines)?;
    let bases = latest_bases(main_lines, &index.current_only, &records)?;
    let confirmed = confirm_markers(main_lines, &index, &bases, &records)?;
    truncate_unconfirmed(read_only, &files, &confirmed)?;
    if !read_only {
        let retired_current_only = retired_current_only(&index);
        let terminal_tasks = terminal_tasks(&index);
        let replacements =
            plan_reclamations(&files, &confirmed, &retired_current_only, &terminal_tasks, &bases)?;
        for (file, content) in replacements {
            replace_sidecar(directory, &file, &content);
        }
    }
    Ok(())
}

fn load_sidecars(
    directory: &Path,
    names: &[String],
    read_only: bool,
) -> Result<Vec<SidecarFile>, JsonlError> {
    let mut sidecar_names: Vec<&String> = names.iter().filter(|name| is_sidecar_name(name)).collect();
    sidecar_names.sort();
    let mut files = Vec::with_capacity(sidecar_names.len());
    for name in sidecar_names {
        let path = directory.join(name);
        let lines = read_text_lines(&path, name, read_only, |text, line_number| {
            parse_sidecar_line(name, text, line_number)
        })?;
        ensure_sidecar_order(name, &lines)?;
        files.push(SidecarFile {
            name: name.clone(),
            path,
            lines,
        });
    }
    Ok(files)
}

fn index_records(files: &[SidecarFile]) -> HashMap<RecKey, Value> {
    let mut records = HashMap::new();
    for file in files {
        for line in &file.lines {
            records.insert(
                RecKey {
                    file: file.name.clone(),
                    seq: line.seq,
                    ordinal: line.ordinal,
                },
                line.value.clone(),
            );
        }
    }
    records
}

fn classify_markers(main_lines: &[TextLine]) -> Result<MarkerIndex, JsonlError> {
    let mut index = MarkerIndex {
        current_only: HashSet::new(),
        retired: HashSet::new(),
        task_live: HashMap::new(),
    };
    for line in main_lines {
        for write in writes_of(&line.value)? {
            match write.get("type").and_then(Value::as_str) {
                Some("document.create") => {
                    let record = write.get("record").ok_or_else(|| {
                        corruption(format!("Invalid document creation in {MAIN_FILE}"), None)
                    })?;
                    let id = record.get("id").and_then(safe_i64).ok_or_else(|| {
                        corruption(format!("Invalid document creation in {MAIN_FILE}"), None)
                    })?;
                    if is_current_only(record)? {
                        index.current_only.insert(id);
                    }
                }
                Some("document.retire") => {
                    let id = field_i64(write, "id", "Invalid document retirement")?;
                    index.retired.insert(id);
                }
                Some("task") => {
                    let id = write
                        .get("value")
                        .and_then(|value| value.get("id"))
                        .and_then(safe_i64)
                        .ok_or_else(|| {
                            corruption(format!("Invalid terminal task write in {MAIN_FILE}"), None)
                        })?;
                    index.task_live.insert(id, false);
                }
                Some("task.sidecar") => {
                    let id = field_i64(write, "id", "Invalid task sidecar write")?;
                    index.task_live.insert(id, true);
                }
                _ => {}
            }
        }
    }
    Ok(index)
}

fn latest_bases(
    main_lines: &[TextLine],
    current_only: &HashSet<i64>,
    records: &HashMap<RecKey, Value>,
) -> Result<HashMap<i64, BasePos>, JsonlError> {
    let mut latest: HashMap<i64, BasePos> = HashMap::new();
    for line in main_lines {
        for write in writes_of(&line.value)? {
            let kind = write.get("type").and_then(Value::as_str);
            let id = match kind {
                Some("document.create") => write
                    .get("record")
                    .and_then(|record| record.get("id"))
                    .and_then(safe_i64),
                Some("document.change") => write.get("id").and_then(safe_i64),
                _ => continue,
            };
            let Some(id) = id else {
                continue;
            };
            if !current_only.contains(&id) {
                continue;
            }
            let Some(ordinal) = write.get("ordinal").and_then(safe_i64) else {
                continue;
            };
            let key = RecKey {
                file: sidecar_file(KIND_DOC, id),
                seq: line.seq,
                ordinal,
            };
            let Some(record) = records.get(&key) else {
                continue;
            };
            if !is_document_base(record, id) {
                continue;
            }
            let pos = BasePos {
                seq: line.seq,
                ordinal,
            };
            let replace = match latest.get(&id) {
                None => true,
                Some(prev) => {
                    pos.seq > prev.seq || (pos.seq == prev.seq && pos.ordinal > prev.ordinal)
                }
            };
            if replace {
                latest.insert(id, pos);
            }
        }
    }
    Ok(latest)
}

fn confirm_markers(
    main_lines: &[TextLine],
    index: &MarkerIndex,
    bases: &HashMap<i64, BasePos>,
    records: &HashMap<RecKey, Value>,
) -> Result<HashSet<RecKey>, JsonlError> {
    let retired_current_only = retired_current_only(index);
    let terminal_tasks = terminal_tasks(index);
    let mut confirmed = HashSet::new();
    for line in main_lines {
        for write in writes_of(&line.value)? {
            match write.get("type").and_then(Value::as_str) {
                Some("task.sidecar") => {
                    let id = field_i64(write, "id", "Invalid task sidecar write")?;
                    let ordinal = field_i64(write, "ordinal", "Invalid task sidecar write")?;
                    let key = RecKey {
                        file: sidecar_file(KIND_TASK, id),
                        seq: line.seq,
                        ordinal,
                    };
                    if let Some(record) = confirm_record(
                        key,
                        line.seq,
                        records,
                        &mut confirmed,
                        terminal_tasks.contains(&id),
                    )? {
                        if task_payload_id(record) != Some(id) {
                            return Err(corruption(
                                format!(
                                    "Confirmed task sidecar data does not match commit {}",
                                    line.seq
                                ),
                                None,
                            ));
                        }
                    }
                }
                Some("document.create") | Some("document.change") => {
                    let kind = write.get("type").and_then(Value::as_str);
                    let id = if kind == Some("document.create") {
                        write
                            .get("record")
                            .and_then(|record| record.get("id"))
                            .and_then(safe_i64)
                            .ok_or_else(|| {
                                corruption(format!("Invalid document creation in {MAIN_FILE}"), None)
                            })?
                    } else {
                        field_i64(write, "id", "Invalid document change")?
                    };
                    let ordinal = field_i64(write, "ordinal", "Invalid document write")?;
                    let reclaimed = retired_current_only.contains(&id)
                        || is_before_base(bases.get(&id).copied(), line.seq, ordinal);
                    let key = RecKey {
                        file: sidecar_file(KIND_DOC, id),
                        seq: line.seq,
                        ordinal,
                    };
                    if let Some(record) = confirm_record(
                        key,
                        line.seq,
                        records,
                        &mut confirmed,
                        reclaimed,
                    )? {
                        if document_payload_id(record) != Some(id) {
                            return Err(corruption(
                                format!(
                                    "Confirmed document sidecar data does not match commit {}",
                                    line.seq
                                ),
                                None,
                            ));
                        }
                        if kind == Some("document.create") && !payload_is_base(record) {
                            return Err(corruption(
                                format!(
                                    "Document creation lacks a confirmed base in commit {}",
                                    line.seq
                                ),
                                None,
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(confirmed)
}

fn confirm_record<'a>(
    key: RecKey,
    marker_seq: i64,
    records: &'a HashMap<RecKey, Value>,
    confirmed: &mut HashSet<RecKey>,
    optional: bool,
) -> Result<Option<&'a Value>, JsonlError> {
    let file = key.file.clone();
    if confirmed.contains(&key) {
        return Err(corruption(
            "Sidecar record is confirmed more than once".to_string(),
            None,
        ));
    }
    match records.get(&key) {
        None if optional => Ok(None),
        None => Err(corruption(
            format!("Missing confirmed sidecar record {file} at sequence {marker_seq}"),
            None,
        )),
        Some(value) => {
            confirmed.insert(key);
            Ok(Some(value))
        }
    }
}

fn truncate_unconfirmed(
    read_only: bool,
    files: &[SidecarFile],
    confirmed: &HashSet<RecKey>,
) -> Result<(), JsonlError> {
    for file in files {
        let mut unconfirmed_at = None;
        for line in &file.lines {
            let key = RecKey {
                file: file.name.clone(),
                seq: line.seq,
                ordinal: line.ordinal,
            };
            if confirmed.contains(&key) {
                if unconfirmed_at.is_some() {
                    return Err(corruption(
                        format!("Confirmed record follows an unconfirmed tail in {}", file.name),
                        None,
                    ));
                }
            } else if unconfirmed_at.is_none() {
                unconfirmed_at = Some(line.start);
            }
        }
        if let Some(at) = unconfirmed_at {
            if !read_only {
                truncate_to(&file.path, at, &format!("tail truncation of {}", file.name))?;
            }
        }
    }
    Ok(())
}

fn plan_reclamations(
    files: &[SidecarFile],
    confirmed: &HashSet<RecKey>,
    retired_current_only: &HashSet<i64>,
    terminal_tasks: &HashSet<i64>,
    bases: &HashMap<i64, BasePos>,
) -> Result<Vec<(String, String)>, JsonlError> {
    let mut replacements = Vec::new();
    for file in files {
        let confirmed_lines: Vec<&TextLine> = file
            .lines
            .iter()
            .filter(|line| {
                confirmed.contains(&RecKey {
                    file: file.name.clone(),
                    seq: line.seq,
                    ordinal: line.ordinal,
                })
            })
            .collect();
        let retained = retained_lines(
            &file.name,
            &confirmed_lines,
            retired_current_only,
            terminal_tasks,
            bases,
        )?;
        let Some(kept) = retained else {
            continue;
        };
        if kept.len() < confirmed_lines.len() || kept.is_empty() {
            let mut content = String::new();
            for line in kept {
                content.push_str(&append_line(&line.value));
            }
            replacements.push((file.name.clone(), content));
        }
    }
    Ok(replacements)
}

fn retained_lines<'a>(
    file: &str,
    confirmed_lines: &[&'a TextLine],
    retired_current_only: &HashSet<i64>,
    terminal_tasks: &HashSet<i64>,
    bases: &HashMap<i64, BasePos>,
) -> Result<Option<Vec<&'a TextLine>>, JsonlError> {
    let Some(id) = sidecar_numeric_id(file) else {
        return Ok(None);
    };
    if file.starts_with("task-") && terminal_tasks.contains(&id) {
        return Ok(Some(Vec::new()));
    }
    if !file.starts_with("doc-") {
        return Ok(None);
    }
    if retired_current_only.contains(&id) {
        return Ok(Some(Vec::new()));
    }
    let Some(base) = bases.get(&id).copied() else {
        return Ok(None);
    };
    let mut kept = Vec::new();
    for line in confirmed_lines {
        if !is_before_base(Some(base), line.seq, line.ordinal) {
            kept.push(*line);
        }
    }
    Ok(Some(kept))
}

fn retired_current_only(index: &MarkerIndex) -> HashSet<i64> {
    index
        .retired
        .iter()
        .copied()
        .filter(|id| index.current_only.contains(id))
        .collect()
}

fn terminal_tasks(index: &MarkerIndex) -> HashSet<i64> {
    index
        .task_live
        .iter()
        .filter(|(_, live)| !**live)
        .map(|(id, _)| *id)
        .collect()
}

fn is_before_base(base: Option<BasePos>, seq: i64, ordinal: i64) -> bool {
    match base {
        Some(base) => seq < base.seq || (seq == base.seq && ordinal < base.ordinal),
        None => false,
    }
}

fn is_document_base(record: &Value, id: i64) -> bool {
    let Some(payload) = record.get("payload") else {
        return false;
    };
    payload.get("type").and_then(Value::as_str) == Some("document")
        && payload.get("id").and_then(safe_i64) == Some(id)
        && payload
            .get("content")
            .and_then(|content| content.get("kind"))
            .and_then(Value::as_str)
            == Some("base")
}

fn payload_is_base(record: &Value) -> bool {
    record
        .get("payload")
        .and_then(|payload| payload.get("content"))
        .and_then(|content| content.get("kind"))
        .and_then(Value::as_str)
        == Some("base")
}

fn task_payload_id(record: &Value) -> Option<i64> {
    let payload = record.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("task") {
        return None;
    }
    payload.get("value")?.get("id").and_then(safe_i64)
}

fn document_payload_id(record: &Value) -> Option<i64> {
    let payload = record.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("document") {
        return None;
    }
    payload.get("id").and_then(safe_i64)
}

fn writes_of(marker: &Value) -> Result<&Vec<Value>, JsonlError> {
    marker
        .get("writes")
        .and_then(Value::as_array)
        .ok_or_else(|| corruption(format!("Invalid commit marker in {MAIN_FILE}"), None))
}

fn field_i64(value: &Value, key: &str, message: &str) -> Result<i64, JsonlError> {
    value
        .get(key)
        .and_then(safe_i64)
        .ok_or_else(|| corruption(format!("{message} in {MAIN_FILE}"), None))
}

fn sidecar_numeric_id(file: &str) -> Option<i64> {
    let ( _kind, id) = sidecar_stem(file)?;
    let id: i64 = id.parse().ok()?;
    if e4a_types::is_safe_integer(id) {
        Some(id)
    } else {
        None
    }
}

fn list_files(directory: &Path) -> Result<Vec<String>, JsonlError> {
    let entries =
        std::fs::read_dir(directory).map_err(|error| io_corruption("directory listing", error))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            // A name can vanish between readdir and the next stat (#11994).
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_corruption("directory listing", error)),
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_corruption("directory listing", error)),
        };
        if !file_type.is_file() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        names.push(name);
    }
    Ok(names)
}

fn remove_reclaim_files(directory: &Path, names: &[String]) -> Result<(), JsonlError> {
    for name in names {
        if !is_reclaim_name(name) {
            continue;
        }
        let path = directory.join(name);
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_corruption(&format!("removal of {name}"), error)),
        }
    }
    Ok(())
}

/// Best-effort, same as pi-durable: a failed rewrite is left for the next open.
fn replace_sidecar(directory: &Path, file: &str, content: &str) {
    let path = directory.join(file);
    if content.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let temp = directory.join(format!("{file}{RECLAIM_SUFFIX}"));
    if std::fs::write(&temp, content.as_bytes()).is_err() {
        return;
    }
    if std::fs::rename(&temp, &path).is_err() {
        let _ = std::fs::remove_file(temp);
    }
}

fn read_text_lines(
    path: &Path,
    name: &str,
    read_only: bool,
    mut parse: impl FnMut(&str, usize) -> Result<(Value, i64, i64), JsonlError>,
) -> Result<Vec<TextLine>, JsonlError> {
    let Some(bytes) = read_binary(path, name)? else {
        return Ok(Vec::new());
    };
    let mut complete = bytes.len();
    let torn = complete > 0 && bytes[complete - 1] != b'\n';
    if torn {
        complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        if !read_only {
            truncate_to(path, complete as u64, &format!("torn-line truncation of {name}"))?;
        }
    }
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut line_number = 1usize;
    for (end, byte) in bytes[..complete].iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let text = std::str::from_utf8(&bytes[start..end]).map_err(|error| {
            corruption(
                format!("Invalid UTF-8 in complete {name} line {line_number}"),
                Some(error.to_string()),
            )
        })?;
        let (value, seq, ordinal) = parse(text, line_number)?;
        lines.push(TextLine {
            value,
            start: start as u64,
            seq,
            ordinal,
        });
        start = end + 1;
        line_number += 1;
    }
    Ok(lines)
}

fn read_binary(path: &Path, name: &str) -> Result<Option<Vec<u8>>, JsonlError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_corruption(&format!("read of {name}"), error)),
    }
}

fn truncate_to(path: &Path, len: u64, action: &str) -> Result<(), JsonlError> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| io_corruption(action, error))?;
    file.set_len(len)
        .map_err(|error| io_corruption(action, error))?;
    Ok(())
}

fn append_bytes(path: &Path, bytes: &[u8], action: &str) -> Result<(), JsonlError> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| io_corruption(action, error))?;
    std::io::Write::write_all(&mut file, bytes).map_err(|error| io_corruption(action, error))?;
    Ok(())
}

impl From<JsonlError> for Error {
    fn from(error: JsonlError) -> Self {
        Error::message(error.to_string())
    }
}

struct EncodedCommit {
    marker: Value,
    sidecars: Vec<(String, String)>,
}

enum ReplayBatch {
    Apply(Vec<StorageWrite>),
    Deletes(Vec<i64>),
}

/// Session store for a JSONL directory.
///
/// A writable open creates the directory, repairs torn tails, and loads confirmed
/// commits. Each stored batch appends its sidecar records, then one marker.
/// Read-only commits are refused. A failure after a batch is applied poisons the handle.
pub struct JsonlStorage {
    engine: RefCell<Engine>,
    writer: RefCell<JsonlWriter>,
    closed: Cell<bool>,
}

impl JsonlStorage {
    /// Open `directory` for writing. Creates it when missing.
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, Error> {
        let directory = directory.as_ref();
        let writer = JsonlWriter::open(directory, false)?;
        let engine = replay_engine(directory)?;
        let expected = writer.last_seq.checked_add(1).ok_or_else(|| {
            Error::message("JSONL commit sequence is exhausted")
        })?;
        if engine.next_seq() != expected {
            return Err(Error::message(
                "JSONL commit sequence does not match stored state",
            ));
        }
        Ok(Self {
            engine: RefCell::new(engine),
            writer: RefCell::new(writer),
            closed: Cell::new(false),
        })
    }

    fn ensure(&self) -> Result<(), Error> {
        if self.closed.get() {
            return Err(Error::message("JsonlStorage is closed"));
        }
        if let Some(poison) = self.writer.borrow().poison.clone() {
            return Err(Error::from(poison));
        }
        Ok(())
    }

    fn read<T>(&self, read: impl FnOnce(&Engine) -> Result<T, Error>) -> Result<T, Error> {
        self.ensure()?;
        read(&self.engine.borrow())
    }
}

impl Storage for JsonlStorage {
    fn commit(
        &self,
        batches: &[Vec<StorageWrite>],
        receive: &mut dyn FnMut(usize, i64),
    ) -> Result<CommitResult, Error> {
        self.ensure()?;
        if self.writer.borrow().read_only {
            return Err(JsonlError::ReadOnly.into());
        }
        let mut encoded = Vec::new();
        let mut seqs = Vec::new();
        let refused = {
            let mut engine = self.engine.borrow_mut();
            let mut refused = None;
            for batch in batches {
                let mut resolved = batch.clone();
                let resolved_ok = resolve_copies_for_jsonl(&engine, &mut resolved).is_ok();
                let outcome = engine.apply_batches(std::slice::from_ref(batch))?;
                if let Some(reason) = outcome.refused {
                    refused = Some(reason);
                    break;
                }
                if !resolved_ok {
                    let error = Error::message("JSONL encode saw a refused batch as stored");
                    drop(engine);
                    self.poison_with(&error);
                    return Err(error);
                }
                let seq = outcome.seqs.first().copied().ok_or_else(|| {
                    Error::message("JSONL commit assigned no sequence")
                })?;
                match encode_commit(seq, &resolved) {
                    Ok(commit) => encoded.push(commit),
                    Err(error) => {
                        drop(engine);
                        self.poison_with(&error);
                        return Err(error);
                    }
                }
                seqs.push(seq);
            }
            refused
        };
        if !encoded.is_empty() {
            self.writer.borrow_mut().append_encoded(&encoded)?;
        }
        for (index, seq) in seqs.iter().enumerate() {
            receive(index, *seq);
        }
        Ok(CommitResult {
            stored: seqs.len(),
            refused,
        })
    }

    fn mint_id(&self) -> Result<i64, Error> {
        self.ensure()?;
        self.engine.borrow_mut().mint_id()
    }

    fn conversation(&self, id: i64) -> Result<Option<ConversationRecord>, Error> {
        self.read(|engine| engine.conversation(id))
    }

    fn scan_conversations(
        &self,
        query: &ConversationQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<ConversationRecord>, Error> {
        self.read(|engine| {
            engine.scan_conversations(
                query.owner_conversation_id,
                query.owner_task_id,
                query.order,
                limit,
                cursor,
            )
        })
    }

    fn entry(&self, id: i64) -> Result<Option<EntryHit>, Error> {
        self.read(|engine| engine.entry(id))
    }

    fn entry_in(&self, conversation_id: i64, id: i64) -> Result<Option<EntryHit>, Error> {
        self.read(|engine| engine.entry_in(conversation_id, id))
    }

    fn find_latest_head_marker(
        &self,
        conversation_id: i64,
        at_or_before: Option<i64>,
    ) -> Result<Option<EntryRecord>, Error> {
        self.read(|engine| engine.find_latest_head_marker(conversation_id, at_or_before))
    }

    fn scan_entries(
        &self,
        query: &EntryQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<EntryRecord>, Error> {
        self.read(|engine| engine.scan_entries(query, limit, cursor))
    }

    fn task(&self, id: i64) -> Result<Option<TaskRecord>, Error> {
        self.read(|engine| engine.task(id))
    }

    fn scan_tasks(
        &self,
        query: &TaskQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<TaskRecord>, Error> {
        self.read(|engine| engine.scan_tasks(query, limit, cursor))
    }

    fn submission(&self, id: i64) -> Result<Option<SubmissionRecord>, Error> {
        self.read(|engine| engine.submission(id))
    }

    fn scan_submissions(
        &self,
        query: &SubmissionQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<SubmissionRecord>, Error> {
        self.read(|engine| engine.scan_submissions(query, limit, cursor))
    }

    fn submission_by_request(
        &self,
        conversation_id: i64,
        request_id: &str,
    ) -> Result<Option<SubmissionRecord>, Error> {
        self.read(|engine| engine.submission_by_request(conversation_id, request_id))
    }

    fn find_document(
        &self,
        address: &DocumentAddress,
        at: DocumentPoint,
    ) -> Result<Option<DocumentRecord>, Error> {
        self.read(|engine| engine.find_document(address, at))
    }

    fn document(&self, id: i64, at: DocumentPoint) -> Result<Option<StoredDocument>, Error> {
        self.read(|engine| engine.document(id, at))
    }

    fn scan_documents(
        &self,
        query: &DocumentQuery,
        limit: usize,
        cursor: Option<&Cursor>,
    ) -> Result<Page<DocumentRecord>, Error> {
        self.read(|engine| engine.scan_documents(query, limit, cursor))
    }

    fn delete_conversation(&self, id: i64) -> Result<(), Error> {
        self.ensure()?;
        if self.writer.borrow().read_only {
            return Err(JsonlError::ReadOnly.into());
        }
        self.engine.borrow().ensure_deletable(id)?;
        let seq = self.engine.borrow().next_seq();
        let marker = serde_json::json!({
            "format": FORMAT_VERSION,
            "type": "commit",
            "seq": seq,
            "writes": [{ "type": "conversation.delete", "id": id }],
        });
        self.writer.borrow_mut().append_encoded(&[EncodedCommit {
            marker,
            sidecars: Vec::new(),
        }])?;
        let mut engine = self.engine.borrow_mut();
        if let Err(error) = engine.delete_conversation(id) {
            drop(engine);
            self.poison_with(&error);
            return Err(error);
        }
        let Some(next_seq) = seq.checked_add(1) else {
            drop(engine);
            let error = Error::message("JSONL commit sequence is exhausted");
            self.poison_with(&error);
            return Err(error);
        };
        let next_id = engine.next_id();
        engine.finish_load(next_id, next_seq);
        Ok(())
    }

    fn close(&self) -> Result<(), Error> {
        self.closed.set(true);
        Ok(())
    }
}

impl JsonlStorage {
    fn poison_with(&self, error: &Error) {
        let cause = JsonlError::Corruption {
            message: error.to_string(),
            cause: None,
        };
        self.writer.borrow_mut().mark_poisoned(&cause);
    }
}

fn encode_commit(seq: i64, writes: &[StorageWrite]) -> Result<EncodedCommit, Error> {
    let mut main_writes = Vec::new();
    let mut records: Vec<(String, Vec<Value>)> = Vec::new();
    let mut next_ordinal = 0i64;
    for write in writes {
        match write {
            StorageWrite::Conversation { .. }
            | StorageWrite::Entry { .. }
            | StorageWrite::Submission { .. }
            | StorageWrite::DocumentRetire { .. } => {
                main_writes.push(serde_json::to_value(write)?);
            }
            StorageWrite::Task { value } => {
                if value.state.status() == TaskStatus::Terminal {
                    main_writes.push(serde_json::to_value(write)?);
                } else {
                    let file = sidecar_name(KIND_TASK, value.id).map_err(Error::message)?;
                    let ordinal = push_sidecar(
                        &mut records,
                        &mut next_ordinal,
                        seq,
                        file,
                        serde_json::json!({ "type": "task", "value": value }),
                    );
                    main_writes.push(serde_json::json!({
                        "type": "task.sidecar",
                        "id": value.id,
                        "ordinal": ordinal,
                    }));
                }
            }
            StorageWrite::DocumentCreate { record, content } => {
                let file = sidecar_name(KIND_DOC, record.id).map_err(Error::message)?;
                let ordinal = push_sidecar(
                    &mut records,
                    &mut next_ordinal,
                    seq,
                    file,
                    serde_json::json!({
                        "type": "document",
                        "id": record.id,
                        "content": content,
                    }),
                );
                main_writes.push(serde_json::json!({
                    "type": "document.create",
                    "record": record,
                    "ordinal": ordinal,
                }));
            }
            StorageWrite::DocumentChange { id, content } => {
                let file = sidecar_name(KIND_DOC, *id).map_err(Error::message)?;
                let ordinal = push_sidecar(
                    &mut records,
                    &mut next_ordinal,
                    seq,
                    file,
                    serde_json::json!({
                        "type": "document",
                        "id": id,
                        "content": content,
                    }),
                );
                main_writes.push(serde_json::json!({
                    "type": "document.change",
                    "id": id,
                    "ordinal": ordinal,
                }));
            }
            StorageWrite::DocumentCopy { .. } => {
                return Err(Error::message(
                    "document copy was not resolved before JSONL encode",
                ));
            }
        }
    }
    let sidecars = records
        .into_iter()
        .map(|(file, lines)| {
            let mut content = String::new();
            for line in lines {
                content.push_str(&append_line(&line));
            }
            (file, content)
        })
        .collect();
    Ok(EncodedCommit {
        marker: serde_json::json!({
            "format": FORMAT_VERSION,
            "type": "commit",
            "seq": seq,
            "writes": main_writes,
        }),
        sidecars,
    })
}

fn push_sidecar(
    records: &mut Vec<(String, Vec<Value>)>,
    next_ordinal: &mut i64,
    seq: i64,
    file: String,
    payload: Value,
) -> i64 {
    let ordinal = *next_ordinal;
    *next_ordinal = next_ordinal.saturating_add(1);
    let record = serde_json::json!({
        "format": FORMAT_VERSION,
        "type": "record",
        "seq": seq,
        "ordinal": ordinal,
        "payload": payload,
    });
    if let Some((_, lines)) = records.iter_mut().find(|(name, _)| name == &file) {
        lines.push(record);
    } else {
        records.push((file, vec![record]));
    }
    ordinal
}

fn resolve_copies_for_jsonl(engine: &Engine, writes: &mut [StorageWrite]) -> Result<(), String> {
    if !writes
        .iter()
        .any(|write| matches!(write, StorageWrite::DocumentCopy { .. }))
    {
        return Ok(());
    }
    let mut changed = HashSet::new();
    for write in writes.iter() {
        match write {
            StorageWrite::DocumentCreate { record, .. }
            | StorageWrite::DocumentCopy { record, .. } => {
                changed.insert(record.id);
            }
            StorageWrite::DocumentChange { id, .. } | StorageWrite::DocumentRetire { id } => {
                changed.insert(*id);
            }
            _ => {}
        }
    }
    for write in writes.iter_mut() {
        let StorageWrite::DocumentCopy { record, source } = write else {
            continue;
        };
        let copy_id = record.id;
        let resolved = (|| -> Result<StorageWrite, String> {
            if changed.contains(&source.id) {
                return Err(format!(
                    "Fork source document {} is changed in the copy batch",
                    source.id
                ));
            }
            let stored = engine
                .document(source.id, source.at)
                .map_err(|error| error.to_string())?;
            let Some(stored) = stored else {
                return Err(format!(
                    "Fork source document {} cannot be read",
                    source.id
                ));
            };
            if !jsonl_copy_matches(&stored.record, record) {
                return Err(format!(
                    "Fork source document {} does not match the copied record",
                    source.id
                ));
            }
            Ok(StorageWrite::DocumentCreate {
                record: record.clone(),
                content: DocumentContent::Base {
                    version: stored.version,
                    value: stored.value,
                },
            })
        })();
        match resolved {
            Ok(create) => *write = create,
            Err(_) => return Err(format!("Document copy {copy_id} was rejected")),
        }
    }
    Ok(())
}

fn jsonl_copy_matches(stored: &DocumentRecord, record: &DocumentCreate) -> bool {
    matches!(stored.scope, DocumentScope::Conversation { .. })
        && matches!(record.scope, DocumentScope::Conversation { .. })
        && stored.kind == record.kind
        && stored.key == record.key
        && stored.history == record.history
        && stored.fork == record.fork
}

fn replay_engine(directory: &Path) -> Result<Engine, JsonlError> {
    let main_path = directory.join(MAIN_FILE);
    let main_lines = read_text_lines(&main_path, MAIN_FILE, true, parse_main_line)?;
    let names = list_files(directory)?;
    let files = load_sidecars(directory, &names, true)?;
    let records = index_records(&files);
    let index = classify_markers(&main_lines)?;
    let bases = latest_bases(&main_lines, &index.current_only, &records)?;
    confirm_markers(&main_lines, &index, &bases, &records)?;
    let mut engine = Engine::new();
    for line in &main_lines {
        match replay_marker(line, &index, &bases, &records)? {
            ReplayBatch::Apply(writes) => apply_replay(&mut engine, line.seq, writes)?,
            ReplayBatch::Deletes(ids) => delete_replay(&mut engine, line.seq, &ids)?,
        }
    }
    Ok(engine)
}

fn apply_replay(engine: &mut Engine, seq: i64, writes: Vec<StorageWrite>) -> Result<(), JsonlError> {
    if engine.next_seq() != seq {
        return Err(corruption(
            format!("Commit sequence does not match stored state in {MAIN_FILE}"),
            None,
        ));
    }
    let outcome = engine.apply_batches(&[writes]).map_err(|error| {
        corruption(
            format!("Invalid committed state at sequence {seq}"),
            Some(error.to_string()),
        )
    })?;
    if outcome.refused.is_some() || outcome.seqs.first().copied() != Some(seq) {
        return Err(corruption(
            format!("Invalid committed state at sequence {seq}"),
            outcome.refused,
        ));
    }
    Ok(())
}

fn delete_replay(engine: &mut Engine, seq: i64, ids: &[i64]) -> Result<(), JsonlError> {
    if engine.next_seq() != seq {
        return Err(corruption(
            format!("Commit sequence does not match stored state in {MAIN_FILE}"),
            None,
        ));
    }
    for id in ids {
        engine.delete_conversation(*id).map_err(|error| {
            corruption(
                format!("Invalid committed state at sequence {seq}"),
                Some(error.to_string()),
            )
        })?;
    }
    let next_id = engine.next_id();
    engine.finish_load(next_id, seq.saturating_add(1));
    Ok(())
}

macro_rules! parse_state {
    ($value:expr, $seq:expr) => {
        serde_json::from_value($value).map_err(|error| {
            corruption(
                format!("Invalid committed state at sequence {}", $seq),
                Some(error.to_string()),
            )
        })
    };
}

fn replay_marker(
    line: &TextLine,
    index: &MarkerIndex,
    bases: &HashMap<i64, BasePos>,
    records: &HashMap<RecKey, Value>,
) -> Result<ReplayBatch, JsonlError> {
    let seq = line.seq;
    let mut writes = Vec::new();
    let mut deletes = Vec::new();
    let retired_current_only = retired_current_only(index);
    let terminal = terminal_tasks(index);
    for write in writes_of(&line.value)? {
        match write.get("type").and_then(Value::as_str) {
            Some("conversation.delete") => {
                deletes.push(field_i64(write, "id", "Invalid conversation delete")?);
            }
            Some("conversation") | Some("entry") | Some("submission") | Some("task")
            | Some("document.retire") => {
                writes.push(parse_state!(write.clone(), seq)?);
            }
            Some("task.sidecar") => {
                let id = field_i64(write, "id", "Invalid task sidecar write")?;
                let ordinal = field_i64(write, "ordinal", "Invalid task sidecar write")?;
                if terminal.contains(&id) {
                    continue;
                }
                let payload = sidecar_payload(records, &sidecar_file(KIND_TASK, id), seq, ordinal)
                    .ok_or_else(|| missing_sidecar(&sidecar_file(KIND_TASK, id), seq))?;
                let value = payload.get("value").cloned().ok_or_else(|| {
                    corruption(format!("Invalid live task record in {MAIN_FILE}"), None)
                })?;
                let value: TaskRecord = parse_state!(value, seq)?;
                writes.push(StorageWrite::Task { value });
            }
            Some("document.create") => {
                let record_value = write.get("record").cloned().ok_or_else(|| {
                    corruption(format!("Invalid document creation in {MAIN_FILE}"), None)
                })?;
                let id = record_value.get("id").and_then(safe_i64).ok_or_else(|| {
                    corruption(format!("Invalid document creation in {MAIN_FILE}"), None)
                })?;
                let ordinal = field_i64(write, "ordinal", "Invalid document write")?;
                let reclaimed = retired_current_only.contains(&id)
                    || is_before_base(bases.get(&id).copied(), seq, ordinal);
                let content = if reclaimed {
                    DocumentContent::Base {
                        version: 1,
                        value: serde_json::json!({}),
                    }
                } else {
                    let payload =
                        sidecar_payload(records, &sidecar_file(KIND_DOC, id), seq, ordinal)
                            .ok_or_else(|| missing_sidecar(&sidecar_file(KIND_DOC, id), seq))?;
                    let content = payload.get("content").cloned().ok_or_else(|| {
                        corruption(format!("Invalid document content in {MAIN_FILE}"), None)
                    })?;
                    let content: DocumentContent = parse_state!(content, seq)?;
                    content
                };
                let record: DocumentCreate = parse_state!(record_value, seq)?;
                writes.push(StorageWrite::DocumentCreate { record, content });
            }
            Some("document.change") => {
                let id = field_i64(write, "id", "Invalid document change")?;
                let ordinal = field_i64(write, "ordinal", "Invalid document write")?;
                let reclaimed = retired_current_only.contains(&id)
                    || is_before_base(bases.get(&id).copied(), seq, ordinal);
                if reclaimed {
                    continue;
                }
                let payload = sidecar_payload(records, &sidecar_file(KIND_DOC, id), seq, ordinal)
                    .ok_or_else(|| missing_sidecar(&sidecar_file(KIND_DOC, id), seq))?;
                let content = payload.get("content").cloned().ok_or_else(|| {
                    corruption(format!("Invalid document content in {MAIN_FILE}"), None)
                })?;
                let content: DocumentContent = parse_state!(content, seq)?;
                writes.push(StorageWrite::DocumentChange { id, content });
            }
            _ => {
                return Err(corruption(
                    format!("Unknown write type in {MAIN_FILE}"),
                    None,
                ));
            }
        }
    }
    if !deletes.is_empty() && !writes.is_empty() {
        return Err(corruption(
            format!("Invalid commit marker in {MAIN_FILE}"),
            None,
        ));
    }
    if deletes.is_empty() {
        Ok(ReplayBatch::Apply(writes))
    } else {
        Ok(ReplayBatch::Deletes(deletes))
    }
}

fn sidecar_payload<'a>(
    records: &'a HashMap<RecKey, Value>,
    file: &str,
    seq: i64,
    ordinal: i64,
) -> Option<&'a Value> {
    records.get(&RecKey {
        file: file.to_string(),
        seq,
        ordinal,
    })
    .and_then(|record| record.get("payload"))
}

fn missing_sidecar(file: &str, seq: i64) -> JsonlError {
    corruption(
        format!("Missing confirmed sidecar record {file} at sequence {seq}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        append_line, is_reclaim_name, is_sidecar_name, parse_line, sidecar_name, RECLAIM_SUFFIX,
    };
    use serde_json::json;

    #[test]
    fn names_match_sidecar_and_reclaim_rules() {
        assert_eq!(sidecar_name("doc", 0).unwrap(), "doc-0.jsonl");
        assert_eq!(sidecar_name("task", 12).unwrap(), "task-12.jsonl");
        assert!(is_sidecar_name(&sidecar_name("doc", 0).unwrap()));
        assert!(is_sidecar_name("task-10.jsonl"));
        assert!(!is_sidecar_name("doc-01.jsonl"));
        assert!(!is_sidecar_name("doc--1.jsonl"));
        assert!(!is_sidecar_name("doc-0.jsonl.reclaim"));
        assert!(!is_sidecar_name("entry-1.jsonl"));
        assert!(!is_sidecar_name("doc-.jsonl"));
        assert!(!is_sidecar_name("doc-1.jsonl.bak"));
        assert!(sidecar_name("entry", 1).is_err());
        assert!(sidecar_name("doc", -1).is_err());
        assert!(sidecar_name("DOC", 1).is_err());

        let reclaim = format!("{}{RECLAIM_SUFFIX}", sidecar_name("task", 7).unwrap());
        assert!(is_reclaim_name(&reclaim));
        assert!(is_reclaim_name("doc-0.jsonl.reclaim"));
        assert!(!is_reclaim_name("doc-0.jsonl"));
        assert!(!is_reclaim_name("doc-01.jsonl.reclaim"));
        assert!(!is_reclaim_name("doc-0.jsonl.reclaim.reclaim"));
        assert!(!is_reclaim_name("other-1.jsonl.reclaim"));
    }

    #[test]
    fn parse_line_accepts_one_value_and_rejects_junk() {
        assert_eq!(parse_line("{\"a\":1}").unwrap(), json!({"a": 1}));
        assert_eq!(parse_line(" null ").unwrap(), json!(null));
        assert_eq!(parse_line("[]").unwrap(), json!([]));
        assert!(parse_line("").is_err());
        assert!(parse_line("   ").is_err());
        assert!(parse_line("\n").is_err());
        assert!(parse_line("1 2").is_err());
        assert!(parse_line("{\"a\":1}{}").is_err());
        assert!(parse_line("true false").is_err());

        let value = json!({"format": 1, "type": "commit"});
        let line = append_line(&value);
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        assert_eq!(parse_line(&line).unwrap(), value);
    }
}
