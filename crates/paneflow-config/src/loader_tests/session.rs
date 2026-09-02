use crate::schema::*;

fn make_workspace(title: &str, cwd: &str, tabs: Vec<TabSession>) -> WorkspaceSession {
    WorkspaceSession {
        title: title.to_string(),
        cwd: cwd.to_string(),
        tabs,
        active_tab: 0,
        legacy_layout: None,
        legacy_empty: false,
        custom_buttons: vec![],
        expanded_paths: vec![],
        managed_worktrees: vec![],
        sidebar_collapsed: false,
    }
}

#[test]
fn a_folded_rail_row_survives_a_restart_and_an_older_file_stays_unfolded() {
    let mut ws = make_workspace("main", "/home/user/project", vec![]);
    ws.sidebar_collapsed = true;
    let json = serde_json::to_string(&ws).unwrap();
    assert!(json.contains("sidebar_collapsed"));
    let back: WorkspaceSession = serde_json::from_str(&json).unwrap();
    assert!(back.sidebar_collapsed);

    let unfolded = serde_json::to_string(&make_workspace("main", "/p", vec![])).unwrap();
    assert!(!unfolded.contains("sidebar_collapsed"));
    let legacy: WorkspaceSession = serde_json::from_str(r#"{"title":"main","cwd":"/p"}"#).unwrap();
    assert!(!legacy.sidebar_collapsed);
}

fn make_surface(cwd: &str) -> SurfaceDefinition {
    SurfaceDefinition {
        surface_type: Some("terminal".to_string()),
        cwd: Some(cwd.to_string()),
        ..Default::default()
    }
}

#[test]
fn test_session_roundtrip_single_workspace() {
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![make_workspace(
            "main",
            "/home/user/project",
            vec![TabSession::with_layout(LayoutNode::Pane {
                surfaces: vec![make_surface("/home/user/project")],
            })],
        )],
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
}

#[test]
fn test_session_roundtrip_multiple_workspaces() {
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 1,
        workspaces: vec![
            make_workspace(
                "frontend",
                "/home/user/web",
                vec![TabSession::with_layout(LayoutNode::Pane {
                    surfaces: vec![make_surface("/home/user/web")],
                })],
            ),
            make_workspace(
                "backend",
                "/home/user/api",
                vec![TabSession::with_layout(LayoutNode::Pane {
                    surfaces: vec![make_surface("/home/user/api")],
                })],
            ),
            make_workspace("devops", "/home/user/infra", vec![TabSession::empty()]),
        ],
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    assert_eq!(restored.active_workspace, 1);
    assert_eq!(restored.workspaces.len(), 3);
}

#[test]
fn test_session_roundtrip_nested_splits() {
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![make_workspace(
            "dev",
            "/home/user",
            vec![TabSession::with_layout(LayoutNode::Split {
                direction: "horizontal".to_string(),
                ratio: None,
                ratios: Some(vec![0.6, 0.4]),
                children: vec![
                    LayoutNode::Pane {
                        surfaces: vec![make_surface("/home/user/code")],
                    },
                    LayoutNode::Split {
                        direction: "vertical".to_string(),
                        ratio: None,
                        ratios: Some(vec![0.5, 0.5]),
                        children: vec![
                            LayoutNode::Pane {
                                surfaces: vec![make_surface("/home/user/tests")],
                            },
                            LayoutNode::Pane {
                                surfaces: vec![make_surface("/home/user/logs")],
                            },
                        ],
                    },
                ],
            })],
        )],
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    let layout = restored.workspaces[0].tabs[0].layout.as_ref().unwrap();
    assert_eq!(layout.leaf_count(), 3);
}

#[test]
fn test_session_roundtrip_with_scrollback() {
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![make_workspace(
            "main",
            "/tmp",
            vec![TabSession::with_layout(LayoutNode::Pane {
                surfaces: vec![SurfaceDefinition {
                    surface_type: Some("terminal".to_string()),
                    cwd: Some("/tmp".to_string()),
                    scrollback: Some("$ ls\nfile1.txt\nfile2.txt\n$ echo hello\nhello".to_string()),
                    ..Default::default()
                }],
            })],
        )],
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    let surface = match &restored.workspaces[0].tabs[0].layout {
        Some(LayoutNode::Pane { surfaces }) => &surfaces[0],
        _ => panic!("expected pane"),
    };
    assert!(surface.scrollback.as_ref().unwrap().contains("hello"));
}

#[test]
fn test_session_corrupted_json_returns_none() {
    let corrupted = r#"{"version":1,"active_workspace":0,"workspaces":[{"title":"ma"#;
    let result: Result<SessionState, _> = serde_json::from_str(corrupted);
    assert!(result.is_err(), "Corrupted JSON should fail to parse");
}

#[test]
fn test_session_scrollback_none_omitted_from_json() {
    let surface = SurfaceDefinition {
        scrollback: None,
        ..Default::default()
    };
    let json = serde_json::to_string(&surface).unwrap();
    assert!(
        !json.contains("scrollback"),
        "None scrollback should be omitted from JSON"
    );
}

#[test]
fn test_session_with_removed_agents_view_restores_in_cli_mode() {
    let legacy = r#"{
        "version": 1,
        "active_workspace": 0,
        "workspaces": [
            { "title": "main", "cwd": "/tmp", "layout": null }
        ],
        "projects": [
            { "id": 1, "title": "Paneflow", "cwd": "/tmp", "threads": [] }
        ],
        "active_project": 0,
        "chats": [],
        "agents_target": { "type": "chat", "thread_id": 3 },
        "mode": "agents"
    }"#;
    let restored: SessionState = serde_json::from_str(legacy).unwrap();
    assert_eq!(restored.workspaces.len(), 1, "the workspaces survive");
    assert_eq!(
        restored.mode,
        AppMode::Cli,
        "an unknown mode falls back to CLI"
    );
}

