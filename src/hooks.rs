//! Hook JSON merging and hook script generation.
//!
//! Claude hooks are how the remote Claude session triggers syncs back to the
//! local machine. This module has two responsibilities:
//!
//! 1. **JSON merging** ([`merge_hooks`]): merges relocal's hook entries into an
//!    existing `settings.json` without clobbering user-defined hooks or other keys.
//!    Relocal hooks are identified by `relocal-hook.sh` in the command string.
//!
//! 2. **Script generation** ([`hook_script_content`]): produces the bash script
//!    installed at `~/relocal/.bin/relocal-hook.sh` on the remote. The script
//!    communicates with the local sidecar via FIFOs.

use serde_json::{json, Map, Value};

/// Marker substring used to identify relocal hook entries.
const RELOCAL_HOOK_MARKER: &str = "relocal-hook.sh";

/// Builds the hook command string for a given session and direction.
fn hook_command(session_name: &str, direction: &str) -> String {
    format!("RELOCAL_SESSION={session_name} ~/relocal/.bin/relocal-hook.sh {direction}")
}

/// Builds a relocal matcher group as a JSON value.
///
/// Claude hooks use a nested format where each array element is a matcher
/// group containing a `hooks` array of handler objects.
fn relocal_hook_entry(session_name: &str, direction: &str) -> Value {
    json!({
        "hooks": [
            {
                "type": "command",
                "command": hook_command(session_name, direction)
            }
        ]
    })
}

/// Returns true if a matcher group contains a relocal-managed hook.
fn is_relocal_entry(entry: &Value) -> bool {
    entry
        .get("hooks")
        .and_then(|v| v.as_array())
        .is_some_and(|hooks| {
            hooks.iter().any(|h| {
                h.get("command")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s.contains(RELOCAL_HOOK_MARKER))
            })
        })
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

