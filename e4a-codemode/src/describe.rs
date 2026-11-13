// Portions derived from pi-durable 1.5.2 (packages/durable/src/codemode/describe.ts), MIT, © 2025 Mario Zechner / Earendil Works. See LICENSES/pi-MIT.txt.

//! What the model reads about code mode: the tool description, and the `codemode`
//! section that lists callable tools by namespace inside a token budget.
//!
//! `sample` is the tool sample `renderToolSample` would produce. That renderer
//! lives in pi-codemode and is not linked here.

use std::collections::HashSet;

use serde_json::{json, Value};

use crate::models::models_description;

/// Default token budget of the tool declarations in the `codemode` section.
pub const DEFAULT_CODEMODE_INLINE_BUDGET: u32 = 3000;

/// Characters per token when estimating the cost of a tool section.
const CHARS_PER_TOKEN: u64 = 4;

const DESCRIPTION_INTRO: &str = "Run JavaScript that calls other tools. The input is raw JavaScript (not JSON, no code fence), run as an async function body in a QuickJS sandbox: top-level `await` and `return` work. No Node, file system, network, or timers.\n- `await tools.<name>({ ...args })` resolves to a string, or an object if the tool's declaration says so, and rejects with an Error on failure. A tool whose declaration returns an object resolves on failure too when it returned that object; check its fields. Calls still running when the script ends are cancelled.\n- The script survives a restart: it resumes where it was, without running finished calls again.\n- Optional first line: `// @options: {\"max_output_tokens\": 10000, \"timeout_ms\": 60000}`";

const GLOBALS: &str = "Globals:\n- `text(value)`, `image(dataUrlOrImageBlock)`, `console.log(...)`, and top-level `return` add output; `exit()` ends the script. With several text items, each starts with a `==> text N/M <==` line, and `console` lines follow the other output in one `<console_output>` block. `image()` also saves the image to a temp file and the result names its path.\n- `store(key, value)` and `load(key)` keep JSON values across codemode calls.\n- `ALL_TOOLS`, `await searchTools(query, { limit?, namespace? })`, `await describeTool(name)`, `await describeNamespace(name)`: find tools the `codemode` section does not list.";

const SECTION_CLOSING: &str = "The `codemode` section of the system prompt lists the tools scripts can call, by namespace, as far as space allows.";

/// A group of tools named by a common prefix, such as an MCP server's `mcp__github__`.
///
/// The prefix is not a `/` split. The longest prefix a tool name starts with wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodemodeNamespace {
    pub name: String,
    pub description: Option<String>,
    pub instructions: Option<String>,
    pub prefix: String,
}

/// A callable tool as the section and `declaration_of` see it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDescription {
    pub name: String,
    pub description: String,
    /// JSON Schema of the single argument (`parameters` on the registration).
    pub parameters: Value,
    /// Present when the tool resolves to a value instead of text.
    pub structured_output_schema: Option<Value>,
    /// Description plus TypeScript declaration, already rendered.
    pub sample: String,
}

/// What a script sees of a tool. No output schema means a string.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

/// The code mode tool's description. `models` appends the catalog paragraph.
pub fn codemode_description(models: bool) -> String {
    let models_text = if models {
        Some(models_description())
    } else {
        None
    };
    let mut parts: Vec<&str> = vec![DESCRIPTION_INTRO, GLOBALS];
    if let Some(text) = models_text.as_deref() {
        parts.push(text);
    }
    parts.push(SECTION_CLOSING);
    parts.join("\n\n")
}

pub fn declaration_of(tool: &ToolDescription) -> ToolDeclaration {
    ToolDeclaration {
        name: tool.name.clone(),
        description: tool.description.clone(),
        input_schema: tool.parameters.clone(),
        output_schema: tool
            .structured_output_schema
            .clone()
            .unwrap_or_else(text_output_schema),
    }
}

