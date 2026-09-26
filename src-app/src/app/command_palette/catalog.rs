use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::{PaneFlowApp, SettingsSection, ThemeMode};

pub(crate) type RunFn = fn(&mut PaneFlowApp, &mut Window, &mut Context<PaneFlowApp>);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Needs {
    Always,
    Terminal,
    TerminalSearch,
    Markdown,
    MarkdownSearch,
    Workspace,
    GitRepo,
    ManyWorkspaces,
    ManyTabs,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    Theme,
    ThemeMode,
    Workspace,
    Tab,
    OpenWorkspaceIn,
    PaneLayout,
    SettingsTab,
    TerminalFont,
    FontSize,
    FontWeight,
    LineHeight,
    CellWidth,
    CursorShape,
    MinimumContrast,
    DefaultShell,
    DefaultEditor,
    OnQuit,
    EndedSessions,
}

pub(crate) enum Kind {
    Action(&'static str),
    Run(RunFn),
    Toggle {
        read: fn(&PaneFlowApp) -> bool,
        write: fn(&mut PaneFlowApp, bool, &mut Context<PaneFlowApp>),
    },
    Scope(Scope),
}

pub(crate) struct Command {
    pub(crate) label: &'static str,
    pub(crate) keywords: &'static str,
    pub(crate) needs: Needs,
    pub(crate) kind: Kind,
}

pub(crate) enum Apply {
    Setting {
        key: &'static str,
        nested: bool,
        value: Value,
    },
    Theme(usize),
    Mode(ThemeMode),
    Workspace(usize),
    Tab(usize),
    Action(&'static str),
    Settings(SettingsSection),
}

impl Apply {
    pub(crate) fn applies_in_place(&self) -> bool {
        match self {
            Apply::Setting { .. } | Apply::Theme(_) | Apply::Mode(_) => true,
            Apply::Workspace(_) | Apply::Tab(_) | Apply::Action(_) | Apply::Settings(_) => false,
        }
    }
}

pub(crate) struct ScopeValue {
    pub(crate) label: String,
    pub(crate) current: bool,
    pub(crate) apply: Apply,
}

impl Scope {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Scope::Theme => "Theme",
            Scope::ThemeMode => "Theme mode",
            Scope::Workspace => "Workspace",
            Scope::Tab => "Tab",
            Scope::OpenWorkspaceIn => "Open in",
            Scope::PaneLayout => "Layout",
            Scope::SettingsTab => "Settings",
            Scope::TerminalFont => "Font",
            Scope::FontSize => "Font size",
            Scope::FontWeight => "Font weight",
            Scope::LineHeight => "Line height",
            Scope::CellWidth => "Cell width",
            Scope::CursorShape => "Cursor shape",
            Scope::MinimumContrast => "Minimum contrast",
            Scope::DefaultShell => "Shell",
            Scope::DefaultEditor => "Editor",
            Scope::OnQuit => "On quit",
            Scope::EndedSessions => "Ended sessions",
        }
    }

    pub(crate) fn placeholder(self) -> String {
        format!("Select {}…", self.title().to_lowercase())
    }

    pub(crate) fn current(self, app: &PaneFlowApp) -> Option<String> {
        self.values(app)
            .into_iter()
            .find(|value| value.current)
            .map(|value| value.label)
    }

    pub(crate) fn values(self, app: &PaneFlowApp) -> Vec<ScopeValue> {
        match self {
            Scope::Theme => theme_values(app),
            Scope::ThemeMode => mode_values(app),
            Scope::Workspace => workspace_values(app),
            Scope::Tab => tab_values(app),
            Scope::OpenWorkspaceIn => action_values(&[
                ("Zed", "open_workspace_in_zed"),
                ("Cursor", "open_workspace_in_cursor"),
                ("VS Code", "open_workspace_in_vscode"),
                ("Windsurf", "open_workspace_in_windsurf"),
                ("File manager", "reveal_workspace_in_file_manager"),
            ]),
            Scope::PaneLayout => action_values(&[
                ("Even horizontal", "layout_even_horizontal"),
                ("Even vertical", "layout_even_vertical"),
                ("Main vertical", "layout_main_vertical"),
                ("Tiled", "layout_tiled"),
            ]),
            Scope::SettingsTab => settings_values(app),
            Scope::TerminalFont => font_values(app),
            Scope::FontSize => font_size_values(app),
            Scope::FontWeight => font_weight_values(app),
            Scope::LineHeight => line_height_values(app),
            Scope::CellWidth => cell_width_values(app),
            Scope::CursorShape => cursor_shape_values(app),
            Scope::MinimumContrast => {
                minimum_contrast_values(&app.cached_config.terminal.clone().unwrap_or_default())
            }
            Scope::DefaultShell => shell_values(app),
            Scope::DefaultEditor => editor_values(app),
            Scope::OnQuit => on_quit_values(app),
            Scope::EndedSessions => ended_sessions_values(app),
        }
    }
}

