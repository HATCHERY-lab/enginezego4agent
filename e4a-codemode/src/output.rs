// Portions derived from pi-durable 1.5.2 (packages/durable/src/codemode/output.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! Parse the text a code-mode tool result shows the model.
//!
//! The formatter writes `Script completed` or `Script failed`, a one-decimal
//! wall time, then `Output:`. A failure adds `Script error:` and one of
//! `script`, `timeout`, `aborted`, or `sandbox`.

use crate::protocol::ErrorKind;

/// Default token budget for script output.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 10_000;

/// Lines of the result besides the script's text: the header, markers, notes.
pub const LINE_MARGIN: usize = 100;

/// Characters per token when estimating.
const CHARS_PER_TOKEN: u32 = 4;

const NO_CALLS: &str = "No tool calls were made.";
const CALLS_PREFIX: &str = "Tool calls made before the failure (they are not undone): ";

/// A successful script, as the model reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    /// Milliseconds, from the `Wall time N.N seconds` line (one decimal).
    pub wall_ms: u64,
    /// Body after `Output:`, including markers.
    pub output: String,
    /// Text items. Several items keep the `==> text N/M <==` split.
    pub texts: Vec<String>,
    pub console: Option<String>,
    pub full_output_path: Option<String>,
    /// Images `models.generateImages()` returned that the script did not show.
    pub unshown_images: Option<u64>,
}

/// A failed script. `kind` is the name `output.ts` uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputFailure {
    pub kind: ErrorKind,
    pub message: String,
    pub calls: Vec<CallNote>,
    pub wall_ms: u64,
    /// Script output before `Script error:`.
    pub output: String,
    pub texts: Vec<String>,
    pub console: Option<String>,
    pub full_output_path: Option<String>,
    pub unshown_images: Option<u64>,
}

/// One tool call named in the failure summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallNote {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputError {
    Failure(OutputFailure),
    Unrecognized,
}

impl OutputError {
    /// `script`, `timeout`, `aborted`, `sandbox`, or `unrecognized`.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Failure(failure) => failure.kind.as_str(),
            Self::Unrecognized => "unrecognized",
        }
    }
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for OutputError {}

/// File extension `image()` uses for a MIME type. Unknown types use `.bin`.
pub fn image_extension(mime: &str) -> &'static str {
    match mime {
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        _ => ".bin",
    }
}

/// `Math.ceil(utf16Length / 4)`, the file's token estimate.
pub fn token_estimate(text: &str) -> u64 {
    let len = u64::try_from(text.encode_utf16().count()).unwrap_or(u64::MAX);
    len.div_ceil(u64::from(CHARS_PER_TOKEN))
}

/// Lines kept at each end when output is cut.
///
/// The file uses `floor((maxLines - LINE_MARGIN) / 2)`. A `max_lines` below
/// [`LINE_MARGIN`] yields `0` instead of a negative count.
pub fn line_budget(max_lines: usize) -> usize {
    max_lines.saturating_sub(LINE_MARGIN) / 2
}

/// Parse one formatted tool result.
///
/// Success is [`ToolResult`]. A `Script failed` blob is [`OutputError::Failure`]
/// whose reason is `script`, `timeout`, `aborted`, or `sandbox`. Any other
/// shape is [`OutputError::Unrecognized`].
pub fn parse_output(blob: &str) -> Result<ToolResult, OutputError> {
    let Some((status, rest)) = split_line(blob) else {
        return Err(OutputError::Unrecognized);
    };
    let ok = match status {
        "Script completed" => true,
        "Script failed" => false,
        _ => return Err(OutputError::Unrecognized),
    };
    let Some((wall_line, rest)) = split_line(rest) else {
        return Err(OutputError::Unrecognized);
    };
    let Some(wall_ms) = parse_wall(wall_line) else {
        return Err(OutputError::Unrecognized);
    };
    let Some((mark, body)) = split_line(rest) else {
        return Err(OutputError::Unrecognized);
    };
    if mark != "Output:" {
        return Err(OutputError::Unrecognized);
    }
    if ok {
        let (texts, console) = split_body(body);
        return Ok(ToolResult {
            wall_ms,
            texts,
            console,
            full_output_path: find_full_path(body),
            unshown_images: find_unshown(body),
            output: body.to_owned(),
        });
    }
    let (before, error_body) = split_script_error(body);
    let Some(error_body) = error_body else {
        return Err(OutputError::Unrecognized);
    };
    let (head, calls, note) = split_failure(error_body);
    let (kind, message) = classify_head(head);
    let (texts, console) = split_body(before);
    Err(OutputError::Failure(OutputFailure {
        kind,
        message,
        calls,
        wall_ms,
        texts,
        console,
        full_output_path: find_full_path(body),
        unshown_images: note.or_else(|| find_unshown(body)),
        output: before.to_owned(),
    }))
}