/// Namespace token of a tool name: the text before the first `__`.
///
/// `describe.js` does not split on `/`. A tool belongs to the configured
/// [`CodemodeNamespace`] whose `prefix` is the longest one `name` starts with
/// ([`longest_namespace`]). Those prefixes use `__`, as in an MCP server's
/// `mcp__github__`. Given only the name, this returns the text before the
/// first `__`. A name with no `__`, or an empty segment, has no namespace.
pub fn namespace_of(tool: &str) -> Option<&str> {
    let (namespace, _) = tool.split_once("__")?;
    if namespace.is_empty() {
        None
    } else {
        Some(namespace)
    }
}

/// The namespace whose prefix is the longest one `name` starts with.
pub fn longest_namespace<'a>(
    name: &str,
    namespaces: &'a [CodemodeNamespace],
) -> Option<&'a CodemodeNamespace> {
    let mut found: Option<&CodemodeNamespace> = None;
    for namespace in namespaces {
        if !name.starts_with(&namespace.prefix) {
            continue;
        }
        let replace = match found {
            None => true,
            Some(current) => utf16_len(&namespace.prefix) > utf16_len(&current.prefix),
        };
        if replace {
            found = Some(namespace);
        }
    }
    found
}

/// The `codemode` section. `None` when `tools` is empty.
///
/// Each round, every group places its cheapest remaining tool. A group whose
/// next tool does not fit drops out. Tools with no namespace are the first group.
pub fn codemode_section(
    tools: &[ToolDescription],
    namespaces: &[CodemodeNamespace],
    budget: u64,
) -> Option<String> {
    if tools.is_empty() {
        return None;
    }
    let mut groups: Vec<Group<'_>> = vec![Group {
        namespace: None,
        entries: Vec::new(),
    }];
    for tool in tools {
        let namespace = longest_namespace(&tool.name, namespaces);
        let key_name = namespace.map(|item| item.name.as_str());
        let index = groups.iter().position(|group| {
            group.namespace.map(|item| item.name.as_str()) == key_name
        });
        let index = match index {
            Some(index) => index,
            None => {
                groups.push(Group {
                    namespace,
                    entries: Vec::new(),
                });
                groups.len().saturating_sub(1)
            }
        };
        let group = &mut groups[index];
        let section = render_tool_section(tool);
        let cost = token_cost(&section);
        group.entries.push(Entry {
            name: tool.name.clone(),
            section,
            cost,
        });
    }
    groups.sort_by(|left, right| match (left.namespace, right.namespace) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(left), Some(right)) => left.name.cmp(&right.name),
    });
    let queues: Vec<Vec<Queued>> = groups
        .iter()
        .map(|group| {
            group
                .entries
                .iter()
                .map(|entry| Queued {
                    name: entry.name.clone(),
                    cost: entry.cost,
                })
                .collect()
        })
        .collect();
    let shown = select_catalog(&queues, budget);
    let mut sections = vec!["Tools scripts can call (`await tools.<name>(args)`):".to_owned()];
    for group in &groups {
        if group.entries.is_empty() {
            continue;
        }
        let visible: Vec<&Entry> = group
            .entries
            .iter()
            .filter(|entry| shown.contains(&entry.name))
            .collect();
        if let Some(namespace) = group.namespace {
            let listing = if visible.len() == group.entries.len() {
                ""
            } else if visible.is_empty() {
                " (tools not listed)"
            } else {
                " (some tools not listed)"
            };
            let mut heading = format!("## {}{listing}", namespace.name);
            if let Some(description) = namespace
                .description
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                heading.push('\n');
                heading.push_str(description);
            }
            sections.push(heading);
        } else if visible.len() < group.entries.len() {
            sections.push("Some tools are not listed: find them with `searchTools()`.".to_owned());
        }
        for entry in visible {
            sections.push(entry.section.clone());
        }
    }
    Some(sections.join("\n\n"))
}

