//! Roles as data: the roster ez4a hires from.
//!
//! A role file is one JSON object — `id`, `model`, `tools`, `instructions` —
//! under `roles/` (the repo ships a starter roster) or `EZ4A_ROLES_DIR`. The
//! builtin roles are the fallback when no file names the id, so
//! `ez4a run --role reviewer` works on a bare checkout. An agent that spawns
//! workers never lists the `agent` tool for them: the manager difference is
//! data too, and a role file that grants `agent` to a worker-shaped roster is
//! the operator's choice to make, not the loader's to police.

use std::path::Path;

use serde_json::Value;

use crate::dev_agent::AgentRole;

/// One hired role, owned: file data outlives no catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleDef {
    pub id: String,
    pub model: String,
    pub tools: Vec<String>,
    pub instructions: String,
}

impl RoleDef {
    /// Parse one role object. `id` and `instructions` must be non-empty; a
    /// role with no tools works (a pure advisor), a model may be empty (the
    /// caller's role-model resolution decides).
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| "role needs a non-empty id".to_owned())?
            .to_owned();
        let model = value
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let instructions = value
            .get("instructions")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| format!("role {id} needs instructions"))?
            .to_owned();
        let tools = value
            .get("tools")
            .and_then(Value::as_array)
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(RoleDef {
            id,
            model,
            tools,
            instructions,
        })
    }

    /// The prompt a hired role runs with: its standing instructions, then the
    /// task.
    pub fn prompt_for(&self, task: &str) -> String {
        format!("{}\n\nTask: {}", self.instructions, task)
    }
}

fn builtin_json(role: &AgentRole) -> Value {
    serde_json::json!({
        "id": role.id,
        "model": role.model,
        "tools": role.tools,
        "instructions": role.instructions,
    })
}

/// The starter roster: the builtin roles plus one file-only role, reviewer.
pub fn builtin() -> Vec<RoleDef> {
    let mut roles: Vec<RoleDef> = [
        crate::dev_agent::dev_agent(),
        crate::manager_agent::manager_agent(),
        crate::manager_agent::hatchery_admin_agent(),
    ]
    .iter()
    .map(|role| RoleDef::from_json(&builtin_json(role)).expect("builtin roles are well formed"))
    .collect();
    roles.push(
        RoleDef::from_json(&serde_json::json!({
            "id": "reviewer",
            "model": "google/gemma-4-31b-it:free",
            "tools": ["read", "mail_send", "mail_inbox"],
            "instructions": "You are the reviewer. Read what you are given, judge it \
against the request, and answer with a verdict and the specific problems you found. \
Report by mail when the work is not yours to fix."
        }))
        .expect("reviewer is well formed"),
    );
    roles
}

/// The catalog: every `roles/*.json` (or `EZ4A_ROLES_DIR`) over the builtin
/// roster — a file redefines a builtin id by naming it. A directory that
/// cannot be read is just the builtin roster.
pub fn load(dir: Option<&Path>) -> Vec<RoleDef> {
    let mut roles = builtin();
    let Some(dir) = dir else {
        return roles;
    };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return roles,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Ok(role) = RoleDef::from_json(&value) {
            roles.retain(|other| other.id != role.id);
            roles.push(role);
        }
    }
    roles
}

/// Find one role by id in the catalog.
pub fn find<'a>(catalog: &'a [RoleDef], id: &str) -> Option<&'a RoleDef> {
    catalog.iter().find(|role| role.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_file_parses_and_redefines_a_builtin() {
        let dir = std::env::temp_dir().join(format!("ez4a-roles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dev.json");
        std::fs::write(
            &file,
            serde_json::json!({
                "id": "dev",
                "model": "google/gemma-4-31b-it:free",
                "tools": ["read", "write"],
                "instructions": "Lean worker."
            })
            .to_string(),
        )
        .unwrap();

        let catalog = load(Some(&dir));
        let dev = find(&catalog, "dev").expect("dev is redefined");
        assert_eq!(dev.model, "google/gemma-4-31b-it:free");
        assert_eq!(dev.tools, vec!["read", "write"]);
        assert!(find(&catalog, "reviewer").is_some());
        assert_eq!(find(&catalog, "nope"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_role_without_instructions_or_id_is_refused() {
        assert!(RoleDef::from_json(&serde_json::json!({ "id": "x" })).is_err());
        assert!(RoleDef::from_json(&serde_json::json!({ "instructions": "y" })).is_err());
        assert!(RoleDef::from_json(&serde_json::json!({ "id": " ", "instructions": "y" })).is_err());
        let role = RoleDef::from_json(&serde_json::json!({ "id": "x", "instructions": "y" }))
            .unwrap();
        assert_eq!(role.prompt_for("do it"), "y\n\nTask: do it");
    }
}