#[test]
fn test_session_backward_compat_pre_us007() {
    let legacy = r#"{
        "version": 1,
        "active_workspace": 0,
        "workspaces": [
            { "title": "main", "cwd": "/tmp", "layout": null }
        ]
    }"#;
    let restored: SessionState = serde_json::from_str(legacy).unwrap();
    assert_eq!(restored.workspaces.len(), 1);
    assert_eq!(
        restored.mode,
        AppMode::Cli,
        "legacy session.json must restore in CLI mode"
    );
}

#[test]
fn test_app_mode_serializes_snake_case() {
    assert_eq!(serde_json::to_string(&AppMode::Cli).unwrap(), "\"cli\"");
    assert_eq!(serde_json::to_string(&AppMode::Diff).unwrap(), "\"diff\"");
}

#[test]
fn test_app_mode_diff_round_trips() {
    let json = serde_json::to_string(&AppMode::Diff).unwrap();
    let back: AppMode = serde_json::from_str(&json).unwrap();
    assert_eq!(back, AppMode::Diff);

    let session = r#"{
        "version": 1,
        "active_workspace": 0,
        "workspaces": [],
        "mode": "diff"
    }"#;
    let restored: SessionState = serde_json::from_str(session).unwrap();
    assert_eq!(restored.mode, AppMode::Diff);
}

#[test]
fn test_session_diff_scope_round_trips_and_defaults() {
    let legacy = r#"{ "version": 1, "active_workspace": 0, "workspaces": [] }"#;
    let restored: SessionState = serde_json::from_str(legacy).unwrap();
    assert_eq!(restored.diff_scope, None);

    let with_scope = r#"{
        "version": 1,
        "active_workspace": 0,
        "workspaces": [],
        "diff_scope": "worktree"
    }"#;
    let restored2: SessionState = serde_json::from_str(with_scope).unwrap();
    assert_eq!(restored2.diff_scope.as_deref(), Some("worktree"));
}

#[test]
fn test_v2_workspace_writes_no_legacy_keys() {
    let ws = make_workspace("main", "/tmp", vec![TabSession::empty()]);
    let value = serde_json::to_value(&ws).unwrap();
    let keys = value.as_object().expect("a JSON object");
    assert!(
        !keys.contains_key("layout"),
        "no workspace-level layout in v2"
    );
    assert!(!keys.contains_key("empty"), "no empty marker in v2");
    assert!(!keys.contains_key("active_tab"), "index 0 stays implicit");
    assert!(keys.contains_key("tabs"), "the tab list is always written");
}

const V1_FIXTURE: &str = r#"{
    "version": 1,
    "active_workspace": 0,
    "workspaces": [
        {
            "title": "paneflow",
            "cwd": "/home/user/dev/paneflow",
            "layout": {
                "type": "split",
                "direction": "horizontal",
                "ratios": [0.6, 0.4],
                "children": [
                    {
                        "type": "pane",
                        "surfaces": [
                            { "surface_type": "terminal", "name": "zsh", "cwd": "/home/user/dev/paneflow" },
                            { "surface_type": "terminal", "name": "cargo-run", "cwd": "/home/user/dev/paneflow", "focus": true },
                            { "surface_type": "terminal", "name": "claude", "cwd": "/home/user/dev/paneflow" }
                        ]
                    },
                    {
                        "type": "pane",
                        "surfaces": [
                            { "surface_type": "terminal", "name": "vite", "cwd": "/home/user/dev/paneflow/web" }
                        ]
                    }
                ]
            },
            "custom_buttons": [],
            "expanded_paths": ["src"]
        }
    ],
    "active_project": 0
}"#;

