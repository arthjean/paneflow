use super::super::*;
use crate::schema::*;
use std::collections::HashMap;

#[test]
fn test_serialization_roundtrip() {
    let config = PaneFlowConfig {
        shortcuts: {
            let mut m = HashMap::new();
            m.insert("ctrl+n".to_string(), "new_window".to_string());
            m
        },
        default_shell: Some("/bin/fish".to_string()),
        theme: Some("One Dark".to_string()),
        theme_mode: Some("dark".to_string()),
        commands: vec![CommandDefinition {
            name: "test".to_string(),
            description: Some("A test command".to_string()),
            keywords: vec!["test".to_string()],
            target: CommandTarget::Shell {
                command: "echo hello".to_string(),
            },
        }],
        window_decorations: None,
        window_backdrop: None,
        windows_terminal_material: None,
        windows_chrome_material: None,
        macos_chrome_material: None,
        unfocused_pane_opacity: None,
        reduce_motion: None,
        sidebar_density: None,
        line_height: None,
        cell_width: None,
        font_family: None,
        font_fallbacks: None,
        font_size: None,
        font_weight: None,
        option_as_meta: None,
        shell_integration: None,
        agent_stall_detection: None,
        agent_stall_threshold_secs: None,
        review_prefill_delay_ms: None,
        submit_paste_delay_ms: None,
        claude_code_bypass_permissions: None,
        ai_unrestricted: None,
        ai_injection_fence: None,
        claude_code_button_visible: None,
        codex_button_visible: None,
        opencode_button_visible: None,
        pi_button_visible: None,
        hermes_agent_button_visible: None,
        grok_button_visible: None,
        amp_button_visible: None,
        cursor_button_visible: None,
        gemini_button_visible: None,
        kiro_button_visible: None,
        antigravity_button_visible: None,
        copilot_button_visible: None,
        codebuddy_button_visible: None,
        factory_button_visible: None,
        qoder_button_visible: None,
        openclaw_button_visible: None,
        telemetry: None,
        terminal: None,
        agent_panel: None,
        external_editor: None,
        tool_permissions: HashMap::new(),
    };

    let json = serde_json::to_string_pretty(&config).unwrap();
    let reparsed = parse_and_validate(&json);
    assert_eq!(config, reparsed);
}

#[test]
fn test_nary_layout_roundtrip() {
    // Build a 6-pane N-ary layout: 3 panes horizontal on top, 3 on bottom.
    let make_pane = |name: &str| LayoutNode::Pane {
        surfaces: vec![SurfaceDefinition {
            surface_type: Some("terminal".to_string()),
            name: Some(name.to_string()),
            ..Default::default()
        }],
    };

    let top_row = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![0.33, 0.33, 0.34]),
        children: vec![make_pane("A"), make_pane("B"), make_pane("C")],
    };
    let bottom_row = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![0.33, 0.33, 0.34]),
        children: vec![make_pane("D"), make_pane("E"), make_pane("F")],
    };
    let root = LayoutNode::Split {
        direction: "horizontal".to_string(),
        ratio: None,
        ratios: Some(vec![0.5, 0.5]),
        children: vec![top_row, bottom_row],
    };

    // Serialize to JSON and back.
    let json = serde_json::to_string_pretty(&root).unwrap();
    let deserialized: LayoutNode = serde_json::from_str(&json).unwrap();
    assert_eq!(root, deserialized);
}

