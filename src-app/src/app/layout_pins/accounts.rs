use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{Entity, TestAppContext, VisualTestContext};
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use paneflow_config::schema::{SpaceConfig, SpaceId};

use super::forget_the_previous_pin;
use crate::PaneFlowApp;
use crate::app::bootstrap::Startup;
use crate::app::fake_host::FakeHost;
use crate::terminal::view::DisplayOnlyTerminals;

pub(super) const PINS: usize = 1;

const ACCOUNT_WAIT: Duration = Duration::from_secs(20);

fn mount(cx: &mut TestAppContext) -> (Entity<PaneFlowApp>, &mut VisualTestContext) {
    forget_the_previous_pin();
    cx.executor().allow_parking();
    cx.update(|cx| {
        cx.set_global(DisplayOnlyTerminals);
        cx.set_global(FakeHost::default());
    });
    let (app, cx) = cx.add_window_view(|_, cx| PaneFlowApp::start(Startup::Conformance, cx));
    cx.run_until_parked();
    (app, cx)
}

fn has_account(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
    agent: UsageAgent,
    dir: &Path,
) -> bool {
    let key = account_dir_key(&canonical_account_dir(dir));
    cx.update(|_, cx| {
        app.read(cx)
            .agent_accounts(agent)
            .iter()
            .any(|row| row.key == key)
    })
}

fn wait_for_account(
    app: &Entity<PaneFlowApp>,
    cx: &mut VisualTestContext,
    agent: UsageAgent,
    dir: &Path,
) -> bool {
    let deadline = Instant::now() + ACCOUNT_WAIT;
    loop {
        cx.run_until_parked();
        if has_account(app, cx, agent, dir) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui::test]
#[ignore = "run by every_layout_pin_holds_in_an_isolated_home"]
fn pin_a_space_folder_its_agent_setup_creates_gets_its_account_row(cx: &mut TestAppContext) {
    let (app, cx) = mount(cx);
    let scratch = tempfile::tempdir().expect("a scratch folder");
    let claude = scratch.path().join("claude-work");
    std::fs::create_dir(&claude).expect("the Claude folder");
    let codex = scratch.path().join("codex-work");
    let space = SpaceId::new();
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.cached_config.spaces.push(SpaceConfig {
                id: space.clone(),
                name: "Work".to_string(),
                color: None,
                avatar: None,
                env: BTreeMap::from([
                    (
                        "CLAUDE_CONFIG_DIR".to_string(),
                        claude.to_string_lossy().into_owned(),
                    ),
                    (
                        "CODEX_HOME".to_string(),
                        codex.to_string_lossy().into_owned(),
                    ),
                ]),
            });
            app.refresh_usage_inventory(true, cx);
        });
    });
    assert!(
        wait_for_account(&app, cx, UsageAgent::ClaudeCode, &claude),
        "the space's existing Claude Code folder is an account"
    );
    assert!(
        !has_account(&app, cx, UsageAgent::Codex, &codex),
        "a Codex folder that does not exist yet is not an account"
    );

    std::fs::create_dir(&codex).expect("the agent setup creates the Codex folder");
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            app.space_integrations_settled(&space, vec![("codex".to_string(), Ok(()))], cx);
        });
    });
    assert!(
        wait_for_account(&app, cx, UsageAgent::Codex, &codex),
        "once the space's agent setup created its Codex folder, the folder is an account and its row stops checking"
    );
}