fn count_surfaces(node: &LayoutNode) -> usize {
    match node {
        LayoutNode::Pane { surfaces } => surfaces.len(),
        LayoutNode::Split { children, .. } => children.iter().map(count_surfaces).sum(),
    }
}

fn count_workspace_surfaces(ws: &WorkspaceSession) -> usize {
    ws.tabs
        .iter()
        .filter_map(|tab| tab.layout.as_ref())
        .map(count_surfaces)
        .sum()
}

#[test]
fn test_migrate_v1_preserves_surface_count() {
    let mut state: SessionState = serde_json::from_str(V1_FIXTURE).unwrap();
    assert_eq!(state.version, SESSION_SCHEMA_VERSION_V1);
    let before = count_surfaces(state.workspaces[0].legacy_layout.as_ref().unwrap());
    assert_eq!(before, 4, "fixture holds 4 surfaces across 2 panes");

    migrate_session_v1(&mut state);

    assert_eq!(state.version, SESSION_SCHEMA_VERSION);
    let ws = &state.workspaces[0];
    assert_eq!(count_workspace_surfaces(ws), before, "no surface is lost");
    assert_eq!(ws.tabs.len(), 3, "1 tree tab + 2 promoted surfaces");
    let first = ws.tabs[0].layout.as_ref().unwrap();
    assert_eq!(count_surfaces(first), 2, "one surface per pane");
    assert_eq!(first.leaf_count(), 2, "the split survives intact");
    assert_eq!(ws.tabs[1].title, "zsh");
    assert_eq!(ws.tabs[2].title, "claude");
    let kept = match first {
        LayoutNode::Split { children, .. } => match &children[0] {
            LayoutNode::Pane { surfaces } => surfaces[0].name.clone(),
            _ => panic!("expected a pane"),
        },
        _ => panic!("expected a split"),
    };
    assert_eq!(kept.as_deref(), Some("cargo-run"), "the focused one stays");
    assert_eq!(ws.expanded_paths, vec!["src".to_string()]);
    assert!(ws.legacy_layout.is_none());
    assert!(!ws.legacy_empty);
    assert_eq!(ws.active_tab, 0);
}

#[test]
fn test_migrate_v1_caps_promoted_tabs() {
    let surfaces: Vec<String> = (0..64)
        .map(|i| format!(r#"{{ "surface_type": "terminal", "name": "s{i}" }}"#))
        .collect();
    let json = format!(
        r#"{{ "version": 1, "active_workspace": 0, "workspaces": [
            {{ "title": "big", "cwd": "/tmp", "layout": {{ "type": "pane", "surfaces": [{}] }} }}
        ] }}"#,
        surfaces.join(",")
    );
    let mut state: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(
        count_surfaces(state.workspaces[0].legacy_layout.as_ref().unwrap()),
        64
    );

    migrate_session_v1(&mut state);

    let ws = &state.workspaces[0];
    assert_eq!(ws.tabs.len(), MAX_SESSION_TABS, "capped, not unbounded");
    assert_eq!(
        count_workspace_surfaces(ws),
        MAX_SESSION_TABS,
        "one surface per surviving tab"
    );
    assert_eq!(ws.tabs[1].title, "s1");
    assert_eq!(
        ws.tabs[MAX_SESSION_TABS - 1].title,
        format!("s{}", MAX_SESSION_TABS - 1)
    );
}

#[test]
fn test_migrate_v1_null_layout_becomes_default_pane() {
    let json = r#"{ "version": 1, "active_workspace": 0, "workspaces": [
        { "title": "main", "cwd": "/tmp", "layout": null }
    ] }"#;
    let mut state: SessionState = serde_json::from_str(json).unwrap();
    migrate_session_v1(&mut state);

    let ws = &state.workspaces[0];
    assert_eq!(ws.tabs.len(), 1);
    let layout = ws.tabs[0].layout.as_ref().expect("a default pane");
    assert_eq!(count_surfaces(layout), 1);
}

#[test]
fn test_migrate_v1_empty_marker_becomes_paneless_tab() {
    let json = r#"{ "version": 1, "active_workspace": 0, "workspaces": [
        { "title": "main", "cwd": "/tmp", "layout": null, "empty": true }
    ] }"#;
    let mut state: SessionState = serde_json::from_str(json).unwrap();
    migrate_session_v1(&mut state);

    let ws = &state.workspaces[0];
    assert_eq!(ws.tabs.len(), 1, "FR-01: a workspace always keeps one tab");
    assert!(ws.tabs[0].layout.is_none(), "and it holds no pane");
    assert!(!ws.legacy_empty, "the marker is drained");
}