fn split_line(text: &str) -> Option<(&str, &str)> {
    let (line, rest) = text.split_once('\n')?;
    Some((line.strip_suffix('\r').unwrap_or(line), rest))
}

fn parse_wall(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("Wall time ")?.strip_suffix(" seconds")?;
    let (whole, frac) = rest.split_once('.')?;
    if whole.is_empty()
        || frac.len() != 1
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !frac.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    let frac: u64 = frac.parse().ok()?;
    whole.checked_mul(1000)?.checked_add(frac.checked_mul(100)?)
}

fn split_script_error(body: &str) -> (&str, Option<&str>) {
    const MARK: &str = "Script error:\n";
    if let Some(rest) = body.strip_prefix(MARK) {
        return ("", Some(rest));
    }
    const NEEDLE: &str = "\nScript error:\n";
    match body.rfind(NEEDLE) {
        Some(index) => (&body[..index], Some(&body[index + NEEDLE.len()..])),
        None => (body, None),
    }
}

fn split_failure(body: &str) -> (&str, Vec<CallNote>, Option<u64>) {
    let Some((head, tail)) = body.rsplit_once("\n\n") else {
        return (body, Vec::new(), None);
    };
    match parse_summary(tail) {
        Some((calls, unshown)) => (head, calls, unshown),
        None => (body, Vec::new(), None),
    }
}

fn parse_summary(tail: &str) -> Option<(Vec<CallNote>, Option<u64>)> {
    let (first, rest) = match tail.split_once('\n') {
        Some((first, rest)) => (first.strip_suffix('\r').unwrap_or(first), rest),
        None => (tail, ""),
    };
    let calls = if first == NO_CALLS {
        Vec::new()
    } else if let Some(list) = first.strip_prefix(CALLS_PREFIX) {
        parse_calls(list)?
    } else {
        return None;
    };
    let rest = rest.trim_start_matches(['\r', '\n']);
    if rest.is_empty() {
        return Some((calls, None));
    }
    let mut lines = rest.lines();
    let note = parse_unshown_line(lines.next().unwrap_or(""))?;
    if lines.any(|line| !line.is_empty()) {
        return None;
    }
    Some((calls, Some(note)))
}

fn parse_calls(list: &str) -> Option<Vec<CallNote>> {
    if list.is_empty() {
        return Some(Vec::new());
    }
    let mut calls = Vec::new();
    for item in list.split(", ") {
        let (name, status) = item.rsplit_once(" (")?;
        let status = status.strip_suffix(')')?;
        if name.is_empty() || status.is_empty() {
            return None;
        }
        calls.push(CallNote {
            name: name.to_owned(),
            status: status.to_owned(),
        });
    }
    Some(calls)
}

fn classify_head(head: &str) -> (ErrorKind, String) {
    if let Some(message) = head.strip_prefix("Script timed out: ") {
        (ErrorKind::Timeout, message.to_owned())
    } else if let Some(message) = head.strip_prefix("Script aborted: ") {
        (ErrorKind::Aborted, message.to_owned())
    } else if let Some(message) = head.strip_prefix("Script sandbox failed: ") {
        (ErrorKind::Sandbox, message.to_owned())
    } else {
        (ErrorKind::Script, head.to_owned())
    }
}

fn split_body(body: &str) -> (Vec<String>, Option<String>) {
    let (body, console) = split_console(body);
    (split_texts(body), console.map(str::to_owned))
}

fn split_console(body: &str) -> (&str, Option<&str>) {
    const OPEN: &str = "<console_output>\n";
    const CLOSE: &str = "\n</console_output>";
    let Some(start) = body.rfind(OPEN) else {
        return (body, None);
    };
    let content_start = start + OPEN.len();
    let Some(rel_end) = body[content_start..].find(CLOSE) else {
        return (body, None);
    };
    let content_end = content_start + rel_end;
    let after = content_end + CLOSE.len();
    if body[after..].trim().is_empty() {
        let before = body[..start].strip_suffix('\n').unwrap_or(&body[..start]);
        (before, Some(&body[content_start..content_end]))
    } else {
        (body, None)
    }
}

