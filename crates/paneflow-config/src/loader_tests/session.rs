use crate::schema::*;

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
        version: 1,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            title: "main".to_string(),
            cwd: "/home/user/project".to_string(),
            layout: Some(LayoutNode::Pane {
                surfaces: vec![make_surface("/home/user/project")],
            }),
            custom_buttons: vec![],
            expanded_paths: vec![],
            managed_worktrees: vec![],
        }],
        projects: Vec::new(),
        active_project: 0,
        chats: Vec::new(),
        agents_target: None,
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
        version: 1,
        active_workspace: 1,
        workspaces: vec![
            WorkspaceSession {
                title: "frontend".to_string(),
                cwd: "/home/user/web".to_string(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![make_surface("/home/user/web")],
                }),
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
            },
            WorkspaceSession {
                title: "backend".to_string(),
                cwd: "/home/user/api".to_string(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![make_surface("/home/user/api")],
                }),
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
            },
            WorkspaceSession {
                title: "devops".to_string(),
                cwd: "/home/user/infra".to_string(),
                layout: None,
                custom_buttons: vec![],
                expanded_paths: vec![],
                managed_worktrees: vec![],
            },
        ],
        projects: Vec::new(),
        active_project: 0,
        chats: Vec::new(),
        agents_target: None,
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
        version: 1,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            title: "dev".to_string(),
            cwd: "/home/user".to_string(),
            custom_buttons: vec![],
            expanded_paths: vec![],
            managed_worktrees: vec![],
            layout: Some(LayoutNode::Split {
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
            }),
        }],
        projects: Vec::new(),
        active_project: 0,
        chats: Vec::new(),
        agents_target: None,
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    // Verify structure: root is split with 2 children, second child is also a split
    let layout = restored.workspaces[0].layout.as_ref().unwrap();
    assert_eq!(layout.leaf_count(), 3);
}

#[test]
fn test_session_roundtrip_with_scrollback() {
    let state = SessionState {
        version: 1,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            title: "main".to_string(),
            cwd: "/tmp".to_string(),
            custom_buttons: vec![],
            expanded_paths: vec![],
            managed_worktrees: vec![],
            layout: Some(LayoutNode::Pane {
                surfaces: vec![SurfaceDefinition {
                    surface_type: Some("terminal".to_string()),
                    cwd: Some("/tmp".to_string()),
                    scrollback: Some("$ ls\nfile1.txt\nfile2.txt\n$ echo hello\nhello".to_string()),
                    ..Default::default()
                }],
            }),
        }],
        projects: Vec::new(),
        active_project: 0,
        chats: Vec::new(),
        agents_target: None,
        mode: AppMode::default(),
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    let surface = match &restored.workspaces[0].layout {
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
fn test_session_roundtrip_mixed_workspaces_and_projects() {
    let state = SessionState {
        version: 1,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            title: "main".to_string(),
            cwd: "/home/user".to_string(),
            layout: Some(LayoutNode::Pane {
                surfaces: vec![make_surface("/home/user")],
            }),
            custom_buttons: vec![],
            expanded_paths: vec![],
            managed_worktrees: vec![],
        }],
        projects: vec![ProjectSession {
            id: 42,
            title: "Paneflow".to_string(),
            cwd: "/home/user/dev/paneflow".to_string(),
            is_expanded: true,
            threads: vec![ThreadSession {
                id: 100,
                title: "Wire up the agents view".to_string(),
                agent: "claude_code".to_string(),
                cwd: "/home/user/dev/paneflow".to_string(),
                created_at: 1_716_336_000_000,
                model: Some("sonnet".to_string()),
                mode: Some("default".to_string()),
                store_id: Some("uuid-abc-123".to_string()),
                kind: None,
                terminal_agent: None,
                pinned: false,
                session_id: None,
                title_user_set: false,
            }],
        }],
        active_project: 0,
        chats: Vec::new(),
        agents_target: None,
        mode: AppMode::Agents,
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    assert_eq!(restored.projects[0].threads[0].agent, "claude_code");
    assert_eq!(restored.mode, AppMode::Agents);
}

