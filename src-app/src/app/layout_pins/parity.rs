use gpui::TestAppContext;
use paneflow_config::schema::{LayoutNode, SessionState};
use serde_json::{Value, json};

use super::Rig;

pub(super) const PINS: usize = 1;

fn pane(name: &str) -> Value {
    json!({"type": "pane", "surfaces": [{"surface_type": "terminal", "custom_name": name}]})
}

fn split(direction: &str, ratios: &[f64], children: Vec<Value>) -> Value {
    json!({"type": "split", "direction": direction, "ratios": ratios, "children": children})
}

fn tab(title: &str, layout: Value) -> Value {
    json!({"title": title, "title_source": "user", "layout": layout})
}

fn workspace(title: &str, tabs: Vec<Value>, active_tab: usize) -> Value {
    json!({"title": title, "cwd": "", "tabs": tabs, "active_tab": active_tab})
}

fn previous_release_session() -> SessionState {
    let session = json!({
        "version": 3,
        "active_workspace": 1,
        "workspaces": [
            workspace("api", vec![
                tab("server", split("vertical", &[0.3, 0.7], vec![pane("p01"), pane("p02")])),
                tab("tests", split("horizontal", &[0.25, 0.25, 0.5], vec![pane("p03"), pane("p04"), pane("p05")])),
            ], 1),
            workspace("web", vec![
                tab("dev", pane("p06")),
                tab("build", split("vertical", &[0.6, 0.4], vec![
                    split("horizontal", &[0.5, 0.5], vec![pane("p07"), pane("p08")]),
                    pane("p09"),
                ])),
                tab("logs", split("horizontal", &[0.4, 0.6], vec![pane("p10"), pane("p11")])),
            ], 2),
            workspace("infra", vec![
                tab("grid", split("vertical", &[0.5, 0.5], vec![
                    split("horizontal", &[0.7, 0.3], vec![pane("p12"), pane("p13")]),
                    split("horizontal", &[0.2, 0.8], vec![pane("p14"), pane("p15")]),
                ])),
                tab("pair", split("vertical", &[0.45, 0.55], vec![pane("p16"), pane("p17")])),
                tab("trio", split("vertical", &[0.2, 0.3, 0.5], vec![pane("p18"), pane("p19"), pane("p20")])),
            ], 0),
        ],
        "detached_panes": [{
            "location": {"mode": "cli", "workspace": 2, "tab": 1, "leaf": 1},
            "layout_leaf_count": 2,
            "x": 80.0, "y": 60.0, "width": 720.0, "height": 480.0
        }]
    });
    serde_json::from_value(session).expect("a session written by the previous release")
}

fn render(node: &LayoutNode) -> String {
    match node {
        LayoutNode::Pane { surfaces } => surfaces
            .iter()
            .map(|surface| surface.custom_name.clone().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("+"),
        LayoutNode::Split {
            direction,
            children,
            ..
        } => {
            let letter = if direction == "vertical" { "V" } else { "H" };
            let parts: Vec<String> = children
                .iter()
                .zip(node.resolved_ratios())
                .map(|(child, ratio)| format!("{ratio:.3} {}", render(child)))
                .collect();
            format!("{letter}({})", parts.join(", "))
        }
    }
}

fn outline(state: &SessionState) -> Vec<String> {
    let mut lines = vec![format!("active workspace {}", state.active_workspace)];
    for ws in &state.workspaces {
        lines.push(format!("{} active tab {}", ws.title, ws.active_tab));
        for tab in &ws.tabs {
            let layout = tab.layout.as_ref().map_or_else(String::new, render);
            lines.push(format!("  {} = {layout}", tab.title));
        }
    }
    for placement in &state.detached_panes {
        let location = placement.location;
        lines.push(format!(
            "detached {}/{}/{} at {}x{} {}x{}",
            location.workspace,
            location.tab,
            location.leaf,
            placement.x,
            placement.y,
            placement.width,
            placement.height
        ));
    }
    lines
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_session_saved_by_the_previous_release_restores_unchanged(cx: &mut TestAppContext) {
    let original = previous_release_session();
    let panes: usize = original
        .workspaces
        .iter()
        .flat_map(|ws| ws.tabs.iter())
        .filter_map(|tab| tab.layout.as_ref())
        .map(|layout| render(layout).matches('p').count())
        .sum();
    assert_eq!(original.workspaces.len(), 3);
    assert_eq!(
        original
            .workspaces
            .iter()
            .map(|ws| ws.tabs.len())
            .sum::<usize>(),
        8
    );
    assert_eq!(panes, 20);
    let mut rig = Rig::mount_restored(cx, &original, Vec::new());
    rig.act(|app, window, cx| app.restore_detached_panes(window, cx));
    let restored = rig.state();
    assert_eq!(outline(&restored), outline(&original));
    let zoomed = rig.cx.update(|_, cx| {
        rig.app
            .read(cx)
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs().iter())
            .any(crate::workspace::Tab::is_zoomed)
    });
    assert!(
        !zoomed,
        "no tab was zoomed in the file and none is after restore"
    );
    assert_eq!(restored.detached_panes.len(), 1);
    assert!(
        restored.detached_panes[0].pane.is_some(),
        "the placement is keyed by pane id"
    );
    assert!(restored.next_layout_id.is_some());
    assert!(
        restored
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .all(|tab| tab.layout_ids.is_some()),
        "every tab now carries its ids"
    );
}
