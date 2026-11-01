use gpui::{Context, Window};
use paneflow_config::schema::InterfaceStyle;
use serde_json::{Value, json};

use paneflow_config::schema::SpaceId;

use crate::{PaneFlowApp, SettingsSection, ThemeMode};

pub(crate) type RunFn = fn(&mut PaneFlowApp, &mut Window, &mut Context<PaneFlowApp>);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Needs {
    Always,
    Terminal,
    TerminalSearch,
    ForkableConversation,
    ConversationResumeOffer,
    ConversationFailure,
    ConversationNotice,
    WriteRequest,
    Markdown,
    MarkdownSearch,
    Workspace,
    GitRepo,
    ManyWorkspaces,
    Spaces,
    ManyTabs,
    NativeMaterial,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Scope {
    Theme,
    ThemeMode,
    InterfaceStyle,
    Workspace,
    Space,
    MoveToSpace,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Section {
    Panes,
    Tabs,
    Workspaces,
    Sidebar,
    Terminal,
    Markdown,
    Agents,
    Appearance,
    Paneflow,
}

impl Section {
    pub(crate) const ALL: [Section; 9] = [
        Section::Panes,
        Section::Tabs,
        Section::Workspaces,
        Section::Sidebar,
        Section::Terminal,
        Section::Markdown,
        Section::Agents,
        Section::Appearance,
        Section::Paneflow,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Section::Panes => "Panes",
            Section::Tabs => "Tabs",
            Section::Workspaces => "Workspaces",
            Section::Sidebar => "Sidebar",
            Section::Terminal => "Terminal",
            Section::Markdown => "Markdown",
            Section::Agents => "Agents",
            Section::Appearance => "Appearance",
            Section::Paneflow => "Paneflow",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Section::Panes => "icons/palette/dashboard-square-01.svg",
            Section::Tabs => "icons/palette/browser.svg",
            Section::Workspaces => "icons/palette/folder-01.svg",
            Section::Sidebar => "icons/palette/sidebar-left.svg",
            Section::Terminal => "icons/palette/computer-terminal-02.svg",
            Section::Markdown => "icons/palette/file-01.svg",
            Section::Agents => "icons/palette/ai-magic.svg",
            Section::Appearance => "icons/palette/paint-brush-01.svg",
            Section::Paneflow => "icons/palette/settings-02.svg",
        }
    }

    pub(crate) fn placeholder(self) -> String {
        format!("Search {}…", self.title().to_lowercase())
    }

    pub(crate) fn commands(self) -> impl Iterator<Item = (usize, &'static Command)> {
        COMMANDS
            .iter()
            .enumerate()
            .filter(move |(_, command)| command.section == self)
    }
}

impl Needs {
    pub(crate) fn is_contextual(self) -> bool {
        matches!(
            self,
            Needs::TerminalSearch
                | Needs::ConversationResumeOffer
                | Needs::ConversationFailure
                | Needs::ConversationNotice
                | Needs::WriteRequest
                | Needs::MarkdownSearch
        )
    }
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
    pub(crate) icon: Option<&'static str>,
    pub(crate) keywords: &'static str,
    pub(crate) needs: Needs,
    pub(crate) section: Section,
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
    InterfaceStyle(InterfaceStyle),
    Workspace(usize),
    Space(Option<SpaceId>),
    MoveToSpace(Option<SpaceId>),
    MoveToNewSpace,
    Tab(usize),
    Action(&'static str),
    Settings(SettingsSection),
}

impl Apply {
    pub(crate) fn applies_in_place(&self) -> bool {
        match self {
            Apply::Setting { .. } | Apply::Theme(_) | Apply::Mode(_) | Apply::InterfaceStyle(_) => {
                true
            }
            Apply::Workspace(_)
            | Apply::Space(_)
            | Apply::MoveToSpace(_)
            | Apply::MoveToNewSpace
            | Apply::Tab(_)
            | Apply::Action(_)
            | Apply::Settings(_) => false,
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
            Scope::InterfaceStyle => "Interface style",
            Scope::Workspace => "Workspace",
            Scope::Space => "Space",
            Scope::MoveToSpace => "Destination space",
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
            Scope::InterfaceStyle => interface_style_values(app),
            Scope::Workspace => workspace_values(app),
            Scope::Space => space_values(app),
            Scope::MoveToSpace => move_to_space_values(app),
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

fn interface_style_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let current = app.cached_config.interface_style();
    InterfaceStyle::ALL
        .iter()
        .map(|&style| ScopeValue {
            label: style.label().to_string(),
            current: current == Some(style),
            apply: Apply::InterfaceStyle(style),
        })
        .collect()
}

fn workspace_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let shown = app.shown_workspace_idx();
    palette_workspace_order(
        &app.workspaces,
        &app.cached_config,
        app.active_space.as_ref(),
    )
    .into_iter()
    .map(|idx| {
        let workspace = &app.workspaces[idx];
        let title = if workspace.title.is_empty() {
            workspace.cwd.clone()
        } else {
            workspace.title.clone()
        };
        let label = if workspace.space == app.active_space {
            title
        } else {
            format!(
                "{} · {title}",
                crate::app::spaces::space_label(&app.cached_config, workspace.space.as_ref())
            )
        };
        ScopeValue {
            label,
            current: Some(idx) == shown,
            apply: Apply::Workspace(idx),
        }
    })
    .collect()
}

pub(crate) fn palette_workspace_order(
    workspaces: &[crate::workspace::Workspace],
    config: &paneflow_config::schema::PaneFlowConfig,
    active: Option<&SpaceId>,
) -> Vec<usize> {
    if config.spaces.is_empty() {
        return (0..workspaces.len()).collect();
    }
    let mut order = PaneFlowApp::compute_display_order(workspaces, active);
    for space in crate::app::spaces::space_order(config) {
        if space.as_ref() != active {
            order.extend(PaneFlowApp::compute_display_order(
                workspaces,
                space.as_ref(),
            ));
        }
    }
    order
}

fn space_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    crate::app::spaces::space_order(&app.cached_config)
        .into_iter()
        .map(|space| ScopeValue {
            label: crate::app::spaces::space_label(&app.cached_config, space.as_ref()).to_string(),
            current: space == app.active_space,
            apply: Apply::Space(space),
        })
        .collect()
}

fn move_to_space_values(app: &PaneFlowApp) -> Vec<ScopeValue> {
    let Some(workspace) = app.active_workspace() else {
        return Vec::new();
    };
    let mut values: Vec<ScopeValue> = crate::app::spaces::space_order(&app.cached_config)
        .into_iter()
        .filter(|space| *space != workspace.space)
        .map(|space| ScopeValue {
            label: crate::app::spaces::space_label(&app.cached_config, space.as_ref()).to_string(),
            current: false,
            apply: Apply::MoveToSpace(space),
        })
        .collect();
    if app.cached_config.spaces.len() < paneflow_config::schema::MAX_SPACES {
        values.push(ScopeValue {
            label: "New space…".to_string(),
            current: false,
            apply: Apply::MoveToNewSpace,
        });
    }
    values
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
        ("Spaces", SettingsSection::Spaces),
        ("Worktrees", SettingsSection::Worktrees),
        ("Agents", SettingsSection::Agents),
        ("Usage", SettingsSection::Usage),
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
        icon: Some("icons/palette/layout-two-column.svg"),
        keywords: "pane divide right",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("split_vertically"),
    },
    Command {
        label: "Split horizontal",
        icon: Some("icons/palette/layout-two-row.svg"),
        keywords: "pane divide down",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("split_horizontally"),
    },
    Command {
        label: "Equalize panes",
        icon: Some("icons/palette/layout-three-column.svg"),
        keywords: "balance even sizes",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("split_equalize"),
    },
    Command {
        label: "Pane layout",
        icon: Some("icons/palette/layout-dashboard.svg"),
        keywords: "arrange tiled grid",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Scope(Scope::PaneLayout),
    },
    Command {
        label: "Focus left",
        icon: Some("icons/palette/arrow-left-02.svg"),
        keywords: "pane move",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("focus_left"),
    },
    Command {
        label: "Focus right",
        icon: Some("icons/palette/arrow-right-02.svg"),
        keywords: "pane move",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("focus_right"),
    },
    Command {
        label: "Focus up",
        icon: Some("icons/palette/arrow-up-02.svg"),
        keywords: "pane move",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("focus_up"),
    },
    Command {
        label: "Focus down",
        icon: Some("icons/palette/arrow-down-02.svg"),
        keywords: "pane move",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("focus_down"),
    },
    Command {
        label: "Detach pane into a window",
        icon: Some("icons/palette/link-square-02.svg"),
        keywords: "float window return",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("toggle_detached_pane"),
    },
    Command {
        label: "Close pane",
        icon: Some("icons/palette/cancel-square.svg"),
        keywords: "remove stop sessions",
        needs: Needs::Workspace,
        section: Section::Panes,
        kind: Kind::Action("close_pane"),
    },
    Command {
        label: "New tab",
        icon: Some("icons/palette/add-01.svg"),
        keywords: "create",
        needs: Needs::Workspace,
        section: Section::Tabs,
        kind: Kind::Action("new_tab"),
    },
    Command {
        label: "Switch tab",
        icon: Some("icons/palette/arrow-data-transfer-horizontal.svg"),
        keywords: "go to change",
        needs: Needs::ManyTabs,
        section: Section::Tabs,
        kind: Kind::Scope(Scope::Tab),
    },
    Command {
        label: "Next tab",
        icon: Some("icons/palette/circle-arrow-right-02.svg"),
        keywords: "cycle",
        needs: Needs::ManyTabs,
        section: Section::Tabs,
        kind: Kind::Action("next_tab"),
    },
    Command {
        label: "Previous tab",
        icon: Some("icons/palette/circle-arrow-left-02.svg"),
        keywords: "cycle",
        needs: Needs::ManyTabs,
        section: Section::Tabs,
        kind: Kind::Action("previous_tab"),
    },
    Command {
        label: "Close tab",
        icon: Some("icons/palette/cancel-01.svg"),
        keywords: "remove stop sessions",
        needs: Needs::Workspace,
        section: Section::Tabs,
        kind: Kind::Action("close_tab"),
    },
    Command {
        label: "New workspace",
        icon: Some("icons/palette/folder-add.svg"),
        keywords: "project folder open",
        needs: Needs::Always,
        section: Section::Workspaces,
        kind: Kind::Action("new_workspace"),
    },
    Command {
        label: "Switch workspace",
        icon: Some("icons/palette/folders.svg"),
        keywords: "go to change project",
        needs: Needs::ManyWorkspaces,
        section: Section::Workspaces,
        kind: Kind::Scope(Scope::Workspace),
    },
    Command {
        label: "Next workspace",
        icon: Some("icons/palette/folder-export.svg"),
        keywords: "cycle",
        needs: Needs::ManyWorkspaces,
        section: Section::Workspaces,
        kind: Kind::Action("next_workspace"),
    },
    Command {
        label: "Switch space",
        icon: Some("icons/palette/archive-02.svg"),
        keywords: "go to change context work personal",
        needs: Needs::Spaces,
        section: Section::Workspaces,
        kind: Kind::Scope(Scope::Space),
    },
    Command {
        label: "Next space",
        icon: Some("icons/palette/arrow-right-double.svg"),
        keywords: "cycle context",
        needs: Needs::Spaces,
        section: Section::Workspaces,
        kind: Kind::Action("next_space"),
    },
    Command {
        label: "Previous space",
        icon: Some("icons/palette/arrow-left-double.svg"),
        keywords: "cycle context",
        needs: Needs::Spaces,
        section: Section::Workspaces,
        kind: Kind::Action("previous_space"),
    },
    Command {
        label: "New space",
        icon: Some("icons/palette/add-square.svg"),
        keywords: "create context group work personal",
        needs: Needs::Always,
        section: Section::Workspaces,
        kind: Kind::Action("new_space"),
    },
    Command {
        label: "Move workspace to space",
        icon: Some("icons/palette/folder-move-to.svg"),
        keywords: "context group organize",
        needs: Needs::Spaces,
        section: Section::Workspaces,
        kind: Kind::Scope(Scope::MoveToSpace),
    },
    Command {
        label: "Clone repository…",
        icon: Some("icons/palette/folder-git-2.svg"),
        keywords: "git checkout download",
        needs: Needs::Always,
        section: Section::Workspaces,
        kind: Kind::Action("clone_repository"),
    },
    Command {
        label: "Open workspace in",
        icon: Some("icons/palette/square-arrow-up-right.svg"),
        keywords: "editor zed cursor vscode windsurf file manager reveal",
        needs: Needs::Workspace,
        section: Section::Workspaces,
        kind: Kind::Scope(Scope::OpenWorkspaceIn),
    },
    Command {
        label: "Copy workspace path",
        icon: Some("icons/palette/copy-01.svg"),
        keywords: "clipboard folder",
        needs: Needs::Workspace,
        section: Section::Workspaces,
        kind: Kind::Action("copy_workspace_path"),
    },
    Command {
        label: "Close workspace",
        icon: Some("icons/palette/folder-remove.svg"),
        keywords: "stop sessions quit project",
        needs: Needs::Workspace,
        section: Section::Workspaces,
        kind: Kind::Action("close_workspace"),
    },
    Command {
        label: "Expand every workspace in the sidebar",
        icon: Some("icons/palette/unfold-more.svg"),
        keywords: "tree unfold show sessions",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Run(|app, _window, cx| set_all_expanded(app, true, cx)),
    },
    Command {
        label: "Collapse every workspace in the sidebar",
        icon: Some("icons/palette/unfold-less.svg"),
        keywords: "tree fold hide sessions",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Run(|app, _window, cx| set_all_expanded(app, false, cx)),
    },
    Command {
        label: "Focus Workspaces sidebar",
        icon: Some("icons/palette/sidebar-left-01.svg"),
        keywords: "keyboard navigate rail tabs",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Action("focus_workspaces_sidebar"),
    },
    Command {
        label: "Toggle Files sidebar",
        icon: Some("icons/palette/folder-tree.svg"),
        keywords: "tree explorer",
        needs: Needs::Workspace,
        section: Section::Sidebar,
        kind: Kind::Action("toggle_files_sidebar"),
    },
    Command {
        label: "Maximize or restore the Changes dock",
        icon: Some("icons/palette/maximize-screen.svg"),
        keywords: "diff git review",
        needs: Needs::Workspace,
        section: Section::Workspaces,
        kind: Kind::Action("toggle_diff_dock_maximize"),
    },
    Command {
        label: "Changes dock: open a file tab",
        icon: Some("icons/palette/file-add.svg"),
        keywords: "diff git editor",
        needs: Needs::GitRepo,
        section: Section::Workspaces,
        kind: Kind::Action("diff_new_file_tab"),
    },
    Command {
        label: "Changes dock: open a terminal tab",
        icon: Some("icons/palette/computer-terminal-01.svg"),
        keywords: "diff git shell",
        needs: Needs::GitRepo,
        section: Section::Workspaces,
        kind: Kind::Action("diff_new_terminal_tab"),
    },
    Command {
        label: "Select all",
        icon: Some("icons/palette/text-selection.svg"),
        keywords: "clipboard terminal",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("terminal_select_all"),
    },
    Command {
        label: "Insert path",
        icon: Some("icons/palette/folder-search.svg"),
        keywords: "file folder directory browse fuzzy find cwd picker",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("insert_path"),
    },
    Command {
        label: "Fork conversation",
        icon: Some("icons/palette/git-fork.svg"),
        keywords: "agent branch claude codex split session both share files",
        needs: Needs::ForkableConversation,
        section: Section::Agents,
        kind: Kind::Action("fork_conversation"),
    },
    Command {
        label: "Resume conversation",
        icon: Some("icons/palette/play-circle.svg"),
        keywords: "agent restore reopen session claude codex",
        needs: Needs::ConversationResumeOffer,
        section: Section::Agents,
        kind: Kind::Action("accept_conversation_notice"),
    },
    Command {
        label: "New agent session",
        icon: Some("icons/palette/bubble-chat-add.svg"),
        keywords: "agent resume failed restart claude codex",
        needs: Needs::ConversationFailure,
        section: Section::Agents,
        kind: Kind::Action("accept_conversation_notice"),
    },
    Command {
        label: "Allow agent write for this session",
        icon: Some("icons/palette/checkmark-circle-02.svg"),
        keywords: "agent write request approve permit message orchestrate",
        needs: Needs::WriteRequest,
        section: Section::Agents,
        kind: Kind::Action("accept_conversation_notice"),
    },
    Command {
        label: "Deny agent write",
        icon: Some("icons/palette/blocked.svg"),
        keywords: "agent write request refuse reject block message",
        needs: Needs::WriteRequest,
        section: Section::Agents,
        kind: Kind::Action("dismiss_conversation_notice"),
    },
    Command {
        label: "Dismiss conversation notice",
        icon: Some("icons/palette/notification-off-01.svg"),
        keywords: "agent banner resume close hide",
        needs: Needs::ConversationNotice,
        section: Section::Agents,
        kind: Kind::Action("dismiss_conversation_notice"),
    },
    Command {
        label: "Find in this pane",
        icon: Some("icons/palette/search-01.svg"),
        keywords: "search terminal",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("toggle_search"),
    },
    Command {
        label: "Search next match",
        icon: Some("icons/palette/arrow-down-01.svg"),
        keywords: "find forward",
        needs: Needs::TerminalSearch,
        section: Section::Terminal,
        kind: Kind::Action("search_next"),
    },
    Command {
        label: "Search previous match",
        icon: Some("icons/palette/arrow-up-01.svg"),
        keywords: "find backward",
        needs: Needs::TerminalSearch,
        section: Section::Terminal,
        kind: Kind::Action("search_prev"),
    },
    Command {
        label: "Toggle search regex",
        icon: Some("icons/palette/regex.svg"),
        keywords: "find pattern",
        needs: Needs::TerminalSearch,
        section: Section::Terminal,
        kind: Kind::Action("toggle_search_regex"),
    },
    Command {
        label: "Dismiss search",
        icon: Some("icons/palette/search-remove.svg"),
        keywords: "find close",
        needs: Needs::TerminalSearch,
        section: Section::Terminal,
        kind: Kind::Action("dismiss_search"),
    },
    Command {
        label: "Increase pane font size",
        icon: Some("icons/palette/zoom-in.svg"),
        keywords: "terminal zoom bigger",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("font_size_increase"),
    },
    Command {
        label: "Decrease pane font size",
        icon: Some("icons/palette/zoom-out.svg"),
        keywords: "terminal zoom smaller",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("font_size_decrease"),
    },
    Command {
        label: "Reset pane font size",
        icon: Some("icons/palette/arrow-reload-horizontal.svg"),
        keywords: "terminal zoom default",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("font_size_reset"),
    },
    Command {
        label: "Reset terminal",
        icon: Some("icons/palette/refresh-ccw.svg"),
        keywords: "fix garbled escape",
        needs: Needs::Terminal,
        section: Section::Terminal,
        kind: Kind::Action("reset_terminal"),
    },
    Command {
        label: "Markdown: scroll up one page",
        icon: Some("icons/palette/arrow-up-double.svg"),
        keywords: "preview document",
        needs: Needs::Markdown,
        section: Section::Markdown,
        kind: Kind::Action("markdown_scroll_page_up"),
    },
    Command {
        label: "Markdown: scroll down one page",
        icon: Some("icons/palette/arrow-down-double.svg"),
        keywords: "preview document",
        needs: Needs::Markdown,
        section: Section::Markdown,
        kind: Kind::Action("markdown_scroll_page_down"),
    },
    Command {
        label: "Markdown: open the find bar",
        icon: Some("icons/palette/search-01.svg"),
        keywords: "preview search document",
        needs: Needs::Markdown,
        section: Section::Markdown,
        kind: Kind::Action("markdown_find_open"),
    },
    Command {
        label: "Markdown: copy the selection",
        icon: Some("icons/palette/copy-01.svg"),
        keywords: "preview clipboard",
        needs: Needs::Markdown,
        section: Section::Markdown,
        kind: Kind::Action("markdown_copy"),
    },
    Command {
        label: "Markdown: jump to the next match",
        icon: Some("icons/palette/arrow-down-01.svg"),
        keywords: "preview search forward",
        needs: Needs::MarkdownSearch,
        section: Section::Markdown,
        kind: Kind::Action("markdown_find_next"),
    },
    Command {
        label: "Markdown: jump to the previous match",
        icon: Some("icons/palette/arrow-up-01.svg"),
        keywords: "preview search backward",
        needs: Needs::MarkdownSearch,
        section: Section::Markdown,
        kind: Kind::Action("markdown_find_prev"),
    },
    Command {
        label: "Markdown: close the find bar",
        icon: Some("icons/palette/search-remove.svg"),
        keywords: "preview search dismiss",
        needs: Needs::MarkdownSearch,
        section: Section::Markdown,
        kind: Kind::Action("markdown_find_dismiss"),
    },
    Command {
        label: "Theme",
        icon: Some("icons/palette/paint-board.svg"),
        keywords: "color palette appearance dark light",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::Theme),
    },
    Command {
        label: "Theme mode",
        icon: Some("icons/palette/sun-moon.svg"),
        keywords: "appearance dark light system",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::ThemeMode),
    },
    Command {
        label: "Interface style",
        icon: Some("icons/palette/layers-01.svg"),
        keywords: "appearance themed blended transparency translucent blur material mica vibrancy",
        needs: Needs::NativeMaterial,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::InterfaceStyle),
    },
    Command {
        label: "Terminal font",
        icon: Some("icons/palette/text-square.svg"),
        keywords: "typeface family appearance",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::TerminalFont),
    },
    Command {
        label: "Terminal font size",
        icon: Some("icons/palette/text-font.svg"),
        keywords: "typeface appearance points",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::FontSize),
    },
    Command {
        label: "Terminal font weight",
        icon: Some("icons/palette/text-bold.svg"),
        keywords: "typeface appearance bold",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::FontWeight),
    },
    Command {
        label: "Terminal line height",
        icon: Some("icons/palette/paragraph-spacing.svg"),
        keywords: "typeface appearance spacing",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::LineHeight),
    },
    Command {
        label: "Terminal cell width",
        icon: Some("icons/palette/letter-spacing.svg"),
        keywords: "typeface appearance spacing",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::CellWidth),
    },
    Command {
        label: "Cursor shape",
        icon: Some("icons/palette/cursor-text.svg"),
        keywords: "appearance caret block beam",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::CursorShape),
    },
    Command {
        label: "Minimum contrast",
        icon: Some("icons/palette/contrast.svg"),
        keywords: "legibility readability apca light theme accessibility",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Scope(Scope::MinimumContrast),
    },
    Command {
        label: "Integrated glyphs",
        icon: Some("icons/palette/shapes.svg"),
        keywords: "appearance powerline box drawing",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Toggle {
            read: |app| terminal_bool(app, |terminal| terminal.resolved_integrated_glyphs()),
            write: |app, next, cx| app.persist_setting(true, "integrated_glyphs", json!(next), cx),
        },
    },
    Command {
        label: "Color emoji",
        icon: Some("icons/palette/smile.svg"),
        keywords: "appearance glyphs",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Toggle {
            read: |app| terminal_bool(app, |terminal| terminal.resolved_color_emoji()),
            write: |app, next, cx| app.persist_setting(true, "color_emoji", json!(next), cx),
        },
    },
    Command {
        label: "Terminal scrollbar",
        icon: Some("icons/palette/scroll-vertical.svg"),
        keywords: "appearance scroll",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Toggle {
            read: |app| terminal_bool(app, |terminal| terminal.resolved_scrollbar_visible()),
            write: |app, next, cx| app.persist_setting(true, "scrollbar", json!(next), cx),
        },
    },
    Command {
        label: "Reduce motion",
        icon: Some("icons/palette/pause-circle.svg"),
        keywords: "appearance animation accessibility",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Toggle {
            read: |app| app.cached_config.reduce_motion.unwrap_or(false),
            write: |app, next, cx| app.persist_setting(false, "reduce_motion", json!(next), cx),
        },
    },
    Command {
        label: "Comfortable density",
        icon: Some("icons/palette/layout-grid.svg"),
        keywords: "appearance compact panes gutter spacing cards layout",
        needs: Needs::Always,
        section: Section::Appearance,
        kind: Kind::Toggle {
            read: |app| app.cached_config.comfortable_density_enabled(),
            write: |app, next, cx| {
                app.persist_setting(false, "comfortable_density", json!(next), cx)
            },
        },
    },
    Command {
        label: "Show the branch in the sidebar",
        icon: Some("icons/palette/git-branch.svg"),
        keywords: "git appearance",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.branch.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "branch", next, cx),
        },
    },
    Command {
        label: "Show the diffstat in the sidebar",
        icon: Some("icons/palette/plus-minus-01.svg"),
        keywords: "git appearance changes",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.diffstat.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "diffstat", next, cx),
        },
    },
    Command {
        label: "Show pull requests in the sidebar",
        icon: Some("icons/palette/git-pull-request.svg"),
        keywords: "git appearance github",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.pr.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "pr", next, cx),
        },
    },
    Command {
        label: "Show the indent guide in the sidebar",
        icon: Some("icons/palette/text-indent.svg"),
        keywords: "appearance tree",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.indent_guide.unwrap_or(true),
            write: |app, next, cx| sidebar_show_toggle(app, "indent_guide", next, cx),
        },
    },
    Command {
        label: "Show the Agents section in the sidebar",
        icon: Some("icons/palette/sparkles.svg"),
        keywords: "agents attention appearance hide",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Toggle {
            read: |app| app.cached_config.sidebar_show.agents_enabled(),
            write: |app, next, cx| sidebar_show_toggle(app, "agents", next, cx),
        },
    },
    Command {
        label: "Expand the Agents section in the sidebar",
        icon: Some("icons/palette/arrow-expand-01.svg"),
        keywords: "agents collapse fold attention",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Toggle {
            read: |app| !app.agents_section.borrow().collapsed(),
            write: |app, next, cx| app.set_agents_section_collapsed(!next, cx),
        },
    },
    Command {
        label: "Default editor",
        icon: Some("icons/palette/source-code.svg"),
        keywords: "external open zed cursor vscode",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Scope(Scope::DefaultEditor),
    },
    Command {
        label: "Shell in the integrated terminal",
        icon: Some("icons/palette/command-line.svg"),
        keywords: "bash zsh fish powershell default",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Scope(Scope::DefaultShell),
    },
    Command {
        label: "When quitting with sessions running",
        icon: Some("icons/palette/logout-square-01.svg"),
        keywords: "exit ask keep stop",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Scope(Scope::OnQuit),
    },
    Command {
        label: "Ended sessions listed per workspace",
        icon: Some("icons/palette/history.svg"),
        keywords: "sidebar history limit",
        needs: Needs::Always,
        section: Section::Sidebar,
        kind: Kind::Scope(Scope::EndedSessions),
    },
    Command {
        label: "Native OS notifications",
        icon: Some("icons/palette/notification-01.svg"),
        keywords: "alert agent waiting",
        needs: Needs::Always,
        section: Section::Agents,
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
        icon: Some("icons/palette/clean.svg"),
        keywords: "git cleanup prune",
        needs: Needs::Always,
        section: Section::Paneflow,
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
        icon: Some("icons/palette/square-unlock-02.svg"),
        keywords: "agent permissions unrestricted",
        needs: Needs::Always,
        section: Section::Agents,
        kind: Kind::Toggle {
            read: |app| app.cached_config.ai_unrestricted.unwrap_or(false),
            write: |app, next, cx| app.persist_setting(false, "ai_unrestricted", json!(next), cx),
        },
    },
    Command {
        label: "Injection fence",
        icon: Some("icons/palette/security-check.svg"),
        keywords: "agent security prompt",
        needs: Needs::Always,
        section: Section::Agents,
        kind: Kind::Toggle {
            read: |app| app.cached_config.ai_injection_fence.unwrap_or(true),
            write: |app, next, cx| {
                app.persist_setting(false, "ai_injection_fence", json!(next), cx)
            },
        },
    },
    Command {
        label: "Full access for Claude Code",
        icon: Some("icons/palette/key-01.svg"),
        keywords: "agent permissions bypass",
        needs: Needs::Always,
        section: Section::Agents,
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
        icon: Some("icons/palette/sliders-horizontal.svg"),
        keywords: "preferences options configure",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Scope(Scope::SettingsTab),
    },
    Command {
        label: "Check for updates",
        icon: Some("icons/palette/download-04.svg"),
        keywords: "version upgrade release",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Action("check_for_updates"),
    },
    Command {
        label: "Paneflow documentation",
        icon: Some("icons/palette/book-open-01.svg"),
        keywords: "help docs manual",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::DOCUMENTATION_URL, cx)
        }),
    },
    Command {
        label: "What's new",
        icon: Some("icons/palette/new-releases.svg"),
        keywords: "help releases changelog",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::RELEASES_URL, cx)
        }),
    },
    Command {
        label: "Automations",
        icon: Some("icons/palette/flash.svg"),
        keywords: "help scripting cli mcp",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::AUTOMATIONS_URL, cx)
        }),
    },
    Command {
        label: "Troubleshooting",
        icon: Some("icons/palette/bug-01.svg"),
        keywords: "help support debug",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Run(|app, _window, cx| {
            app.open_help_url(crate::app::title_bar_menus::TROUBLESHOOTING_URL, cx)
        }),
    },
    Command {
        label: "System info…",
        icon: Some("icons/palette/computer.svg"),
        keywords: "help diagnostics version gpu",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Run(|app, window, cx| app.open_system_info_dialog(window, cx)),
    },
    Command {
        label: "About Paneflow",
        icon: Some("icons/palette/information-circle.svg"),
        keywords: "help version license",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Run(|app, window, cx| app.open_about_dialog(window, cx)),
    },
    Command {
        label: "Quit Paneflow",
        icon: Some("icons/palette/logout-01.svg"),
        keywords: "exit close application",
        needs: Needs::Always,
        section: Section::Paneflow,
        kind: Kind::Action("quit"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_in(id: u64, space: Option<SpaceId>) -> crate::workspace::Workspace {
        let mut workspace = crate::workspace::Workspace::empty_with_cwd_and_id(
            id,
            format!("ws{id}"),
            std::path::PathBuf::new(),
        );
        workspace.space = space;
        workspace
    }

    #[test]
    fn the_workspace_picker_lists_the_active_space_first() {
        let work = SpaceId::parse("550e8400-e29b-41d4-a716-446655440001").unwrap();
        let workspaces = vec![
            workspace_in(1, None),
            workspace_in(2, Some(work.clone())),
            workspace_in(3, None),
        ];
        let mut config = paneflow_config::schema::PaneFlowConfig::default();
        assert_eq!(
            palette_workspace_order(&workspaces, &config, None),
            [0, 1, 2]
        );
        config.spaces.push(paneflow_config::schema::SpaceConfig {
            id: work.clone(),
            name: "Work".to_string(),
            color: None,
            avatar: None,
            env: Default::default(),
        });
        assert_eq!(
            palette_workspace_order(&workspaces, &config, Some(&work)),
            [1, 0, 2]
        );
        assert_eq!(
            palette_workspace_order(&workspaces, &config, None),
            [0, 2, 1]
        );
    }

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
            Scope::InterfaceStyle,
            Scope::Workspace,
            Scope::Space,
            Scope::MoveToSpace,
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
    fn every_section_holds_a_command_and_an_embedded_icon() {
        for section in Section::ALL {
            assert!(section.commands().next().is_some(), "{section:?} is empty");
            assert!(
                crate::assets::Assets::get(section.icon()).is_some(),
                "{section:?} points at a missing icon"
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
    fn every_command_has_an_embedded_icon() {
        for command in COMMANDS {
            let Some(icon) = command.icon else {
                panic!("{} has no icon", command.label);
            };
            assert!(
                crate::assets::Assets::get(icon).is_some(),
                "{} points at the missing icon {icon}",
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
        assert!(Apply::InterfaceStyle(InterfaceStyle::Blended).applies_in_place());
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
