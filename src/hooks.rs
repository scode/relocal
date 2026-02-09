//! Hook JSON merging for `.claude/settings.json`.
//!
//! Claude hooks are how the remote Claude session triggers syncs back to the
//! local machine. This module handles merging relocal's hook entries into an
//! existing `settings.json` without clobbering user-defined hooks or other keys.
//! Relocal hooks are identified by `relocal-hook.sh` in the command string.

use serde_json::{json, Map, Value};

/// Marker substring used to identify relocal hook entries.
const RELOCAL_HOOK_MARKER: &str = "relocal-hook.sh";

/// Builds the hook command string for a given session and direction.
fn hook_command(session_name: &str, direction: &str) -> String {
    format!("RELOCAL_SESSION={session_name} ~/relocal/.bin/relocal-hook.sh {direction}")
}

/// Builds a single relocal hook entry as a JSON value.
fn relocal_hook_entry(session_name: &str, direction: &str) -> Value {
    json!({
        "type": "command",
        "command": hook_command(session_name, direction)
    })
}

/// Returns true if a hook entry is a relocal-managed hook.
fn is_relocal_entry(entry: &Value) -> bool {
    entry
        .get("command")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s.contains(RELOCAL_HOOK_MARKER))
}

/// Ensures the hook array contains exactly one relocal entry with the correct
/// session name. User hooks are preserved in their original positions.
fn upsert_relocal_hook(array: &mut Vec<Value>, session_name: &str, direction: &str) {
    let new_entry = relocal_hook_entry(session_name, direction);

    if let Some(pos) = array.iter().position(is_relocal_entry) {
        array[pos] = new_entry;
    } else {
        array.push(new_entry);
    }
}

/// Merges relocal hook configuration into an existing `settings.json` value.
///
/// If `existing` is `None`, returns a fresh `settings.json` with just the hooks.
/// Otherwise, preserves all existing keys and user-defined hooks while ensuring
/// relocal's `UserPromptSubmit` and `Stop` hooks are present and up-to-date.
pub fn merge_hooks(existing: Option<Value>, session_name: &str) -> Value {
    let mut root = match existing {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };

    let hooks = root
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("hooks key must be an object");

    for (hook_name, direction) in [("UserPromptSubmit", "push"), ("Stop", "pull")] {
        let array = hooks
            .entry(hook_name)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .expect("hook array must be an array");

        upsert_relocal_hook(array, session_name, direction);
    }

    Value::Object(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_existing_file() {
        let result = merge_hooks(None, "my-session");
        let hooks = result.get("hooks").unwrap();
        let submit = hooks.get("UserPromptSubmit").unwrap().as_array().unwrap();
        let stop = hooks.get("Stop").unwrap().as_array().unwrap();

        assert_eq!(submit.len(), 1);
        assert_eq!(stop.len(), 1);
        assert!(submit[0]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh push"));
        assert!(stop[0]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh pull"));
    }

    #[test]
    fn no_hooks_key() {
        let existing = json!({"allowedTools": ["bash"]});
        let result = merge_hooks(Some(existing), "s1");

        // Other keys preserved
        assert_eq!(result["allowedTools"], json!(["bash"]));
        // Hooks added
        assert!(result.get("hooks").is_some());
        assert_eq!(
            result["hooks"]["UserPromptSubmit"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn no_arrays() {
        let existing = json!({"hooks": {}});
        let result = merge_hooks(Some(existing), "s1");

        assert_eq!(
            result["hooks"]["UserPromptSubmit"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(result["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn no_relocal_entry_appends() {
        let existing = json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"type": "command", "command": "user-script.sh"}
                ],
                "Stop": [
                    {"type": "command", "command": "other-script.sh"}
                ]
            }
        });
        let result = merge_hooks(Some(existing), "s1");

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let stop = result["hooks"]["Stop"].as_array().unwrap();

        // User hooks preserved, relocal appended
        assert_eq!(submit.len(), 2);
        assert_eq!(submit[0]["command"], "user-script.sh");
        assert!(submit[1]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh"));

        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["command"], "other-script.sh");
    }

    #[test]
    fn existing_relocal_entry_updated() {
        let existing = json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"type": "command", "command": "RELOCAL_SESSION=old ~/relocal/.bin/relocal-hook.sh push"}
                ],
                "Stop": [
                    {"type": "command", "command": "RELOCAL_SESSION=old ~/relocal/.bin/relocal-hook.sh pull"}
                ]
            }
        });
        let result = merge_hooks(Some(existing), "new-session");

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let stop = result["hooks"]["Stop"].as_array().unwrap();

        // Still one entry each (updated in place, not duplicated)
        assert_eq!(submit.len(), 1);
        assert_eq!(stop.len(), 1);
        assert!(submit[0]["command"]
            .as_str()
            .unwrap()
            .contains("RELOCAL_SESSION=new-session"));
        assert!(stop[0]["command"]
            .as_str()
            .unwrap()
            .contains("RELOCAL_SESSION=new-session"));
    }

    #[test]
    fn user_hooks_preserved_and_not_reordered() {
        let existing = json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"type": "command", "command": "first.sh"},
                    {"type": "command", "command": "RELOCAL_SESSION=old ~/relocal/.bin/relocal-hook.sh push"},
                    {"type": "command", "command": "third.sh"}
                ]
            }
        });
        let result = merge_hooks(Some(existing), "s1");

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(submit.len(), 3);
        assert_eq!(submit[0]["command"], "first.sh");
        assert!(submit[1]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh"));
        assert_eq!(submit[2]["command"], "third.sh");
    }

    #[test]
    fn other_top_level_keys_preserved() {
        let existing = json!({
            "allowedTools": ["bash", "read"],
            "model": "opus",
            "hooks": {}
        });
        let result = merge_hooks(Some(existing), "s1");

        assert_eq!(result["allowedTools"], json!(["bash", "read"]));
        assert_eq!(result["model"], "opus");
    }

    #[test]
    fn session_name_interpolated() {
        let result = merge_hooks(None, "my-proj");
        let cmd = result["hooks"]["UserPromptSubmit"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("RELOCAL_SESSION=my-proj"));
    }

    #[test]
    fn idempotent() {
        let first = merge_hooks(None, "s1");
        let second = merge_hooks(Some(first.clone()), "s1");
        assert_eq!(first, second);
    }
}