fn theme_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app.current_theme_preset().name;
    crate::theme::PRESETS
        .iter()
        .enumerate()
        .map(|(idx, preset)| ScopeValue {
            label: preset.name.to_string(),
            current: preset.name == current,
            apply: Apply::Theme(idx),
        })
        .collect()
}

fn mode_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    [
        ("Follow the system", ThemeMode::System),
        ("Light", ThemeMode::Light),
        ("Dark", ThemeMode::Dark),
    ]
    .into_iter()
    .map(|(label, mode)| ScopeValue {
        label: label.to_string(),
        current: app.theme_mode == mode,
        apply: Apply::Mode(mode),
    })
    .collect()
}

fn workspace_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    app.workspaces
        .iter()
        .enumerate()
        .map(|(idx, workspace)| ScopeValue {
            label: if workspace.title.is_empty() {
                workspace.cwd.clone()
            } else {
                workspace.title.clone()
            },
            current: idx == app.active_idx,
            apply: Apply::Workspace(idx),
        })
        .collect()
}

fn tab_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let Some(workspace) = app.active_workspace() else {
        return Vec::new();
    };
    let active = workspace.active_tab_idx();
    workspace
        .tabs()
        .iter()
        .enumerate()
        .map(|(idx, tab)| ScopeValue {
            label: if tab.title().is_empty() {
                format!("Tab {}", idx + 1)
            } else {
                tab.title().to_string()
            },
            current: idx == active,
            apply: Apply::Tab(idx),
        })
        .collect()
}

fn action_values(entries: &[(&str, &'static str)]) -> Vec<ScopeValue> {
    entries
        .iter()
        .map(|(label, action)| ScopeValue {
            label: (*label).to_string(),
            current: false,
            apply: Apply::Action(action),
        })
        .collect()
}

fn settings_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    [
        ("General", SettingsSection::General),
        ("Appearance", SettingsSection::Appearance),
        ("Terminal", SettingsSection::Terminal),
        ("Workspaces", SettingsSection::Workspaces),
        ("Worktrees", SettingsSection::Worktrees),
        ("Agents", SettingsSection::Agents),
        ("MCP servers", SettingsSection::McpServers),
        ("Shortcuts", SettingsSection::Shortcuts),
    ]
    .into_iter()
    .map(|(label, section)| ScopeValue {
        label: label.to_string(),
        current: app.settings_section == Some(section),
        apply: Apply::Settings(section),
    })
    .collect()
}

fn font_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current =
        crate::terminal::element::resolve_font_family(app.cached_config.font_family.as_deref());
    let mut values = vec![ScopeValue {
        label: format!("Paneflow default - {current}"),
        current: app.cached_config.font_family.is_none(),
        apply: Apply::Setting {
            key: "font_family",
            nested: false,
            value: Value::Null,
        },
    }];
    values.extend(app.mono_font_names.iter().map(|name| ScopeValue {
        label: name.clone(),
        current: app.cached_config.font_family.as_deref() == Some(name.as_str()),
        apply: Apply::Setting {
            key: "font_family",
            nested: false,
            value: json!(name),
        },
    }));
    values
}

fn font_size_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app
        .cached_config
        .font_size
        .unwrap_or(crate::terminal::element::DEFAULT_FONT_SIZE);
    (8..=32)
        .map(|size| ScopeValue {
            label: format!("{size} pt"),
            current: (current - size as f32).abs() < 0.01,
            apply: Apply::Setting {
                key: "font_size",
                nested: false,
                value: json!(size),
            },
        })
        .collect()
}

fn font_weight_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = crate::terminal::element::normalize_font_weight_key(
        app.cached_config.font_weight.as_deref(),
    );
    [
        ("Thin", "thin"),
        ("Extra-light", "extra_light"),
        ("Light", "light"),
        ("Semi light", "semi_light"),
        ("Normal", "normal"),
        ("Medium", "medium"),
        ("Semi-bold", "semi_bold"),
        ("Bold", "bold"),
        ("Extra-bold", "extra_bold"),
        ("Black", "black"),
        ("Extra-black", "extra_black"),
    ]
    .into_iter()
    .map(|(label, key)| ScopeValue {
        label: label.to_string(),
        current: key == current,
        apply: Apply::Setting {
            key: "font_weight",
            nested: false,
            value: json!(key),
        },
    })
    .collect()
}

fn steps(from: f32, to: f32, step: f32) -> Vec<f32> {
    let count = ((to - from) / step).round() as i32;
    (0..=count)
        .map(|index| ((from + step * index as f32) * 100.).round() / 100.)
        .collect()
}

