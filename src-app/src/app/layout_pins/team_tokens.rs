use gpui::TestAppContext;

use super::{Rig, assert_isolated_home, isolated_child};
use crate::NewTab;
use crate::agent_launcher::TerminalAgent;
use crate::app::pane_palette::Preset;
use crate::layout::SplitDirection;
use crate::pane::PaneEvent;
use crate::tmux_compat::TEAM_ENV;
use crate::tmux_compat::teams::LEADER;

pub(super) const FILTER: &str = "app::layout_pins::team_tokens::";
const TEAM_LABEL: &str = "Claude Code (team)";
const PINS: usize = 4;

#[cfg(unix)]
fn stand_in_claude(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(TerminalAgent::ClaudeCode.binary());
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("the stand-in claude is written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the stand-in claude is executable");
}

#[test]
fn every_team_entry_point_gives_its_lead_a_team_token() {
    if cfg!(windows) {
        let config = paneflow_config::schema::PaneFlowConfig::default();
        assert!(
            crate::agent_launcher::AgentLaunch::all(&config)
                .iter()
                .all(|launch| !launch.tmux_compat()),
            "the team preset has no Windows path"
        );
        return;
    }
    let home = tempfile::tempdir().expect("a pin home");
    let bin = tempfile::tempdir().expect("a directory for the stand-in claude");
    #[cfg(unix)]
    stand_in_claude(bin.path());
    let path = std::env::join_paths(std::iter::once(bin.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("a PATH with the stand-in claude first");
    let output = isolated_child(home.path(), &["--ignored", "--test-threads=1", FILTER])
        .env("PATH", path)
        .output()
        .expect("the pin runner starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a team token pin failed\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
    );
    let passed = stdout
        .lines()
        .filter(|line| {
            line.starts_with("test app::layout_pins::team_tokens::") && line.ends_with(" ok")
        })
        .count();
    assert_eq!(passed, PINS, "{stdout}");
}

fn mount_with_the_team_preset(cx: &mut TestAppContext) -> (Rig<'_>, usize, Preset) {
    assert_isolated_home();
    crate::agent_launcher::refresh_installed_binaries();
    assert!(
        TerminalAgent::ClaudeCode.is_installed(),
        "the runner puts a stand-in claude first on PATH"
    );
    let mut rig = Rig::mount(cx);
    let ws = rig.workspace("alpha");
    rig.requests();
    let preset = rig.act(|app, _, _| {
        app.pane_palette_presets(ws)
            .into_iter()
            .find(|preset| preset.label == TEAM_LABEL)
            .expect("the team preset is offered")
    });
    (rig, ws, preset)
}

fn team_index(rig: &mut Rig<'_>, ws: usize) -> usize {
    rig.act(|app, _, _| {
        app.pane_palette_presets(ws)
            .iter()
            .position(|preset| preset.label == TEAM_LABEL)
            .expect("the team preset is offered")
    })
}

fn assert_the_lead_holds_a_team_token(rig: &mut Rig<'_>, lead: &str) {
    assert_eq!(rig.requests(), [format!("create {lead}")]);
    let terminal = rig.terminal(lead);
    let surface = rig.surface_id(lead);
    let token = rig
        .cx
        .read(|cx| terminal.read(cx).launch_env())
        .and_then(|env| env.get(TEAM_ENV).cloned())
        .unwrap_or_else(|| panic!("the lead {lead} was started without {TEAM_ENV}"));
    let entered = rig.act(|app, _, _| app.tmux_teams.enter(&token, surface, |_| true).ok());
    assert_eq!(
        entered,
        Some(LEADER),
        "the token names a team Paneflow issued"
    );
}

#[gpui::test]
#[ignore = "run by every_team_entry_point_gives_its_lead_a_team_token"]
fn pin_the_new_tab_action_opens_a_team_lead_with_a_token(cx: &mut TestAppContext) {
    let (mut rig, ws, _) = mount_with_the_team_preset(cx);
    let index = team_index(&mut rig, ws);
    rig.act(|app, window, cx| {
        app.handle_new_tab(&NewTab, window, cx);
        app.pane_palette_launch(index, window, cx);
    });
    assert_the_lead_holds_a_team_token(&mut rig, "b");
}

#[gpui::test]
#[ignore = "run by every_team_entry_point_gives_its_lead_a_team_token"]
fn pin_the_sidebar_new_tab_opens_a_team_lead_with_a_token(cx: &mut TestAppContext) {
    let (mut rig, ws, _) = mount_with_the_team_preset(cx);
    let index = team_index(&mut rig, ws);
    rig.act(|app, window, cx| {
        app.open_pane_palette(ws, window, cx);
        app.pane_palette_launch(index, window, cx);
    });
    assert_the_lead_holds_a_team_token(&mut rig, "b");
}

#[gpui::test]
#[ignore = "run by every_team_entry_point_gives_its_lead_a_team_token"]
fn pin_the_split_palette_opens_a_team_lead_with_a_token(cx: &mut TestAppContext) {
    let (mut rig, ws, _) = mount_with_the_team_preset(cx);
    let index = team_index(&mut rig, ws);
    let pane = rig.pane("a");
    rig.act(|app, window, cx| {
        app.open_split_palette(pane, SplitDirection::Vertical, cx);
        app.pane_palette_launch(index, window, cx);
    });
    assert_the_lead_holds_a_team_token(&mut rig, "b");
}

#[gpui::test]
#[ignore = "run by every_team_entry_point_gives_its_lead_a_team_token"]
fn pin_the_pane_new_tab_menu_opens_a_team_lead_with_a_token(cx: &mut TestAppContext) {
    let (mut rig, _, preset) = mount_with_the_team_preset(cx);
    rig.emit("a", PaneEvent::NewTabPreset(preset));
    assert_the_lead_holds_a_team_token(&mut rig, "b");
}
