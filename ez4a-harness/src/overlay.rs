// engine zero for agents overlay. The worker runs. Advisor and compressor sit on the boundary.

//! Decision after a tool result, before the next model call.
//!
//! Pi does not ship a zoo of roles. The worker is the generation. A steer or
//! abort stops it. Compaction is a separate task. This is that boundary
//! without a second model call.

use serde_json::Value;

/// What the overlay tells the worker loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    /// Run the tool, or keep going after it.
    Continue,
    /// Do not run the tool. The worker stops.
    Stop { reason: &'static str },
    /// Context should be summarized before the next model call.
    Compact,
}

const COMPACT_AT: usize = 80_000;

/// Advisor rule for one pending tool. `transcript_chars` is the text already
/// stored, not this call.
pub fn after_tool(name: &str, arguments: &Value, transcript_chars: usize) -> Overlay {
    if destructive(name, arguments) {
        return Overlay::Stop {
            reason: "advisor stopped a destructive command",
        };
    }
    if transcript_chars >= COMPACT_AT {
        return Overlay::Compact;
    }
    Overlay::Continue
}

fn destructive(name: &str, arguments: &Value) -> bool {
    if name != "powershell" && name != "bash" {
        return false;
    }
    let Some(command) = arguments.get("command").and_then(Value::as_str) else {
        return false;
    };
    let lower = command.to_ascii_lowercase();
    lower.contains("remove-item")
        || lower.contains("rm -rf")
        || lower.contains("rm -fr")
        || lower.contains("del /f")
        || lower.contains("format ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stops_remove_item_and_compacts_when_long() {
        let kill = json!({"command": "Remove-Item -Recurse a.txt"});
        assert_eq!(
            after_tool("powershell", &kill, 0),
            Overlay::Stop {
                reason: "advisor stopped a destructive command"
            }
        );
        assert_eq!(after_tool("read", &json!({"path": "a.txt"}), 80_000), Overlay::Compact);
        assert_eq!(after_tool("read", &json!({"path": "a.txt"}), 10), Overlay::Continue);
    }
}