#[test]
fn test_legacy_binary_still_works() {
    // Legacy format with single `ratio` field (no `ratios`).
    let json = r#"{
        "type": "split",
        "direction": "horizontal",
        "ratio": 0.6,
        "children": [
            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
            {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
        ]
    }"#;
    let node: LayoutNode = serde_json::from_str(json).unwrap();
    match &node {
        LayoutNode::Split {
            ratio,
            ratios,
            children,
            ..
        } => {
            assert_eq!(*ratio, Some(0.6));
            assert!(ratios.is_none());
            assert_eq!(children.len(), 2);
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn test_layout_node_leaf_count() {
    let single = LayoutNode::Pane {
        surfaces: vec![Default::default()],
    };
    assert_eq!(single.leaf_count(), 1);

    // 3-child flat split = 3 leaves
    let flat = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![0.33, 0.33, 0.34]),
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    assert_eq!(flat.leaf_count(), 3);

    // Nested: 2 rows of 3 = 6 leaves
    let nested = LayoutNode::Split {
        direction: "horizontal".to_string(),
        ratio: None,
        ratios: Some(vec![0.5, 0.5]),
        children: vec![flat.clone(), flat],
    };
    assert_eq!(nested.leaf_count(), 6);
}

#[test]
fn test_resolved_ratios_nary() {
    let node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![0.25, 0.25, 0.5]),
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    assert_eq!(node.resolved_ratios(), vec![0.25, 0.25, 0.5]);
}

#[test]
fn test_resolved_ratios_legacy_binary() {
    let node = LayoutNode::Split {
        direction: "horizontal".to_string(),
        ratio: Some(0.6),
        ratios: None,
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    let rs = node.resolved_ratios();
    assert_eq!(rs.len(), 2);
    assert!((rs[0] - 0.6).abs() < f64::EPSILON);
    assert!((rs[1] - 0.4).abs() < f64::EPSILON);
}

#[test]
fn test_resolved_ratios_rejects_nan_and_negative() {
    // US-056: a corrupt session.json can carry NaN/negative/out-of-range
    // ratios. They must be clamped, normalized, and never propagate.
    let node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![f64::NAN, -0.5, 2.0]),
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    let rs = node.resolved_ratios();
    assert_eq!(rs.len(), 3);
    // EP-010 review: post-US-057-parity invariant. NaN/negative are floored,
    // 2.0 is clamped to 1.0; every ratio is finite and in `[0.01, 1.0]`. The
    // post-normalize re-clamp (matching `validate_layout`) keeps the floor,
    // so the sum is ~1.0 but not exactly - the renderer re-normalizes at
    // paint. Assert the floor + a sane sum band, not `== 1.0`.
    assert!(rs
        .iter()
        .all(|&r| r.is_finite() && (0.01 - 1e-9..=1.0).contains(&r)));
    let sum: f64 = rs.iter().sum();
    assert!(
        (sum - 1.0).abs() < 0.05,
        "ratios must stay near 1.0, got {sum}"
    );
}

#[test]
fn test_resolved_ratios_floor_respected_after_normalize() {
    // EP-010 review: the SESSION path (`resolved_ratios` -> `sanitize_ratios`)
    // must honour the 0.01 floor AFTER normalize, matching the config path
    // (`validate_layout`, see `test_per_child_ratios_floor_respected_after_normalize`).
    // `[1.0, 0.005]` clamps to `[1.0, 0.01]` (sum 1.01); normalizing alone
    // would push the second child to ~0.0099 - below the floor. The
    // post-normalize re-clamp must pull it back to 0.01.
    let node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![1.0, 0.005]),
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    let rs = node.resolved_ratios();
    assert_eq!(rs.len(), 2);
    assert!(
        rs.iter().all(|&r| r >= 0.01 - 1e-9),
        "every ratio must stay at/above the 0.01 floor after normalize, got {rs:?}"
    );
}

#[test]
fn test_resolved_ratios_length_mismatch_falls_back() {
    // US-056: a ratios array whose length disagrees with the child count
    // is unrecoverable -> equal shares, never a panic or stale mapping.
    let node = LayoutNode::Split {
        direction: "horizontal".to_string(),
        ratio: None,
        ratios: Some(vec![0.9]), // 1 ratio, 2 children
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    let rs = node.resolved_ratios();
    assert_eq!(rs, vec![0.5, 0.5]);
}

#[test]
fn test_resolved_ratios_fallback_equal() {
    let node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: None,
        children: vec![
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
            LayoutNode::Pane {
                surfaces: vec![Default::default()],
            },
        ],
    };
    let rs = node.resolved_ratios();
    assert_eq!(rs.len(), 3);
    for r in &rs {
        assert!((r - 1.0 / 3.0).abs() < f64::EPSILON);
    }
}