/// The identifier a script uses. Characters that are not valid in a JavaScript
/// identifier become `_`. An empty name becomes `_`.
pub fn to_codemode_identifier(name: &str) -> String {
    let mut ident = String::new();
    for unit in name.encode_utf16() {
        let mapped = char::from_u32(u32::from(unit)).filter(|ch| {
            if ident.is_empty() {
                ch.is_ascii_alphabetic() || *ch == '_' || *ch == '$'
            } else {
                ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '$'
            }
        });
        ident.push(mapped.unwrap_or('_'));
    }
    if ident.is_empty() {
        "_".to_owned()
    } else {
        ident
    }
}

fn text_output_schema() -> Value {
    json!({ "type": "string" })
}

struct Group<'a> {
    namespace: Option<&'a CodemodeNamespace>,
    entries: Vec<Entry>,
}

struct Entry {
    name: String,
    section: String,
    cost: u64,
}

#[derive(Clone)]
struct Queued {
    name: String,
    cost: u64,
}

fn render_tool_section(tool: &ToolDescription) -> String {
    let id = to_codemode_identifier(&tool.name);
    let heading = if id == tool.name {
        format!("### `{id}`")
    } else {
        format!("### `{id}` (`{}`)", tool.name)
    };
    format!("{heading}\n{}", tool.sample.trim())
}

fn select_catalog(groups: &[Vec<Queued>], budget: u64) -> HashSet<String> {
    let mut queues: Vec<Vec<Queued>> = groups
        .iter()
        .filter_map(|group| {
            if group.is_empty() {
                return None;
            }
            let mut queue = group.clone();
            queue.sort_by(|left, right| left.cost.cmp(&right.cost));
            Some(queue)
        })
        .collect();
    let mut shown = HashSet::new();
    let mut remaining = budget;
    while !queues.is_empty() {
        let mut next_round = Vec::new();
        for mut queue in queues.drain(..) {
            let Some(cost) = queue.first().map(|item| item.cost) else {
                continue;
            };
            if cost > remaining {
                continue;
            }
            remaining -= cost;
            let item = queue.remove(0);
            shown.insert(item.name);
            if !queue.is_empty() {
                next_round.push(queue);
            }
        }
        queues = next_round;
    }
    shown
}

fn token_cost(section: &str) -> u64 {
    u64::try_from(utf16_len(section))
        .unwrap_or(u64::MAX)
        .div_ceil(CHARS_PER_TOKEN)
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, sample: &str) -> ToolDescription {
        ToolDescription {
            name: name.to_owned(),
            description: sample.to_owned(),
            parameters: json!({ "type": "object" }),
            structured_output_schema: None,
            sample: sample.to_owned(),
        }
    }

    #[test]
    fn longest_prefix_wins_over_a_shorter_one() {
        let namespaces = [
            CodemodeNamespace {
                name: "mcp".to_owned(),
                description: None,
                instructions: None,
                prefix: "mcp__".to_owned(),
            },
            CodemodeNamespace {
                name: "github".to_owned(),
                description: Some("issues".to_owned()),
                instructions: None,
                prefix: "mcp__github__".to_owned(),
            },
        ];
        let found = longest_namespace("mcp__github__search", &namespaces).expect("prefix");
        assert_eq!(found.name, "github");
        assert!(longest_namespace("read", &namespaces).is_none());
    }

    #[test]
    fn empty_tool_list_has_no_section() {
        assert!(codemode_section(&[], &[], u64::from(DEFAULT_CODEMODE_INLINE_BUDGET)).is_none());
    }

    #[test]
    fn description_mentions_models_only_when_asked() {
        assert!(!codemode_description(false).contains("getModelsOfType"));
        assert!(codemode_description(true).contains("getModelsOfType"));
    }

    #[test]
    fn declaration_without_output_schema_is_a_string() {
        let declaration = declaration_of(&tool("read", "Read a file."));
        assert_eq!(declaration.output_schema, json!({ "type": "string" }));
    }

    #[test]
    fn hyphenated_names_become_identifiers() {
        assert_eq!(to_codemode_identifier("my-tool"), "my_tool");
        assert_eq!(to_codemode_identifier(""), "_");
    }
}