#[test]
fn test_migrate_v1_is_idempotent_on_v2_shape() {
    let mut state: SessionState = serde_json::from_str(V1_FIXTURE).unwrap();
    migrate_session_v1(&mut state);
    let once = state.clone();
    migrate_session_v1(&mut state);
    assert_eq!(once, state);
}

#[test]
fn test_tab_title_source_is_absent_when_the_file_predates_it() {
    let tab: TabSession = serde_json::from_str(r#"{"title": "sprint 3"}"#).unwrap();
    assert_eq!(
        tab.title_source, None,
        "the app's restore path, not this schema, decides what silence means"
    );
}

#[test]
fn test_tab_title_source_reads_an_explicit_value() {
    let prompt: TabSession =
        serde_json::from_str(r#"{"title": "wire up the parser", "title_source": "prompt"}"#)
            .unwrap();
    assert_eq!(prompt.title_source, Some(TabTitleSource::Prompt));

    let generated: TabSession =
        serde_json::from_str(r#"{"title": "Parser wiring", "title_source": "generated"}"#).unwrap();
    assert_eq!(generated.title_source, Some(TabTitleSource::Generated));

    let preset: TabSession =
        serde_json::from_str(r#"{"title": "Claude Code", "title_source": "preset"}"#).unwrap();
    assert_eq!(preset.title_source, Some(TabTitleSource::Preset));

    let user: TabSession =
        serde_json::from_str(r#"{"title": "sprint 3", "title_source": "user"}"#).unwrap();
    assert_eq!(user.title_source, Some(TabTitleSource::User));
}

#[test]
fn test_the_retired_auto_value_reads_as_preset() {
    let tab: TabSession =
        serde_json::from_str(r#"{"title": "Claude Code", "title_source": "auto"}"#).unwrap();
    assert_eq!(tab.title_source, Some(TabTitleSource::Preset));
}

#[test]
fn test_unknown_tab_title_source_falls_back_to_user() {
    let tab: TabSession =
        serde_json::from_str(r#"{"title": "sprint 3", "title_source": "summarizer"}"#).unwrap();
    assert_eq!(tab.title_source, Some(TabTitleSource::User));
}

#[test]
fn test_a_tab_built_in_process_is_preset() {
    assert_eq!(
        TabSession::with_layout(LayoutNode::Pane {
            surfaces: vec![make_surface("/home/user/project")],
        })
        .title_source,
        Some(TabTitleSource::Preset)
    );
    assert_eq!(TabSession::empty().title, "");
}

#[test]
fn test_tab_title_source_survives_a_roundtrip() {
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![make_workspace(
            "main",
            "/home/user/project",
            vec![
                TabSession {
                    title: "sprint 3".to_string(),
                    title_source: Some(TabTitleSource::User),
                    layout: None,
                    worktree: None,
                },
                TabSession {
                    title: "wire up the parser".to_string(),
                    title_source: Some(TabTitleSource::Prompt),
                    layout: None,
                    worktree: Some("/home/user/project.worktrees/parser".to_string()),
                },
            ],
        )],
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string(&state).unwrap();
    let back: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(back, state);
}

#[test]
fn test_migrate_v1_states_no_provenance_for_promoted_titles() {
    let mut state: SessionState = serde_json::from_str(V1_FIXTURE).unwrap();
    migrate_session_v1(&mut state);

    let ws = &state.workspaces[0];
    assert_eq!(ws.tabs[0].title, "", "the inherited tree is an unnamed tab");
    assert_eq!(ws.tabs[1].title_source, None);
}

#[test]
fn test_title_source_precedence_is_a_strict_ladder() {
    use TabTitleSource::*;

    for (weaker, stronger) in [
        (Preset, Prompt),
        (Preset, Generated),
        (Preset, User),
        (Prompt, Generated),
        (Prompt, User),
        (Generated, User),
    ] {
        assert!(weaker.yields_to(stronger), "{weaker:?} -> {stronger:?}");
        assert!(
            !stronger.yields_to(weaker),
            "{stronger:?} must not yield to {weaker:?}"
        );
    }

    assert!(!Preset.yields_to(Preset), "a preset label is written once");
    assert!(
        !Prompt.yields_to(Prompt),
        "a later prompt must not rename the tab away from the first"
    );
    assert!(
        Generated.yields_to(Generated),
        "a regenerated session title is a better one"
    );
    assert!(User.yields_to(User), "renaming twice means the second name");
}

#[test]
fn test_only_the_top_two_ranks_settle_a_title() {
    assert!(!TabTitleSource::Preset.is_settled());
    assert!(!TabTitleSource::Prompt.is_settled());
    assert!(TabTitleSource::Generated.is_settled());
    assert!(TabTitleSource::User.is_settled());
}
