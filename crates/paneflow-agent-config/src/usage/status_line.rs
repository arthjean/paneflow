use std::path::Path;

use serde_json::{json, Value};

use crate::claude_hooks::MANAGED_MARKER;
use crate::hook_command::{command_program_token, display_hook_program, shell_program_path};

pub const STATUS_LINE_KEY: &str = "statusLine";
pub const STATUS_LINE_EVENT: &str = "StatusLine";
const DISABLE_ALL_HOOKS_KEY: &str = "disableAllHooks";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLineState {
    Missing,
    Managed,
    ForeignStatusLine,
    HooksDisabled,
}

pub fn status_line_command(reporter: &Path, home: Option<&Path>) -> String {
    let absolute = shell_program_path(reporter);
    let program = if absolute.starts_with('\'') {
        home_relative_program(reporter, home).unwrap_or(absolute)
    } else {
        absolute
    };
    format!("{program} {STATUS_LINE_EVENT}")
}

fn home_relative_program(reporter: &Path, home: Option<&Path>) -> Option<String> {
    let relative = display_hook_program(reporter.strip_prefix(home?).ok()?).replace('\\', "/");
    let shell_neutral = !relative.is_empty()
        && relative
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-/+@".contains(character));
    shell_neutral.then(|| format!("~/{relative}"))
}

pub fn status_line_state(settings: &Value) -> StatusLineState {
    if settings.get(DISABLE_ALL_HOOKS_KEY).and_then(Value::as_bool) == Some(true) {
        return StatusLineState::HooksDisabled;
    }
    match settings.get(STATUS_LINE_KEY) {
        None | Some(Value::Null) => StatusLineState::Missing,
        Some(status_line) if is_managed_status_line(status_line) => StatusLineState::Managed,
        Some(_) => StatusLineState::ForeignStatusLine,
    }
}

pub fn is_managed_status_line(status_line: &Value) -> bool {
    status_line.get(MANAGED_MARKER).and_then(Value::as_bool) == Some(true)
        || status_line
            .get("command")
            .and_then(Value::as_str)
            .and_then(command_program_token)
            .is_some_and(|program| is_reporter_program(&program))
}

fn is_reporter_program(program: &str) -> bool {
    let basename = program.rsplit(['/', '\\']).next().unwrap_or(program);
    matches!(
        basename,
        "paneflow-ai-hook" | "paneflow-ai-hook.exe" | "paneflow-ai-hook.sh"
    )
}

pub fn reconcile_status_line(settings: &mut Value, command: Option<&str>) -> bool {
    let Some(root) = settings.as_object_mut() else {
        return false;
    };
    let current = root.get(STATUS_LINE_KEY);
    let managed = current.is_some_and(is_managed_status_line);
    let absent = matches!(current, None | Some(Value::Null));
    match command {
        None if managed => root.remove(STATUS_LINE_KEY).is_some(),
        None => false,
        Some(command) if absent || managed => {
            let wanted = json!({
                "type": "command",
                "command": command,
                MANAGED_MARKER: true,
            });
            if current == Some(&wanted) {
                return false;
            }
            root.insert(STATUS_LINE_KEY.to_string(), wanted);
            true
        }
        Some(_) => false,
    }
}