fn line_height_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app
        .cached_config
        .line_height
        .unwrap_or(crate::terminal::element::DEFAULT_LINE_HEIGHT);
    steps(1.0, 2.5, 0.1)
        .into_iter()
        .map(|value| ScopeValue {
            label: format!("{value:.1}"),
            current: (current - value).abs() < 0.01,
            apply: Apply::Setting {
                key: "line_height",
                nested: false,
                value: json!(value),
            },
        })
        .collect()
}

fn cell_width_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app
        .cached_config
        .cell_width
        .unwrap_or(crate::terminal::element::DEFAULT_CELL_WIDTH);
    steps(0.3, 2.0, 0.1)
        .into_iter()
        .map(|value| ScopeValue {
            label: format!("{value:.1}"),
            current: (current - value).abs() < 0.01,
            apply: Apply::Setting {
                key: "cell_width",
                nested: false,
                value: json!(value),
            },
        })
        .collect()
}

fn cursor_shape_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app
        .cached_config
        .terminal
        .as_ref()
        .and_then(|terminal| terminal.cursor_shape)
        .unwrap_or_default();
    [
        ("Vintage (_▂)", "vintage"),
        ("Bar (|)", "beam"),
        ("Underline (_)", "underline"),
        ("Double underline (‿)", "double_underline"),
        ("Filled box (█)", "block"),
        ("Empty box (□)", "hollow"),
    ]
    .into_iter()
    .map(|(label, key)| ScopeValue {
        label: label.to_string(),
        current: cursor_shape_key(current) == key,
        apply: Apply::Setting {
            key: "cursor_shape",
            nested: true,
            value: json!(key),
        },
    })
    .collect()
}

fn minimum_contrast_values(terminal: &paneflow_config::schema::TerminalConfig) -> Vec<ScopeValue> {
    use crate::settings::tabs::terminal::{
        MINIMUM_CONTRAST_STEPS, minimum_contrast_setting, minimum_contrast_step,
    };

    let current = minimum_contrast_step(terminal);
    MINIMUM_CONTRAST_STEPS
        .iter()
        .enumerate()
        .map(|(index, (label, _))| ScopeValue {
            label: (*label).to_string(),
            current: index == current,
            apply: Apply::Setting {
                key: "minimum_contrast",
                nested: true,
                value: minimum_contrast_setting(index),
            },
        })
        .collect()
}

fn cursor_shape_key(shape: paneflow_config::schema::CursorShapeConfig) -> &'static str {
    use paneflow_config::schema::CursorShapeConfig;
    match shape {
        CursorShapeConfig::Vintage => "vintage",
        CursorShapeConfig::Beam => "beam",
        CursorShapeConfig::Underline => "underline",
        CursorShapeConfig::DoubleUnderline => "double_underline",
        CursorShapeConfig::Block => "block",
        CursorShapeConfig::Hollow => "hollow",
    }
}

fn shell_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    #[cfg(target_os = "windows")]
    let shells: Vec<(&str, String)> = vec![
        ("PowerShell", "pwsh.exe".to_string()),
        ("Windows PowerShell", "powershell.exe".to_string()),
        ("Command Prompt", "cmd.exe".to_string()),
        (
            "Git Bash",
            crate::terminal::shell::find_windows_git_bash()
                .unwrap_or_else(|| "bash.exe".to_string()),
        ),
    ];
    #[cfg(not(target_os = "windows"))]
    let shells: Vec<(&str, String)> = vec![
        ("zsh", "/bin/zsh".to_string()),
        ("bash", "/bin/bash".to_string()),
        ("sh", "/bin/sh".to_string()),
        ("fish", "/usr/bin/fish".to_string()),
    ];

    let current = app.cached_config.default_shell.clone().unwrap_or_default();
    shells
        .into_iter()
        .map(|(label, path)| ScopeValue {
            label: label.to_string(),
            current: !current.is_empty() && current == path,
            apply: Apply::Setting {
                key: "default_shell",
                nested: false,
                value: json!(path),
            },
        })
        .collect()
}

fn editor_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app
        .cached_config
        .external_editor
        .clone()
        .unwrap_or_else(|| "auto".to_string());
    crate::settings::tabs::general::EDITOR_PRESETS
        .iter()
        .map(|(label, key)| ScopeValue {
            label: (*label).to_string(),
            current: current == *key,
            apply: Apply::Setting {
                key: "external_editor",
                nested: false,
                value: json!(key),
            },
        })
        .collect()
}

fn on_quit_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    use paneflow_config::schema::OnQuit;
    let current = app.cached_config.resolved_on_quit();
    [
        (OnQuit::Ask, "Ask every time"),
        (OnQuit::Keep, "Keep sessions running"),
        (OnQuit::Stop, "Stop everything"),
    ]
    .into_iter()
    .map(|(choice, label)| ScopeValue {
        label: label.to_string(),
        current: choice == current,
        apply: Apply::Setting {
            key: "on_quit",
            nested: false,
            value: json!(choice.wire_str()),
        },
    })
    .collect()
}

