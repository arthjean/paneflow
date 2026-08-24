use super::super::*;
use crate::schema::*;

#[test]
fn test_default_config() {
    let config = PaneFlowConfig::default();
    assert!(config.shortcuts.is_empty());
    assert!(config.default_shell.is_none());
    assert!(config.commands.is_empty());
}

#[test]
fn test_config_path_is_some() {
    // On most systems dirs::config_dir() succeeds. The subdir varies
    // by build profile (`paneflow` in release, `paneflow-dev` in
    // debug -- see `APP_SUBDIR`) so tests assert against the const,
    // not a hardcoded `paneflow` literal.
    let path = config_path();
    assert!(path.is_some());
    let p = path.unwrap();
    let suffix_unix = format!("{APP_SUBDIR}/paneflow.json");
    let suffix_win = format!("{APP_SUBDIR}\\paneflow.json");
    assert!(
        p.ends_with(&suffix_unix) || p.ends_with(&suffix_win),
        "config path {p:?} does not end with {suffix_unix}"
    );
}

#[test]
fn test_missing_file_returns_defaults() {
    let config = load_config_from_path(std::path::Path::new("/nonexistent/path/config.json"));
    assert_eq!(config, PaneFlowConfig::default());
}

#[test]
fn test_invalid_json_returns_defaults() {
    let config = parse_and_validate("this is not json {{{");
    assert_eq!(config, PaneFlowConfig::default());
}

#[test]
fn test_non_object_root_is_a_typed_parse_error() {
    assert!(try_parse_and_validate("[]").is_err());
    assert!(try_parse_and_validate("null").is_err());
    assert!(try_parse_and_validate(r#""text""#).is_err());
}

#[test]
fn test_unknown_terminal_enum_falls_back_not_wipes_config() {
    // Regression guard: a typo in a terminal enum (`"squiggle"`,
    // `"blinky"`) must fall back to that enum's default WITHOUT discarding
    // the rest of the config. Before the custom `Deserialize`, serde hard-
    // errored here and `parse_and_validate` returned `default()` for the
    // whole file -- theme, shell, and shortcuts all silently lost.
    let json = r#"{
        "theme": "One Dark",
        "default_shell": "/bin/zsh",
        "terminal": { "cursor_shape": "squiggle", "cursor_blink": "blinky" }
    }"#;
    let config = parse_and_validate(json);

    // The surrounding config survives the bad enum values.
    assert_eq!(config.theme.as_deref(), Some("One Dark"));
    assert_eq!(config.default_shell.as_deref(), Some("/bin/zsh"));

    // Each unrecognised enum value resolves to its documented default.
    let term = config
        .terminal
        .expect("terminal block must survive unknown enum values");
    assert_eq!(term.cursor_shape, Some(CursorShapeConfig::Block));
    assert_eq!(
        term.cursor_blink,
        Some(CursorBlinkConfig::TerminalControlled)
    );
}

#[test]
fn test_empty_json_object_returns_defaults() {
    let config = parse_and_validate("{}");
    assert_eq!(config, PaneFlowConfig::default());
}

#[test]
fn test_valid_minimal_config() {
    let json = r#"{
        "default_shell": "/bin/zsh",
        "shortcuts": {"ctrl+t": "new_tab"},
        "commands": []
    }"#;
    let config = parse_and_validate(json);
    assert_eq!(config.default_shell, Some("/bin/zsh".to_string()));
    assert_eq!(config.shortcuts.get("ctrl+t"), Some(&"new_tab".to_string()));
    assert!(config.commands.is_empty());
}

#[test]
fn test_blank_name_skipped() {
    let json = r#"{
        "commands": [
            {"name": "", "keywords": []},
            {"name": "  ", "keywords": []},
            {"name": "valid", "keywords": ["test"], "command": "echo valid"}
        ]
    }"#;
    let config = parse_and_validate(json);
    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].name, "valid");
}

#[test]
fn test_malformed_command_entry_does_not_drop_valid_siblings_or_config() {
    let json = r#"{
        "theme": "One Dark",
        "commands": [
            {"description": "missing name", "command": "bad"},
            {"name": "valid", "keywords": ["test"], "command": "echo ok"}
        ]
    }"#;

    let config = parse_and_validate(json);

    assert_eq!(config.theme.as_deref(), Some("One Dark"));
    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].name, "valid");
}

#[test]
fn test_command_requires_exactly_one_payload() {
    let config = parse_and_validate(
        r#"{
            "commands": [
                {"name": "missing"},
                {"name": "both", "command": "echo bad", "workspace": {}},
                {"name": "blank", "command": "   "},
                {"name": "valid", "command": "echo ok"}
            ]
        }"#,
    );
    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].name, "valid");
}

#[test]
fn test_command_with_workspace() {
    let json = r#"{
        "commands": [{
            "name": "dev",
            "description": "Development workspace",
            "keywords": ["dev", "work"],
            "workspace": {
                "name": "Dev Workspace",
                "cwd": "/home/user/projects",
                "color": "ff6600",
                "layout": {
                    "type": "split",
                    "direction": "horizontal",
                    "ratio": 0.5,
                    "children": [
                        {
                            "type": "pane",
                            "surfaces": [{"surface_type": "terminal", "command": "vim"}]
                        },
                        {
                            "type": "pane",
                            "surfaces": [{"surface_type": "terminal", "command": "cargo watch"}]
                        }
                    ]
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    assert_eq!(config.commands.len(), 1);
    let cmd = &config.commands[0];
    assert_eq!(cmd.name, "dev");
    assert_eq!(cmd.description.as_deref(), Some("Development workspace"));

    let ws = cmd.workspace().unwrap();
    assert_eq!(ws.name.as_deref(), Some("Dev Workspace"));
    assert_eq!(ws.color.as_deref(), Some("ff6600"));

    match ws.layout.as_ref().unwrap() {
        LayoutNode::Split {
            direction,
            ratio,
            children,
            ..
        } => {
            assert_eq!(direction, "horizontal");
            assert_eq!(*ratio, Some(0.5));
            assert_eq!(children.len(), 2);
        }
        _ => panic!("expected split layout"),
    }
}

#[test]
fn test_command_with_shell_command() {
    let json = r#"{
        "commands": [{
            "name": "htop",
            "keywords": ["monitor"],
            "command": "htop"
        }]
    }"#;
    let config = parse_and_validate(json);
    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].shell_command(), Some("htop"));
    assert!(config.commands[0].workspace().is_none());
}