pub fn remove_managed_status_line(settings: &mut Value) -> bool {
    reconcile_status_line(settings, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: &str = "/home/u/.paneflow/bin/paneflow-ai-hook.sh StatusLine";

    #[test]
    fn a_missing_status_line_is_installed_and_marked() {
        let mut settings = json!({"model": "opus", "hooks": {}});
        assert!(reconcile_status_line(&mut settings, Some(COMMAND)));
        assert_eq!(
            settings,
            json!({
                "model": "opus",
                "hooks": {},
                "statusLine": {"type": "command", "command": COMMAND, MANAGED_MARKER: true}
            })
        );
        assert_eq!(status_line_state(&settings), StatusLineState::Managed);
        assert!(!reconcile_status_line(&mut settings, Some(COMMAND)));
    }

    #[test]
    fn a_foreign_status_line_is_never_touched() {
        let foreign = json!({"type": "command", "command": "bash ~/.claude/statusline.sh", "refreshInterval": 5});
        let mut settings = json!({"statusLine": foreign.clone()});
        assert_eq!(
            status_line_state(&settings),
            StatusLineState::ForeignStatusLine
        );
        assert!(!reconcile_status_line(&mut settings, Some(COMMAND)));
        assert!(!remove_managed_status_line(&mut settings));
        assert_eq!(settings["statusLine"], foreign);
    }

    #[test]
    fn a_managed_status_line_follows_a_moved_reporter() {
        let mut settings = json!({
            "statusLine": {"type": "command", "command": "/old/bin/paneflow-ai-hook StatusLine"}
        });
        assert_eq!(status_line_state(&settings), StatusLineState::Managed);
        assert!(reconcile_status_line(&mut settings, Some(COMMAND)));
        assert_eq!(settings["statusLine"]["command"], COMMAND);
        assert_eq!(settings["statusLine"][MANAGED_MARKER], true);
    }

    #[test]
    fn disabling_removes_only_the_managed_status_line() {
        let mut settings = json!({"statusLine": {"type": "command", "command": COMMAND, MANAGED_MARKER: true}, "theme": "dark"});
        assert!(reconcile_status_line(&mut settings, None));
        assert_eq!(settings, json!({"theme": "dark"}));
        assert!(!remove_managed_status_line(&mut settings));
        assert_eq!(status_line_state(&settings), StatusLineState::Missing);
    }

    #[test]
    fn disabled_hooks_win_over_every_other_state() {
        let settings =
            json!({"disableAllHooks": true, "statusLine": {"type": "command", "command": COMMAND}});
        assert_eq!(status_line_state(&settings), StatusLineState::HooksDisabled);
        assert_eq!(
            status_line_state(&json!({"disableAllHooks": false})),
            StatusLineState::Missing
        );
        assert_eq!(
            status_line_state(&json!({"statusLine": null})),
            StatusLineState::Missing
        );
    }

    #[test]
    fn a_non_object_root_is_left_alone() {
        let mut settings = json!([1, 2]);
        assert!(!reconcile_status_line(&mut settings, Some(COMMAND)));
        assert_eq!(settings, json!([1, 2]));
    }

    #[test]
    fn a_reporter_under_a_home_with_spaces_uses_the_shell_neutral_tilde_form() {
        let home = Path::new("/home/Jean Dupont");
        let reporter = home
            .join(".paneflow")
            .join("bin")
            .join("paneflow-ai-hook.exe");
        let command = status_line_command(&reporter, Some(home));
        assert_eq!(command, "~/.paneflow/bin/paneflow-ai-hook.exe StatusLine");
        assert!(is_managed_status_line(&json!({ "command": command })));
        let plain_home = Path::new("/home/u");
        assert_eq!(
            status_line_command(
                &plain_home.join(".paneflow/bin/paneflow-ai-hook"),
                Some(plain_home)
            ),
            "/home/u/.paneflow/bin/paneflow-ai-hook StatusLine",
            "a path that needs no quotes stays absolute"
        );
        let odd = home.join("my bin").join("paneflow-ai-hook");
        assert_eq!(
            status_line_command(&odd, Some(home)),
            "'/home/Jean Dupont/my bin/paneflow-ai-hook' StatusLine",
            "a relative part that itself needs quotes keeps the quoted absolute path"
        );
    }

    #[test]
    fn the_command_quotes_a_reporter_path_with_spaces() {
        let command = status_line_command(
            Path::new("/home/Jean Dupont/.paneflow/bin/paneflow-ai-hook"),
            None,
        );
        assert_eq!(
            command,
            "'/home/Jean Dupont/.paneflow/bin/paneflow-ai-hook' StatusLine"
        );
        assert!(is_managed_status_line(&json!({ "command": command })));
        assert_eq!(
            status_line_command(Path::new("/home/u/.paneflow/bin/paneflow-ai-hook.sh"), None),
            "/home/u/.paneflow/bin/paneflow-ai-hook.sh StatusLine"
        );
    }
}