fn split_texts(body: &str) -> Vec<String> {
    if body.is_empty() {
        return Vec::new();
    }
    if !body.starts_with("==> text ") {
        return vec![body.to_owned()];
    }
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(after_mark) = rest.strip_prefix("==> text ") {
        let Some((header, content)) = after_mark.split_once(" <==\n") else {
            parts.push(rest.to_owned());
            return parts;
        };
        if !valid_mark(header) {
            parts.push(rest.to_owned());
            return parts;
        }
        if let Some(next) = content.find("\n==> text ") {
            parts.push(content[..next].to_owned());
            rest = &content[next + 1..];
        } else {
            parts.push(content.to_owned());
            return parts;
        }
    }
    parts
}

fn valid_mark(header: &str) -> bool {
    let Some((left, right)) = header.split_once('/') else {
        return false;
    };
    !left.is_empty()
        && !right.is_empty()
        && left.bytes().all(|byte| byte.is_ascii_digit())
        && right.bytes().all(|byte| byte.is_ascii_digit())
}

fn find_full_path(text: &str) -> Option<String> {
    const START: &str = "[Full output: ";
    const END: &str = " (read with offset/limit)]";
    let start = text.rfind(START)? + START.len();
    let rest = text.get(start..)?;
    let end = rest.find(END)?;
    let path = &rest[..end];
    if path.is_empty() {
        None
    } else {
        Some(path.to_owned())
    }
}

fn find_unshown(text: &str) -> Option<u64> {
    text.lines().rev().find_map(parse_unshown_line)
}

fn parse_unshown_line(line: &str) -> Option<u64> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let rest = line.strip_prefix("Note: models.generateImages() returned ")?;
    let (count, rest) = rest.split_once(' ')?;
    let count: u64 = count.parse().ok()?;
    let expected = if count == 1 {
        "image that the script did not show. Show each image block of result.output with image(block)."
    } else {
        "images that the script did not show. Show each image block of result.output with image(block)."
    };
    if rest == expected {
        Some(count)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_text_and_console() {
        let blob = "\
Script completed
Wall time 1.2 seconds
Output:
==> text 1/2 <==
alpha
==> text 2/2 <==
beta
<console_output>
log
</console_output>
";
        let parsed = parse_output(blob).expect("completed shape");
        assert_eq!(parsed.wall_ms, 1200);
        assert_eq!(parsed.texts, vec!["alpha".to_owned(), "beta".to_owned()]);
        assert_eq!(parsed.console.as_deref(), Some("log"));
    }

    #[test]
    fn timeout_names_the_reason() {
        let blob = "\
Script failed
Wall time 0.5 seconds
Output:
partial
Script error:
Script timed out: deadline

Tool calls made before the failure (they are not undone): read (ok), bash (error)
";
        let err = parse_output(blob).expect_err("failure");
        assert_eq!(err.reason(), "timeout");
        let OutputError::Failure(failure) = err else {
            panic!("timeout is a failure");
        };
        assert_eq!(failure.kind, ErrorKind::Timeout);
        assert_eq!(failure.message, "deadline");
        assert_eq!(failure.output, "partial");
        assert_eq!(failure.calls.len(), 2);
        assert_eq!(failure.calls[1].status, "error");
    }

    #[test]
    fn aborted_and_sandbox_and_script() {
        let aborted = "\
Script failed
Wall time 0.0 seconds
Output:
Script error:
Script aborted: stopped

No tool calls were made.
";
        assert_eq!(parse_output(aborted).expect_err("abort").reason(), "aborted");
        let sandbox = "\
Script failed
Wall time 0.0 seconds
Output:
Script error:
Script sandbox failed: wasm trap

No tool calls were made.
";
        assert_eq!(parse_output(sandbox).expect_err("sandbox").reason(), "sandbox");
        let script = "\
Script failed
Wall time 0.0 seconds
Output:
Script error:
Error: nope

No tool calls were made.
";
        let err = parse_output(script).expect_err("script");
        assert_eq!(err.reason(), "script");
    }

    #[test]
    fn garbage_is_unrecognized() {
        assert_eq!(
            parse_output("hello").expect_err("shape").reason(),
            "unrecognized"
        );
    }

    #[test]
    fn image_extensions_follow_the_file() {
        assert_eq!(image_extension("image/jpeg"), ".jpg");
        assert_eq!(image_extension("image/svg+xml"), ".bin");
        assert_eq!(line_budget(100_000), (100_000 - LINE_MARGIN) / 2);
    }
}
