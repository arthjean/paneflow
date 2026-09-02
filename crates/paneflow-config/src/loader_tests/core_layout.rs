use super::super::*;
use crate::schema::*;
use tempfile::NamedTempFile;

#[test]
fn test_split_ratio_clamped_low() {
    let json = r#"{
        "commands": [{
            "name": "test",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "split",
                    "direction": "vertical",
                    "ratio": 0.01,
                    "children": [
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                    ]
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Split { ratio, .. } => {
            assert!((ratio.unwrap() - 0.1).abs() < f64::EPSILON);
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn test_split_ratio_clamped_high() {
    let json = r#"{
        "commands": [{
            "name": "test",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "split",
                    "direction": "vertical",
                    "ratio": 0.99,
                    "children": [
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                    ]
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Split { ratio, .. } => {
            assert!((ratio.unwrap() - 0.9).abs() < f64::EPSILON);
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn test_per_child_ratios_floor_respected_after_normalize() {
    let mut node = LayoutNode::Split {
        direction: "vertical".to_string(),
        ratio: None,
        ratios: Some(vec![100.0, 0.001]),
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
            let rs = ratios.unwrap();
            assert!(
                rs.iter().all(|r| *r >= 0.01),
                "every ratio must respect the 0.01 floor after normalize: {rs:?}"
            );
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn test_split_nary_children_accepted() {
    let json = r#"{
        "commands": [{
            "name": "test",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "split",
                    "direction": "horizontal",
                    "ratios": [0.33, 0.33, 0.34],
                    "children": [
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                    ]
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Split {
            children, ratios, ..
        } => {
            assert_eq!(children.len(), 3);
            let rs = ratios.as_ref().unwrap();
            assert_eq!(rs.len(), 3);
            assert!((rs[0] - 0.33).abs() < f64::EPSILON);
        }
        _ => panic!("expected split"),
    }
}

#[test]
fn test_split_zero_children_falls_back_to_default_pane() {
    let json = r#"{
        "commands": [{
            "name": "test",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "split",
                    "direction": "horizontal",
                    "children": []
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Pane { surfaces } => assert_eq!(surfaces.len(), 1),
        _ => panic!("expected default pane"),
    }
}

#[test]
fn test_pane_no_surfaces_gets_default() {
    let json = r#"{
        "commands": [{
            "name": "test",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "pane",
                    "surfaces": []
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Pane { surfaces } => {
            assert_eq!(surfaces.len(), 1);
            assert_eq!(surfaces[0].surface_type.as_deref(), Some("terminal"));
        }
        _ => panic!("expected pane"),
    }
}

#[test]
fn test_load_from_file() {
    use std::io::Write;
    let mut tmp = NamedTempFile::new().unwrap();
    write!(
        tmp,
        r#"{{"default_shell": "/bin/bash", "commands": [{{"name": "ls", "keywords": [], "command": "ls -la"}}]}}"#
    )
    .unwrap();

    let config = load_config_from_path(tmp.path());
    assert_eq!(config.default_shell, Some("/bin/bash".to_string()));
    assert_eq!(config.commands.len(), 1);
    assert_eq!(config.commands[0].name, "ls");
}

#[test]
fn test_load_from_file_invalid_json() {
    use std::io::Write;
    let mut tmp = NamedTempFile::new().unwrap();
    write!(tmp, "not valid json!!").unwrap();

    let config = load_config_from_path(tmp.path());
    assert_eq!(config, PaneFlowConfig::default());
}

#[test]
fn test_nested_split_validation() {
    let json = r#"{
        "commands": [{
            "name": "nested",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "split",
                    "direction": "horizontal",
                    "ratio": 0.5,
                    "children": [
                        {
                            "type": "split",
                            "direction": "vertical",
                            "ratio": 0.05,
                            "children": [
                                {"type": "pane", "surfaces": [{"surface_type": "terminal"}]},
                                {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                            ]
                        },
                        {"type": "pane", "surfaces": [{"surface_type": "terminal"}]}
                    ]
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Split { children, .. } => match &children[0] {
            LayoutNode::Split { ratio, .. } => {
                assert!((ratio.unwrap() - 0.1).abs() < f64::EPSILON);
            }
            _ => panic!("expected nested split"),
        },
        _ => panic!("expected split"),
    }
}

#[test]
fn test_surface_with_env_and_focus() {
    let json = r#"{
        "commands": [{
            "name": "envtest",
            "keywords": [],
            "workspace": {
                "layout": {
                    "type": "pane",
                    "surfaces": [{
                        "surface_type": "terminal",
                        "name": "main",
                        "command": "cargo run",
                        "cwd": "/tmp",
                        "env": {"RUST_LOG": "debug"},
                        "focus": true
                    }]
                }
            }
        }]
    }"#;
    let config = parse_and_validate(json);
    let ws = config.commands[0].workspace().unwrap();
    match ws.layout.as_ref().unwrap() {
        LayoutNode::Pane { surfaces } => {
            assert_eq!(surfaces.len(), 1);
            let s = &surfaces[0];
            assert_eq!(s.name.as_deref(), Some("main"));
            assert_eq!(s.command.as_deref(), Some("cargo run"));
            assert_eq!(s.cwd.as_deref(), Some("/tmp"));
            assert_eq!(s.focus, Some(true));
            let env = s.env.as_ref().unwrap();
            assert_eq!(env.get("RUST_LOG"), Some(&"debug".to_string()));
        }
        _ => panic!("expected pane"),
    }
}