// US-001/US-002 (Agents UI redesign): the
// SessionState gained `chats` and ThreadSession gained `pinned`. These
// cover the round-trip with both fields populated and the backward-compat
// default when a pre-refonte session.json lacks the keys.
#[test]
fn test_session_roundtrip_with_chats_and_pinned() {
    let state = SessionState {
        version: 1,
        active_workspace: 0,
        workspaces: vec![],
        projects: vec![ProjectSession {
            id: 1,
            title: "Paneflow".to_string(),
            cwd: "/home/user/dev/paneflow".to_string(),
            is_expanded: true,
            threads: vec![ThreadSession {
                id: 10,
                title: "Pinned project thread".to_string(),
                agent: "claude_code".to_string(),
                cwd: "/home/user/dev/paneflow".to_string(),
                created_at: 1_716_336_000_000,
                model: None,
                mode: None,
                store_id: None,
                kind: Some("terminal".to_string()),
                terminal_agent: Some("claude_code".to_string()),
                pinned: true,
                session_id: None,
                title_user_set: false,
            }],
        }],
        active_project: 0,
        chats: vec![ThreadSession {
            id: 20,
            title: "Quick scratch chat".to_string(),
            agent: "codex".to_string(),
            cwd: "/home/user".to_string(),
            created_at: 1_716_337_000_000,
            model: None,
            mode: None,
            store_id: None,
            kind: Some("terminal".to_string()),
            terminal_agent: Some("codex".to_string()),
            pinned: false,
            session_id: None,
            title_user_set: false,
        }],
        agents_target: None,
        mode: AppMode::Agents,
        diff_scope: None,
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    let restored: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, restored);
    assert_eq!(restored.chats.len(), 1, "the free chat round-trips");
    assert_eq!(restored.chats[0].cwd, "/home/user", "chat anchored on home");
    assert!(
        restored.projects[0].threads[0].pinned,
        "the pinned flag round-trips on a project thread"
    );
    assert!(!restored.chats[0].pinned, "an unpinned chat stays unpinned");
}

#[test]
fn test_session_pre_refonte_defaults_chats_empty_and_unpinned() {
    // A pre-refonte session.json: a project thread with no `pinned`
    // key, and no top-level `chats` key. Must restore as `chats = []`
    // and `pinned = false` everywhere - no migration, no error.
    let legacy = r#"{
        "version": 1,
        "active_workspace": 0,
        "workspaces": [],
        "projects": [
            {
                "id": 1,
                "title": "Paneflow",
                "cwd": "/home/user/dev/paneflow",
                "is_expanded": true,
                "threads": [
                    {
                        "id": 10,
                        "title": "Old thread",
                        "agent": "claude_code",
                        "cwd": "/home/user/dev/paneflow",
                        "created_at": 0
                    }
                ]
            }
        ],
        "active_project": 0,
        "mode": "agents"
    }"#;
    let restored: SessionState = serde_json::from_str(legacy).unwrap();
    assert!(restored.chats.is_empty(), "chats must default to []");
    assert!(
        !restored.projects[0].threads[0].pinned,
        "a thread with no `pinned` key restores as unpinned"
    );
}

#[test]
fn test_session_backward_compat_pre_us007() {
    // A literal pre-US-007 session.json: no `projects`, no
    // `active_project`, no `mode` keys. Must deserialise to an
    // empty project list and the default `AppMode::Cli`.
    let legacy = r#"{
        "version": 1,
        "active_workspace": 0,
        "workspaces": [
            { "title": "main", "cwd": "/tmp", "layout": null }
        ]
    }"#;
    let restored: SessionState = serde_json::from_str(legacy).unwrap();
    assert_eq!(restored.workspaces.len(), 1);
    assert!(restored.projects.is_empty(), "projects must default to []");
    assert_eq!(restored.active_project, 0);
    assert_eq!(
        restored.mode,
        AppMode::Cli,
        "legacy session.json must restore in CLI mode"
    );
}

#[test]
fn test_app_mode_serializes_snake_case() {
    assert_eq!(serde_json::to_string(&AppMode::Cli).unwrap(), "\"cli\"");
    assert_eq!(
        serde_json::to_string(&AppMode::Agents).unwrap(),
        "\"agents\""
    );
    // US-001 (prd-git-diff-mode-2026-Q3.md): the third mode.
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
fn test_project_session_is_expanded_defaults_true_when_absent() {
    // A ProjectSession written before `is_expanded` existed (or with
    // the key stripped) must restore expanded -- otherwise the
    // sidebar would silently hide threads on first relaunch.
    let json = r#"{
        "id": 7,
        "title": "Proj",
        "cwd": "/tmp",
        "threads": []
    }"#;
    let restored: ProjectSession = serde_json::from_str(json).unwrap();
    assert!(restored.is_expanded);
}