fn ended_sessions_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app.cached_config.sidebar_ended_sessions.unwrap_or(3);
    (0..=10u8)
        .map(|cap| ScopeValue {
            label: match cap {
                0 => "None".to_string(),
                1 => "1 session".to_string(),
                other => format!("{other} sessions"),
            },
            current: cap == current,
            apply: Apply::Setting {
                key: "sidebar_ended_sessions",
                nested: false,
                value: json!(cap),
            },
        })
        .collect()
}

fn terminal_bool(
    app: &PaneFlowApp,
    read: fn(&paneflow_config::schema::TerminalConfig) -> bool,
) -> bool {
    read(&app.cached_config.terminal.clone().unwrap_or_default())
}

fn sidebar_show_toggle(
    app: &mut PaneFlowApp,
    field: &'static str,
    next: bool,
    cx: &mut Context<PaneFlowApp>,
) {
    let mut show =
        serde_json::to_value(app.cached_config.sidebar_show).unwrap_or_else(|_| json!({}));
    if let Some(object) = show.as_object_mut() {
        object.insert(field.to_string(), json!(next));
    }
    app.persist_setting(false, "sidebar_show", show, cx);
}

fn set_all_expanded(app: &mut PaneFlowApp, expanded: bool, cx: &mut Context<PaneFlowApp>) {
    for workspace in &mut app.workspaces {
        workspace.sidebar_expanded = expanded;
    }
    app.save_session(cx);
    cx.notify();
}

