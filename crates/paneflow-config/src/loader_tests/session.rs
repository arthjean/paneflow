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
    }
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
            // An empty folder: one tab, no pane (v2 needs no `empty` marker).
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
    // Verify structure: root is split with 2 children, second child is also a split
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
    // Truncated JSON - simulates crash during write
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

// ─── Telemetry config (US-009) ────────────────────────────────────────
//
// Tri-state encoding:
//   - outer None          → user never prompted (block missing from JSON)
//   - Some { enabled: None } → block present, question unanswered
//   - Some { enabled: Some(true|false) } → explicit consent answer
//
// No event is ever emitted unless `enabled == Some(true)` at the
// capture layer (US-012).

#[test]
fn test_session_with_removed_agents_view_restores_in_cli_mode() {
    // A session.json written by a build that still had the Agents view: an
    // unknown `"mode"` string plus keys this schema no longer declares. It
    // must restore silently in CLI mode - a rejected parse would quarantine
    // the file and cost the user every workspace in it.
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
    // A literal pre-US-007 session.json: no `mode` key. Must deserialise
    // to the default `AppMode::Cli`.
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
    // US-001 (prd-git-diff-mode-2026-Q3.md): the Review mode.
    assert_eq!(serde_json::to_string(&AppMode::Diff).unwrap(), "\"diff\"");
}

#[test]
fn test_app_mode_diff_round_trips() {
    // US-001 (prd-git-diff-mode-2026-Q3.md): `Diff` survives a
    // serialize -> deserialize cycle and a session.json carrying it
    // restores into `AppMode::Diff` (not the `Cli` default).
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
    // US-015 (prd-git-diff-mode-2026-Q3.md): diff_scope persists, and a
    // session.json written before this field restores it as `None`.
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
    // US-018: `layout` and `empty` are v1-only. A v2 workspace must not grow
    // either key back, or an older Paneflow would read the file as v1 data
    // under a v2 version number.
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

// ─── v1 -> v2 migration (US-018, prd-cli-tab-hierarchy-2026-Q3) ────────

/// Frozen v1 `session.json`: one workspace, a horizontal split, and a pane
/// stacking three surfaces (the second one focused) next to a single-surface
/// pane. Deliberately a literal - it is the shape shipped Paneflow versions
/// actually wrote, and must keep migrating even as the v2 structs move on.
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
    // AC7: the migration re-homes surfaces, it never drops them.
    let mut state: SessionState = serde_json::from_str(V1_FIXTURE).unwrap();
    assert_eq!(state.version, SESSION_SCHEMA_VERSION_V1);
    let before = count_surfaces(state.workspaces[0].legacy_layout.as_ref().unwrap());
    assert_eq!(before, 4, "fixture holds 4 surfaces across 2 panes");

    migrate_session_v1(&mut state);

    assert_eq!(state.version, SESSION_SCHEMA_VERSION);
    let ws = &state.workspaces[0];
    assert_eq!(count_workspace_surfaces(ws), before, "no surface is lost");
    // The tree becomes the first tab, each pane reduced to its focused surface.
    assert_eq!(ws.tabs.len(), 3, "1 tree tab + 2 promoted surfaces");
    let first = ws.tabs[0].layout.as_ref().unwrap();
    assert_eq!(count_surfaces(first), 2, "one surface per pane");
    assert_eq!(first.leaf_count(), 2, "the split survives intact");
    // Traversal order: the non-focused surfaces of the first pane, in order.
    assert_eq!(ws.tabs[1].title, "zsh");
    assert_eq!(ws.tabs[2].title, "claude");
    // The focused surface is the one that stayed in the tree.
    let kept = match first {
        LayoutNode::Split { children, .. } => match &children[0] {
            LayoutNode::Pane { surfaces } => surfaces[0].name.clone(),
            _ => panic!("expected a pane"),
        },
        _ => panic!("expected a split"),
    };
    assert_eq!(kept.as_deref(), Some("cargo-run"), "the focused one stays");
    // Untouched v1 fields survive, and the legacy keys are drained.
    assert_eq!(ws.expanded_paths, vec!["src".to_string()]);
    assert!(ws.legacy_layout.is_none());
    assert!(!ws.legacy_empty);
    assert_eq!(ws.active_tab, 0);
}

