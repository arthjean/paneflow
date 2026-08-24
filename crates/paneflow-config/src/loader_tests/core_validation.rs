use crate::schema::*;

// --- Session persistence round-trip tests (US-017) ---

#[test]
fn validate_layout_converts_legacy_2child_ratio_to_explicit_pair() {
    // U-007: a 2-child split's legacy `ratio` is promoted to an explicit
    // `ratios` pair so it survives restore instead of being dropped.
    let mut node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: Some(0.3),
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
    validate_layout(&mut node);
    match node {
        LayoutNode::Split { ratios, .. } => {
            let rs = ratios.expect("legacy ratio should be promoted to ratios");
            assert_eq!(rs.len(), 2);
            assert!((rs[0] - 0.3).abs() < 1e-6, "first ratio preserved: {rs:?}");
            assert!((rs[1] - 0.7).abs() < 1e-6, "second ratio = 1 - r: {rs:?}");
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn validate_layout_drops_legacy_ratio_on_nary_split() {
    // U-007: an N-ary split's legacy `ratio` is ambiguous; it stays unset
    // (a warn is logged) rather than being silently honored.
    let mut node = LayoutNode::Split {
        direction: "horizontal".to_string(),
        ratio: Some(0.3),
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
    validate_layout(&mut node);
    match node {
        LayoutNode::Split { ratios, .. } => {
            assert!(ratios.is_none(), "N-ary legacy ratio must not be converted")
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn validate_layout_normalizes_unknown_split_direction() {
    let mut node = LayoutNode::Split {
        direction: "diagonal".to_string(),
        ratio: None,
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
    validate_layout(&mut node);
    match node {
        LayoutNode::Split { direction, .. } => assert_eq!(direction, "horizontal"),
        _ => panic!("expected split"),
    }
}

#[test]
fn validate_layout_caps_total_leaves() {
    // U-008/U-016: an over-broad layout is trimmed to MAX_LAYOUT_LEAVES.
    let mut node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: None,
        children: (0..100)
            .map(|_| LayoutNode::Pane {
                surfaces: vec![Default::default()],
            })
            .collect(),
    };
    validate_layout(&mut node);
    assert!(
        node.leaf_count() <= MAX_LAYOUT_LEAVES,
        "got {} leaves",
        node.leaf_count()
    );
}

#[test]
fn validate_layout_caps_deep_trees_without_padding_overshoot() {
    let mut node = LayoutNode::Pane {
        surfaces: vec![Default::default()],
    };
    for _ in 0..60 {
        node = LayoutNode::Split {
            direction: "vertical".to_string(),
            ratio: None,
            ratios: None,
            children: vec![
                node,
                LayoutNode::Pane {
                    surfaces: vec![Default::default()],
                },
            ],
        };
    }

    validate_layout(&mut node);
    assert!(node.leaf_count() <= MAX_LAYOUT_LEAVES);
}

#[test]
fn validate_layout_caps_surfaces_per_pane() {
    // U-008: a pane is one leaf, but each surface spawns a terminal - cap it.
    let mut node = LayoutNode::Pane {
        surfaces: (0..200).map(|_| Default::default()).collect(),
    };
    validate_layout(&mut node);
    match node {
        LayoutNode::Pane { surfaces, .. } => assert!(surfaces.len() <= MAX_PANE_SURFACES),
        _ => panic!("expected pane"),
    }
}

#[test]
fn validate_layout_keeps_only_first_surface_focus() {
    let focused_surface = SurfaceDefinition {
        focus: Some(true),
        ..Default::default()
    };
    let mut node = LayoutNode::Pane {
        surfaces: vec![
            Default::default(),
            focused_surface.clone(),
            focused_surface.clone(),
            Default::default(),
        ],
    };

    validate_layout(&mut node);

    match node {
        LayoutNode::Pane { surfaces, .. } => {
            assert_eq!(surfaces[0].focus, None);
            assert_eq!(surfaces[1].focus, Some(true));
            assert_eq!(surfaces[2].focus, None);
            assert_eq!(surfaces[3].focus, None);
        }
        _ => panic!("expected pane"),
    }
}