pub(crate) const COMMANDS: &[Command] = &[
    Command {
        label: "Split vertical",
        keywords: "pane divide right",
        needs: Needs::Workspace,
        kind: Kind::Action("split_vertically"),
    },
    Command {
        label: "Split horizontal",
        keywords: "pane divide down",
        needs: Needs::Workspace,
        kind: Kind::Action("split_horizontally"),
    },
    Command {
        label: "Toggle zoom",
        keywords: "pane fullscreen maximize",
        needs: Needs::Workspace,
        kind: Kind::Action("toggle_zoom"),
    },
    Command {
        label: "Swap pane",
        keywords: "move exchange",
        needs: Needs::Workspace,
        kind: Kind::Action("swap_pane"),
    },
    Command {
        label: "Equalize panes",
        keywords: "balance even sizes",
        needs: Needs::Workspace,
        kind: Kind::Action("split_equalize"),
    },
    Command {
        label: "Pane layout",
        keywords: "arrange tiled grid",
        needs: Needs::Workspace,
        kind: Kind::Scope(Scope::PaneLayout),
    },
    Command {
        label: "Focus left",
        keywords: "pane move",
        needs: Needs::Workspace,
        kind: Kind::Action("focus_left"),
    },
    Command {
        label: "Focus right",
        keywords: "pane move",
        needs: Needs::Workspace,
        kind: Kind::Action("focus_right"),
    },
    Command {
        label: "Focus up",
        keywords: "pane move",
        needs: Needs::Workspace,
        kind: Kind::Action("focus_up"),
    },
    Command {
        label: "Focus down",
        keywords: "pane move",
        needs: Needs::Workspace,
        kind: Kind::Action("focus_down"),
    },
    Command {
        label: "Detach pane into a window",
        keywords: "float window return",
        needs: Needs::Workspace,
        kind: Kind::Action("toggle_detached_pane"),
    },
    Command {
        label: "Hide pane from the layout",
        keywords: "keep session running",
        needs: Needs::Workspace,
        kind: Kind::Action("hide_pane"),
    },
    Command {
        label: "Undo close pane",
        keywords: "restore reopen",
        needs: Needs::Workspace,
        kind: Kind::Action("undo_close_pane"),
    },
    Command {
        label: "Stop the session of this pane",
        keywords: "kill process end",
        needs: Needs::Workspace,
        kind: Kind::Action("stop_session"),
    },
    Command {
        label: "Close pane",
        keywords: "remove stop sessions",
        needs: Needs::Workspace,
        kind: Kind::Action("close_pane"),
    },
    Command {
        label: "New tab",
        keywords: "create",
        needs: Needs::Workspace,
        kind: Kind::Action("new_tab"),
    },
    Command {
        label: "Switch tab",
        keywords: "go to change",
        needs: Needs::ManyTabs,
        kind: Kind::Scope(Scope::Tab),
    },
    Command {
        label: "Next tab",
        keywords: "cycle",
        needs: Needs::ManyTabs,
        kind: Kind::Action("next_tab"),
    },
    Command {
        label: "Previous tab",
        keywords: "cycle",
        needs: Needs::ManyTabs,
        kind: Kind::Action("previous_tab"),
    },
    Command {
        label: "Close tab",
        keywords: "remove stop sessions",
        needs: Needs::Workspace,
        kind: Kind::Action("close_tab"),
    },
    Command {
        label: "New workspace",
        keywords: "project folder open",
        needs: Needs::Always,
        kind: Kind::Action("new_workspace"),
    },
    Command {
        label: "Switch workspace",
        keywords: "go to change project",
        needs: Needs::ManyWorkspaces,
        kind: Kind::Scope(Scope::Workspace),
    },
    Command {
        label: "Next workspace",
        keywords: "cycle",
        needs: Needs::ManyWorkspaces,
        kind: Kind::Action("next_workspace"),
    },
    Command {
        label: "Clone repository…",
        keywords: "git checkout download",
        needs: Needs::Always,
        kind: Kind::Action("clone_repository"),
    },
    Command {
        label: "Open workspace in",
        keywords: "editor zed cursor vscode windsurf file manager reveal",
        needs: Needs::Workspace,
        kind: Kind::Scope(Scope::OpenWorkspaceIn),
    },
    Command {
        label: "Copy workspace path",
        keywords: "clipboard folder",
        needs: Needs::Workspace,
        kind: Kind::Action("copy_workspace_path"),
    },
    Command {
        label: "Resume every ended session",
        keywords: "restart workspace",
        needs: Needs::Workspace,
        kind: Kind::Action("resume_ended_sessions"),
    },
    Command {
        label: "Remove every ended session",
        keywords: "clean list workspace",
        needs: Needs::Workspace,
        kind: Kind::Action("remove_ended_sessions"),
    },
    Command {
        label: "Close workspace",
        keywords: "stop sessions quit project",
        needs: Needs::Workspace,
        kind: Kind::Action("close_workspace"),
    },
    Command {
        label: "Mute this workspace's notifications",
        keywords: "silence agent alerts",
        needs: Needs::Workspace,
        kind: Kind::Toggle {
            read: |app| {
                app.active_workspace()
                    .is_some_and(|workspace| workspace.muted)
            },
            write: |app, _next, cx| {
                let active = app.active_idx;
                app.toggle_workspace_muted(active, cx);
            },
        },
    },
    Command {
        label: "Mark this workspace as read",
        keywords: "notification acknowledge agent",
        needs: Needs::Workspace,
        kind: Kind::Run(|app, _window, cx| {
            let active = app.active_idx;
            app.mark_workspace_read(active, cx);
        }),
    },
    Command {
        label: "Expand every workspace in the sidebar",
        keywords: "tree unfold show sessions",
        needs: Needs::Always,
        kind: Kind::Run(|app, _window, cx| set_all_expanded(app, true, cx)),
    },
    Command {
        label: "Collapse every workspace in the sidebar",
        keywords: "tree fold hide sessions",
        needs: Needs::Always,
        kind: Kind::Run(|app, _window, cx| set_all_expanded(app, false, cx)),
    },
    Command {
        label: "Prompt composer…",
        keywords: "agent write send",
        needs: Needs::Workspace,
        kind: Kind::Action("open_composer"),
    },
    Command {
        label: "Attention queue…",
        keywords: "agent waiting review",
        needs: Needs::Always,
        kind: Kind::Action("open_attention_queue"),
    },
    Command {
        label: "Broadcast groups…",
        keywords: "agent fanout send many",
        needs: Needs::Always,
        kind: Kind::Action("open_broadcast_groups"),
    },
    Command {
        label: "Toggle pane in broadcast group",
        keywords: "agent fanout member",
        needs: Needs::Workspace,
        kind: Kind::Action("toggle_broadcast_member"),
    },
    Command {
        label: "Jump to the next waiting agent",
        keywords: "attention review",
        needs: Needs::Always,
        kind: Kind::Action("jump_next_waiting"),
    },
    Command {
        label: "Focus Workspaces sidebar",
        keywords: "keyboard navigate rail tabs",
        needs: Needs::Always,
        kind: Kind::Action("focus_workspaces_sidebar"),
    },
    Command {
        label: "Toggle Files sidebar",
        keywords: "tree explorer",
        needs: Needs::Workspace,
        kind: Kind::Action("toggle_files_sidebar"),
    },
    Command {
        label: "Maximize or restore the Changes dock",
        keywords: "diff git review",
        needs: Needs::Workspace,
        kind: Kind::Action("toggle_diff_dock_maximize"),
    },
    Command {
        label: "Changes dock: open a file tab",
        keywords: "diff git editor",
        needs: Needs::GitRepo,
        kind: Kind::Action("diff_new_file_tab"),
    },
    Command {
        label: "Changes dock: open a terminal tab",
        keywords: "diff git shell",
        needs: Needs::GitRepo,
        kind: Kind::Action("diff_new_terminal_tab"),
    },
    Command {
        label: "Copy",
        keywords: "clipboard terminal selection",
        needs: Needs::Terminal,
        kind: Kind::Action("terminal_copy"),
    },
    Command {
        label: "Paste",
        keywords: "clipboard terminal insert",
        needs: Needs::Terminal,
        kind: Kind::Action("terminal_paste"),
    },
    Command {
        label: "Select all",
        keywords: "clipboard terminal",
        needs: Needs::Terminal,
        kind: Kind::Action("terminal_select_all"),
    },
    Command {
        label: "Toggle copy mode",
        keywords: "terminal keyboard selection vim",
        needs: Needs::Terminal,
        kind: Kind::Action("toggle_copy_mode"),
    },
    Command {
        label: "Find in this pane",
        keywords: "search terminal",
        needs: Needs::Terminal,
        kind: Kind::Action("toggle_search"),
    },
    Command {
        label: "Search across all panes",
        keywords: "fleet find everywhere",
        needs: Needs::Terminal,
        kind: Kind::Action("toggle_fleet_search"),
    },
    Command {
        label: "Search next match",
        keywords: "find forward",
        needs: Needs::TerminalSearch,
        kind: Kind::Action("search_next"),
    },
    Command {
        label: "Search previous match",
        keywords: "find backward",
        needs: Needs::TerminalSearch,
        kind: Kind::Action("search_prev"),
    },
    Command {
        label: "Toggle search regex",
        keywords: "find pattern",
        needs: Needs::TerminalSearch,
        kind: Kind::Action("toggle_search_regex"),
    },
    Command {
        label: "Dismiss search",
        keywords: "find close",
        needs: Needs::TerminalSearch,
        kind: Kind::Action("dismiss_search"),
    },
    Command {
        label: "Scroll up one page",
        keywords: "terminal scrollback",
        needs: Needs::Terminal,
        kind: Kind::Action("scroll_page_up"),
    },
    Command {
        label: "Scroll down one page",
        keywords: "terminal scrollback",
        needs: Needs::Terminal,
        kind: Kind::Action("scroll_page_down"),
    },
    Command {
        label: "Jump to the previous prompt",
        keywords: "terminal shell integration mark",
        needs: Needs::Terminal,
        kind: Kind::Action("jump_prev_prompt"),
    },
    Command {
        label: "Jump to the next prompt",
        keywords: "terminal shell integration mark",
        needs: Needs::Terminal,
        kind: Kind::Action("jump_next_prompt"),
    },
    Command {
        label: "Increase pane font size",
        keywords: "terminal zoom bigger",
        needs: Needs::Terminal,
        kind: Kind::Action("font_size_increase"),
    },
    Command {
        label: "Decrease pane font size",
        keywords: "terminal zoom smaller",
        needs: Needs::Terminal,
        kind: Kind::Action("font_size_decrease"),
    },
    Command {
        label: "Reset pane font size",
        keywords: "terminal zoom default",
        needs: Needs::Terminal,
        kind: Kind::Action("font_size_reset"),
    },
    Command {
        label: "Clear scroll history",
        keywords: "terminal scrollback wipe",
        needs: Needs::Terminal,
        kind: Kind::Action("clear_scroll_history"),
    },
    Command {
        label: "Reset terminal",
        keywords: "fix garbled escape",
        needs: Needs::Terminal,
        kind: Kind::Action("reset_terminal"),
    },
    Command {
        label: "Markdown: scroll up one page",
        keywords: "preview document",
        needs: Needs::Markdown,
        kind: Kind::Action("markdown_scroll_page_up"),
    },
    Command {
        label: "Markdown: scroll down one page",
        keywords: "preview document",
        needs: Needs::Markdown,
        kind: Kind::Action("markdown_scroll_page_down"),
    },
    Command {
        label: "Markdown: open the find bar",
        keywords: "preview search document",
        needs: Needs::Markdown,
        kind: Kind::Action("markdown_find_open"),
    },
    Command {
        label: "Markdown: copy the selection",
        keywords: "preview clipboard",
        needs: Needs::Markdown,
        kind: Kind::Action("markdown_copy"),
    },
    Command {
        label: "Markdown: jump to the next match",
        keywords: "preview search forward",
        needs: Needs::MarkdownSearch,
        kind: Kind::Action("markdown_find_next"),
    },
    Command {
        label: "Markdown: jump to the previous match",
        keywords: "preview search backward",
        needs: Needs::MarkdownSearch,
        kind: Kind::Action("markdown_find_prev"),
    },
    Command {
        label: "Markdown: close the find bar",
        keywords: "preview search dismiss",
        needs: Needs::MarkdownSearch,
        kind: Kind::Action("markdown_find_dismiss"),
    },
    Command {
        label: "Theme",
        keywords: "color palette appearance dark light",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::Theme),
    },
    Command {
        label: "Theme mode",
        keywords: "appearance dark light system",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::ThemeMode),
    },
    Command {
        label: "Terminal font",
        keywords: "typeface family appearance",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::TerminalFont),
    },
    Command {
        label: "Terminal font size",
        keywords: "typeface appearance points",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::FontSize),
    },
    Command {
        label: "Terminal font weight",
        keywords: "typeface appearance bold",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::FontWeight),
    },
    Command {
        label: "Terminal line height",
        keywords: "typeface appearance spacing",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::LineHeight),
    },
    Command {
        label: "Terminal cell width",
        keywords: "typeface appearance spacing",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::CellWidth),
    },
    Command {
        label: "Cursor shape",
        keywords: "appearance caret block beam",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::CursorShape),
    },
    Command {
        label: "Minimum contrast",
        keywords: "legibility readability apca light theme accessibility",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::MinimumContrast),
    },
    Command {
        label: "Integrated glyphs",
        keywords: "appearance powerline box drawing",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| terminal_bool(app, |terminal| terminal.resolved_integrated_glyphs()),
            write: |app, next, cx| app.persist_setting(true, "integrated_glyphs", json!(next), cx),
        },
    },
    Command {
        label: "Color emoji",
        keywords: "appearance glyphs",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| terminal_bool(app, |terminal| terminal.resolved_color_emoji()),
            write: |app, next, cx| app.persist_setting(true, "color_emoji", json!(next), cx),
        },
    },
    Command {
        label: "Terminal scrollbar",
        keywords: "appearance scroll",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| terminal_bool(app, |terminal| terminal.resolved_scrollbar_visible()),
            write: |app, next, cx| app.persist_setting(true, "scrollbar", json!(next), cx),
        },
    },
    Command {
        label: "Reduce motion",
        keywords: "appearance animation accessibility",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.reduce_motion.unwrap_or(false),
            write: |app, next, cx| app.persist_setting(false, "reduce_motion", json!(next), cx),
        },
    },
    Command {
        label: "Show the branch in the sidebar",
        keywords: "git appearance",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.branch.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "branch", next, cx),
        },
    },
    Command {
        label: "Show the diffstat in the sidebar",
        keywords: "git appearance changes",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.diffstat.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "diffstat", next, cx),
        },
    },
    Command {
        label: "Show pull requests in the sidebar",
        keywords: "git appearance github",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.pr.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "pr", next, cx),
        },
    },
    Command {
        label: "Show the indent guide in the sidebar",
        keywords: "appearance tree",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.indent_guide.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "indent_guide", next, cx),
        },
    },
    Command {
        label: "Default editor",
        keywords: "external open zed cursor vscode",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::DefaultEditor),
    },
    Command {
        label: "Shell in the integrated terminal",
        keywords: "bash zsh fish powershell default",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::DefaultShell),
    },
    Command {
        label: "When quitting with sessions running",
        keywords: "exit ask keep stop",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::OnQuit),
    },
    Command {
        label: "Ended sessions listed per workspace",
        keywords: "sidebar history limit",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::EndedSessions),
    },
    Command {
        label: "Native OS notifications",
        keywords: "alert agent waiting",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| {
                app.cached_config.agent_panel.as_ref().is_some_and(|panel| {
                    panel.resolved_notify_when_agent_waiting()
                        != paneflow_config::schema::NotifyWhenAgentWaiting::Never
                })
            },
            write: |app, next, cx| {
                let value = if next { "PrimaryScreen" } else { "Never" };
                app.persist_agent_panel_setting("notify_when_agent_waiting", json!(value), cx);
            },
        },
    },
    Command {
        label: "Remove old worktrees automatically",
        keywords: "git cleanup prune",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.worktrees.auto_remove.unwrap_or(false),
            write: |app, next, cx| {
                let mut worktrees = serde_json::to_value(app.cached_config.worktrees.clone())
                    .unwrap_or_else(|_| json!({}));
                if let Some(object) = worktrees.as_object_mut() {
                    object.insert("auto_remove".to_string(), json!(next));
                }
                app.persist_setting(false, "worktrees", worktrees, cx);
            },
        },
    },
    Command {
        label: "AI free access",
        keywords: "agent permissions unrestricted",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.ai_unrestricted.unwrap_or(false),
            write: |app, next, cx| app.persist_setting(false, "ai_unrestricted", json!(next), cx),
        },
    },
    Command {
        label: "Injection fence",
        keywords: "agent security prompt",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| app.cached_config.ai_injection_fence.unwrap_or(true),
            write: |app, next, cx| {
                app.persist_setting(false, "ai_injection_fence", json!(next), cx)
            },
        },
    },
    Command {
        label: "Full access for Claude Code",
        keywords: "agent permissions bypass",
        needs: Needs::Always,
        kind: Kind::Toggle {
            read: |app| {
                app.cached_config
                    .claude_code_bypass_permissions
                    .unwrap_or(false)
            },
            write: |app, next, cx| {
                app.persist_setting(false, "claude_code_bypass_permissions", json!(next), cx)
            },
        },
    },
    Command {
        label: "Settings",
        keywords: "preferences options configure",
        needs: Needs::Always,
        kind: Kind::Scope(Scope::SettingsTab),
    },
    Command {
        label: "Check for updates",
        keywords: "version upgrade release",
        needs: Needs::Always,
        kind: Kind::Action("check_for_updates"),
    },
    Command {
        label: "Paneflow documentation",
        keywords: "help docs manual",
        needs: Needs::Always,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::DOCUMENTATION_URL, cx)
        }),
    },
    Command {
        label: "What's new",
        keywords: "help releases changelog",
        needs: Needs::Always,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::RELEASES_URL, cx)
        }),
    },
    Command {
        label: "Automations",
        keywords: "help scripting cli mcp",
        needs: Needs::Always,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::AUTOMATIONS_URL, cx)
        }),
    },
    Command {
        label: "Troubleshooting",
        keywords: "help support debug",
        needs: Needs::Always,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::TROUBLESHOOTING_URL, cx)
        }),
    },
    Command {
        label: "System info…",
        keywords: "help diagnostics version gpu",
        needs: Needs::Always,
        kind: Kind::Run(|app, window, cx| app.open_system_info_dialog(window, cx)),
    },
    Command {
        label: "About Paneflow",
        keywords: "help version license",
        needs: Needs::Always,
        kind: Kind::Run(|app, window, cx| app.open_about_dialog(window, cx)),
    },
    Command {
        label: "Quit Paneflow",
        keywords: "exit close application",
        needs: Needs::Always,
        kind: Kind::Action("quit"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_action_exists_in_the_registry() {
        for command in COMMANDS {
            if let Kind::Action(name) = command.kind {
                assert!(
                    crate::keybindings::action_for_name(name).is_some(),
                    "{name} is not a registered action"
                );
            }
        }
    }

    #[test]
    fn every_scope_is_reachable_from_a_command() {
        let scopes = [
            Scope::Theme,
            Scope::ThemeMode,
            Scope::Workspace,
            Scope::Tab,
            Scope::OpenWorkspaceIn,
            Scope::PaneLayout,
            Scope::SettingsTab,
            Scope::TerminalFont,
            Scope::FontSize,
            Scope::FontWeight,
            Scope::LineHeight,
            Scope::CellWidth,
            Scope::CursorShape,
            Scope::MinimumContrast,
            Scope::DefaultShell,
            Scope::DefaultEditor,
            Scope::OnQuit,
            Scope::EndedSessions,
        ];
        for scope in scopes {
            assert!(
                COMMANDS
                    .iter()
                    .any(|command| matches!(command.kind, Kind::Scope(other) if other == scope)),
                "{:?} has no command",
                scope
            );
        }
    }

    #[test]
    fn labels_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for command in COMMANDS {
            assert!(
                seen.insert(command.label),
                "duplicate label {}",
                command.label
            );
        }
    }

    #[test]
    fn only_the_preference_values_apply_in_place() {
        assert!(
            Apply::Setting {
                key: "font_size",
                nested: false,
                value: json!(14),
            }
            .applies_in_place()
        );
        assert!(Apply::Theme(0).applies_in_place());
        assert!(Apply::Mode(ThemeMode::Dark).applies_in_place());
        assert!(!Apply::Workspace(0).applies_in_place());
        assert!(!Apply::Tab(0).applies_in_place());
        assert!(!Apply::Action("split_vertically").applies_in_place());
        assert!(!Apply::Settings(SettingsSection::General).applies_in_place());
    }

    #[test]
    fn the_minimum_contrast_scope_marks_auto_and_applies_in_place() {
        use paneflow_config::schema::TerminalConfig;

        let unset = minimum_contrast_values(&TerminalConfig::default());
        assert_eq!(unset.len(), 6);
        let marked: Vec<&str> = unset
            .iter()
            .filter(|value| value.current)
            .map(|value| value.label.as_str())
            .collect();
        assert_eq!(marked, vec!["Auto"]);
        assert!(unset[0].apply.applies_in_place());
        assert!(matches!(
            unset[0].apply,
            Apply::Setting {
                key: "minimum_contrast",
                nested: true,
                value: Value::Null,
            }
        ));
        assert!(matches!(
            unset[1].apply,
            Apply::Setting {
                key: "minimum_contrast",
                nested: true,
                ..
            }
        ));

        let explicit = minimum_contrast_values(&TerminalConfig {
            minimum_contrast: Some(75.0),
            ..TerminalConfig::default()
        });
        let marked: Vec<&str> = explicit
            .iter()
            .filter(|value| value.current)
            .map(|value| value.label.as_str())
            .collect();
        assert_eq!(marked, vec!["75"]);
    }

    #[test]
    fn steps_walk_the_whole_range() {
        let values = steps(1.0, 2.5, 0.1);
        assert_eq!(values.first().copied(), Some(1.0));
        assert_eq!(values.last().copied(), Some(2.5));
        assert_eq!(values.len(), 16);
    }
}