#[test]
fn test_migrate_v1_caps_promoted_tabs() {
    // AC4: a v1 pane carrying the 64 allowed surfaces migrates under
    // MAX_SESSION_TABS - surplus dropped loudly, never silently.
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
    // Order is preserved: the first surface stays in the tree tab (no `focus`
    // key means index 0), the next ones are promoted in order.
    assert_eq!(ws.tabs[1].title, "s1");
    assert_eq!(
        ws.tabs[MAX_SESSION_TABS - 1].title,
        format!("s{}", MAX_SESSION_TABS - 1)
    );
}

#[test]
fn test_migrate_v1_null_layout_becomes_default_pane() {
    // AC6: v1 `layout: null` with no tabs key meant "one default pane".
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
    // The EP-003 `empty` marker is the one v1 case that must NOT gain a pane:
    // an empty folder restores empty.
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
    // Re-running the migration over an already-migrated state must not
    // re-promote anything or duplicate a tab.
    let mut state: SessionState = serde_json::from_str(V1_FIXTURE).unwrap();
    migrate_session_v1(&mut state);
    let once = state.clone();
    migrate_session_v1(&mut state);
    assert_eq!(once, state);
}

/// A `session.json` written before auto-naming existed carries no provenance,
/// and every title in it is either a name someone typed or a preset label.
/// Reading that silence as `User` is the half that cannot destroy anything;
/// the app's restore path hands the preset labels back to `Auto`.
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

/// The field briefly had a two-value ladder whose non-user half was "auto".
/// A session file from that build must not have every tab read as user-named,
/// which would freeze them all out of auto-naming with no visible cause.
#[test]
fn test_the_retired_auto_value_reads_as_preset() {
    let tab: TabSession =
        serde_json::from_str(r#"{"title": "Claude Code", "title_source": "auto"}"#).unwrap();
    assert_eq!(tab.title_source, Some(TabTitleSource::Preset));
}

/// Same tolerance as `AppMode`: a value this build does not know must not cost
/// the user every workspace in the file. It fails safe, onto the variant that
/// protects a typed name.
#[test]
fn test_unknown_tab_title_source_falls_back_to_user() {
    let tab: TabSession =
        serde_json::from_str(r#"{"title": "sprint 3", "title_source": "summarizer"}"#).unwrap();
    assert_eq!(tab.title_source, Some(TabTitleSource::User));
}

/// A tab Paneflow builds around a layout states its provenance outright. The
/// paneless `empty()` is `Default`, and its blank title makes the distinction
/// moot - a blank legacy title restores as `Preset` too.
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

/// A promoted tab's title is the surface's `custom_name`, and a v1 file cannot
/// tell one the user typed from one Paneflow wrote. The migration says so
/// rather than guessing, and the restore path judges it by content like every
/// other legacy title.
#[test]
fn test_migrate_v1_states_no_provenance_for_promoted_titles() {
    let mut state: SessionState = serde_json::from_str(V1_FIXTURE).unwrap();
    migrate_session_v1(&mut state);

    let ws = &state.workspaces[0];
    assert_eq!(ws.tabs[0].title, "", "the inherited tree is an unnamed tab");
    assert_eq!(ws.tabs[1].title_source, None);
}

/// The precedence ladder, at the schema boundary that persists it. The app's
/// `Tab::set_title` enforces it; this pins the ordering the file format
/// promises, so a variant reordered here fails loudly rather than silently
/// letting an automatic title overwrite a typed one.
#[test]
fn test_title_source_precedence_is_a_strict_ladder() {
    use TabTitleSource::*;

    // Each rank yields to every rank above it.
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

    // The one-shot ranks refuse their own kind; the live ones accept it.
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