/// Returns the content of the `relocal-hook.sh` script installed on the remote.
///
/// The script accepts a direction argument (`push` or `pull`), writes it to the
/// session's request FIFO, then blocks reading an ack from the ack FIFO. The
/// sidecar on the local side performs the actual sync and writes the ack.
///
/// Each invocation logs timestamped events to
/// `~/relocal/.logs/<session>-<direction>.log` via file descriptor 3,
/// keeping stdout/stderr clean for Claude.
pub fn hook_script_content() -> String {
    r#"#!/bin/bash
set -euo pipefail

DIRECTION="${1:?Usage: relocal-hook.sh <push|pull>}"
FIFO_DIR="$HOME/relocal/.fifos"
LOG_DIR="$HOME/relocal/.logs"
REQUEST_FIFO="$FIFO_DIR/${RELOCAL_SESSION}-request"
ACK_FIFO="$FIFO_DIR/${RELOCAL_SESSION}-ack"

# Open log file (overwritten each invocation per direction)
mkdir -p "$LOG_DIR"
exec 3>"$LOG_DIR/${RELOCAL_SESSION}-${DIRECTION}.log"

echo "[$(date -Iseconds)] hook start: direction=$DIRECTION session=$RELOCAL_SESSION" >&3

# Send sync request (blocks until sidecar reads it)
echo "$DIRECTION" > "$REQUEST_FIFO"
echo "[$(date -Iseconds)] request sent, waiting for ack" >&3

# Wait for ack (blocks until sidecar writes response)
ACK=$(cat "$ACK_FIFO")

if [ "$ACK" = "ok" ]; then
    echo "[$(date -Iseconds)] ack received: ok" >&3
    exec 3>&-
    exit 0
else
    # Strip "error:" prefix if present
    MSG="${ACK#error:}"
    echo "[$(date -Iseconds)] ack received: error: $MSG" >&3
    exec 3>&-
    echo "$MSG" >&2
    exit 1
fi
"#
    .to_string()
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
        // Each entry is a matcher group with a "hooks" array inside
        assert!(submit[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh push"));
        assert!(stop[0]["hooks"][0]["command"]
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
                    {"hooks": [{"type": "command", "command": "user-script.sh"}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "other-script.sh"}]}
                ]
            }
        });
        let result = merge_hooks(Some(existing), "s1");

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let stop = result["hooks"]["Stop"].as_array().unwrap();

        // User hooks preserved, relocal appended
        assert_eq!(submit.len(), 2);
        assert_eq!(submit[0]["hooks"][0]["command"], "user-script.sh");
        assert!(submit[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh"));

        assert_eq!(stop.len(), 2);
        assert_eq!(stop[0]["hooks"][0]["command"], "other-script.sh");
    }

    #[test]
    fn existing_relocal_entry_updated() {
        let existing = json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "RELOCAL_SESSION=old ~/relocal/.bin/relocal-hook.sh push"}]}
                ],
                "Stop": [
                    {"hooks": [{"type": "command", "command": "RELOCAL_SESSION=old ~/relocal/.bin/relocal-hook.sh pull"}]}
                ]
            }
        });
        let result = merge_hooks(Some(existing), "new-session");

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        let stop = result["hooks"]["Stop"].as_array().unwrap();

        // Still one entry each (updated in place, not duplicated)
        assert_eq!(submit.len(), 1);
        assert_eq!(stop.len(), 1);
        assert!(submit[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("RELOCAL_SESSION=new-session"));
        assert!(stop[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("RELOCAL_SESSION=new-session"));
    }

    #[test]
    fn user_hooks_preserved_and_not_reordered() {
        let existing = json!({
            "hooks": {
                "UserPromptSubmit": [
                    {"hooks": [{"type": "command", "command": "first.sh"}]},
                    {"hooks": [{"type": "command", "command": "RELOCAL_SESSION=old ~/relocal/.bin/relocal-hook.sh push"}]},
                    {"hooks": [{"type": "command", "command": "third.sh"}]}
                ]
            }
        });
        let result = merge_hooks(Some(existing), "s1");

        let submit = result["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(submit.len(), 3);
        assert_eq!(submit[0]["hooks"][0]["command"], "first.sh");
        assert!(submit[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("relocal-hook.sh"));
        assert_eq!(submit[2]["hooks"][0]["command"], "third.sh");
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
        let cmd = result["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
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

    #[test]
    fn hook_script_has_shebang_and_strict_mode() {
        let script = hook_script_content();
        assert!(script.starts_with("#!/bin/bash\nset -euo pipefail\n"));
    }

    #[test]
    fn hook_script_uses_relocal_session_env_var() {
        let script = hook_script_content();
        assert!(script.contains("${RELOCAL_SESSION}"));
    }

    #[test]
    fn hook_script_fifo_paths() {
        let script = hook_script_content();
        assert!(script.contains("${RELOCAL_SESSION}-request"));
        assert!(script.contains("${RELOCAL_SESSION}-ack"));
        assert!(script.contains("$HOME/relocal/.fifos"));
    }

    #[test]
    fn hook_script_writes_direction_to_request_fifo() {
        let script = hook_script_content();
        assert!(script.contains("echo \"$DIRECTION\" > \"$REQUEST_FIFO\""));
    }

    #[test]
    fn hook_script_reads_ack() {
        let script = hook_script_content();
        assert!(script.contains("cat \"$ACK_FIFO\""));
    }

    #[test]
    fn hook_script_handles_ok_and_error() {
        let script = hook_script_content();
        assert!(script.contains("\"$ACK\" = \"ok\""));
        assert!(script.contains("exit 0"));
        assert!(script.contains("exit 1"));
        assert!(script.contains(">&2"));
    }

    #[test]
    fn hook_script_opens_log_file() {
        let script = hook_script_content();
        assert!(script.contains("$HOME/relocal/.logs"));
        assert!(script.contains("exec 3>\"$LOG_DIR/${RELOCAL_SESSION}-${DIRECTION}.log\""));
    }

    #[test]
    fn hook_script_logs_to_fd3() {
        let script = hook_script_content();
        // Key events are logged to FD 3
        assert!(script.contains(">&3"));
        assert!(script.contains("hook start:"));
        assert!(script.contains("request sent"));
        assert!(script.contains("ack received"));
    }

    #[test]
    fn hook_script_closes_log_fd() {
        let script = hook_script_content();
        assert!(script.contains("exec 3>&-"));
    }
}
