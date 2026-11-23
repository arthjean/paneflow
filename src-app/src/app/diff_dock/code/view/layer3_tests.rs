use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui::{Entity, EntityInputHandler, Hsla, Modifiers, TestAppContext, VisualTestContext};
use paneflow_config::schema::{LanguageServerEntry, LanguageServersConfig, PaneFlowConfig};
use paneflow_lsp::ServerState;
use paneflow_lsp::trust::{TrustStore, canonical_root};

use super::super::language_server::{LanguageServerHub, LanguageServerLink};
use super::super::language_server_fake::{
    self as fake, ALLOW_FILE, CRASH_STDERR, INIT_ERROR, LOG_FILE,
};
use super::CodeView;
use super::render::{SERVER_NOTICE_TEXT, ServerNotice, server_notice};
use crate::diff::DiffSyntax;

const TEXT: &str = "fn f(p: P) -> i32 {\n    let total = p.count;\n    total\n}\n";
const FIRST: [u32; 15] = [1, 8, 5, 1, 0, 0, 10, 5, 0, 0, 1, 4, 5, 1, 0];
const AFTER_EDIT: [u32; 5] = [3, 4, 5, 0, 0];
const DEADLINE: Duration = Duration::from_secs(60);

struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    trust: PathBuf,
    state: PathBuf,
}

fn project() -> Project {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = canonical_root(dir.path()).expect("canonical");
    let root = base.join("project");
    std::fs::create_dir_all(root.join("src")).expect("project dir");
    std::fs::write(root.join("src/main.rs"), TEXT).expect("main.rs");
    std::fs::write(root.join("src/lib.rs"), TEXT).expect("lib.rs");
    let state = base.join("fake");
    std::fs::create_dir_all(&state).expect("fake state dir");
    fake::write_tokens(&state, &[FIRST.to_vec(), AFTER_EDIT.to_vec()]);
    let trust = base.join("language-server-trust.json");
    TrustStore::load(&trust)
        .allow("rust-analyzer", &root)
        .expect("allow the fake server");
    Project {
        _dir: dir,
        root,
        trust,
        state,
    }
}

fn config(enabled: bool, command: &str, args: Vec<String>) -> PaneFlowConfig {
    let mut config = PaneFlowConfig::default();
    config.editor.semantic_tokens = Some(enabled);
    config.language_servers = LanguageServersConfig(BTreeMap::from([(
        "rust-analyzer".to_string(),
        LanguageServerEntry {
            command: Some(command.to_string()),
            args: Some(args),
            ..LanguageServerEntry::default()
        },
    )]));
    config
}

fn fake_config(enabled: bool, scenario: &str, project: &Project) -> PaneFlowConfig {
    let command = std::env::current_exe()
        .expect("test binary")
        .to_string_lossy()
        .into_owned();
    config(enabled, &command, fake::args(scenario, &project.state))
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    config: PaneFlowConfig,
    project: &Project,
) -> (Entity<CodeView>, &'a mut VisualTestContext) {
    let trust = project.trust.clone();
    cx.update(move |cx| {
        crate::config_snapshot::publish(&config, cx);
        cx.set_global(LanguageServerHub::with_trust_path(Some(trust)));
    });
    cx.executor().allow_parking();
    super::disk_sync::WATCHER_OFF.with(|off| off.set(true));
    let path = project.root.join("src/main.rs");
    let root = project.root.clone();
    let (view, cx) = cx.add_window_view(move |_window, cx| {
        let mut view = CodeView::new(path, cx);
        view.set_workspace_root(Some(root));
        view
    });
    cx.run_until_parked();
    (view, cx)
}

fn wait_until(
    cx: &mut VisualTestContext,
    what: &str,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + DEADLINE;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "never saw {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn has_layer3(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> bool {
    view.read_with(cx, |view, _| {
        view.highlighter()
            .is_some_and(|highlighter| highlighter.has_semantic_layer())
    })
}

fn layer3(
    view: &Entity<CodeView>,
    row: usize,
    cx: &mut VisualTestContext,
) -> Vec<(std::ops::Range<usize>, Hsla)> {
    view.read_with(cx, |view, _| {
        view.highlighter()
            .map(|highlighter| highlighter.semantic_runs(row))
            .unwrap_or_default()
    })
}

fn painted(
    view: &Entity<CodeView>,
    row: usize,
    cx: &mut VisualTestContext,
) -> Vec<(std::ops::Range<usize>, Hsla)> {
    view.read_with(cx, |view, _| {
        view.highlighter()
            .map(|highlighter| highlighter.runs(row))
            .unwrap_or_default()
    })
}

fn color_at(runs: &[(std::ops::Range<usize>, Hsla)], byte: usize) -> Option<Hsla> {
    runs.iter()
        .find(|(range, _)| range.contains(&byte))
        .map(|(_, color)| *color)
}

fn role(name: &str) -> Option<Hsla> {
    DiffSyntax::from_theme(&crate::theme::active_theme()).color_for_capture(name)
}

fn server_state(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> Option<ServerState> {
    view.read_with(cx, |view, _| {
        view.language_server_link()
            .attachment()
            .and_then(|attachment| attachment.state().cloned())
    })
}

fn notice(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> Option<ServerNotice> {
    view.read_with(cx, |view, _| server_notice(view.language_server_link()))
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn type_at(view: &Entity<CodeView>, at: usize, text: &str, cx: &mut VisualTestContext) {
    view.update_in(cx, |view, window, cx| {
        view.replace_text_in_range(Some(at..at), text, window, cx);
    });
}

fn logged(state: &Path, method: &str) -> usize {
    fake::logged_methods(state)
        .iter()
        .filter(|logged| logged.as_str() == method)
        .count()
}

#[gpui::test]
fn tokens_from_the_server_color_a_field_and_a_local_with_their_own_roles(cx: &mut TestAppContext) {
    let project = project();
    let config = fake_config(true, "tokens", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "layer-3 tokens", |cx| has_layer3(&view, cx));
    let row = painted(&view, 1, cx);
    assert_eq!(color_at(&row, 8), role("variable"), "the local `total`");
    assert_eq!(color_at(&row, 18), role("property"), "the field `count`");
    assert_ne!(role("variable"), role("property"));
    assert_eq!(
        layer3(&view, 1, cx),
        vec![
            (8..13, role("variable").expect("variable")),
            (18..23, role("property").expect("property"))
        ]
    );
}

#[gpui::test]
fn an_edit_shifts_tokens_paints_none_on_edited_rows_then_the_next_response_replaces_them(
    cx: &mut TestAppContext,
) {
    let project = project();
    let config = fake_config(true, "tokens", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "layer-3 tokens", |cx| has_layer3(&view, cx));

    type_at(&view, 0, "\n", cx);
    let variable = role("variable").expect("variable");
    let property = role("property").expect("property");
    assert!(layer3(&view, 0, cx).is_empty(), "edited row 0");
    assert!(layer3(&view, 1, cx).is_empty(), "edited row 1");
    assert_eq!(
        layer3(&view, 2, cx),
        vec![(8..13, variable), (18..23, property)],
        "the unedited row moved from 1 to 2 keeps its tokens"
    );
    assert_eq!(layer3(&view, 3, cx), vec![(4..9, variable)]);

    wait_until(cx, "the response to the edit", |cx| {
        layer3(&view, 3, cx) == vec![(4..9, property)]
    });
    assert!(
        layer3(&view, 2, cx).is_empty(),
        "nothing of the previous response remains"
    );
}

#[gpui::test]
fn a_crash_removes_layer_3_and_shows_a_stopped_banner_with_the_last_stderr_line(
    cx: &mut TestAppContext,
) {
    let project = project();
    let config = fake_config(true, "tokens-then-crash", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "layer-3 tokens", |cx| has_layer3(&view, cx));

    type_at(&view, 0, "x", cx);
    wait_until(cx, "the crashed state", |cx| {
        matches!(server_state(&view, cx), Some(ServerState::Crashed { .. }))
    });
    assert!(!has_layer3(&view, cx), "no layer-3 color remains");
    assert!(layer3(&view, 2, cx).is_empty());
    let banner = notice(&view, cx).expect("a stopped banner");
    assert!(
        banner.text.contains("rust-analyzer stopped"),
        "{}",
        banner.text
    );
    assert!(banner.text.contains(CRASH_STDERR), "{}", banner.text);
    assert!(banner.restart);
    draw(cx);
    assert!(cx.debug_bounds(SERVER_NOTICE_TEXT).is_some());
    assert!(cx.debug_bounds("code-language-server-restart").is_some());
}

#[gpui::test]
fn restart_starts_the_server_again_reopens_the_document_and_removes_the_banner(
    cx: &mut TestAppContext,
) {
    let project = project();
    let config = fake_config(true, "crash-until-allowed", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "the crashed state", |cx| {
        matches!(server_state(&view, cx), Some(ServerState::Crashed { .. }))
    });
    std::fs::write(project.state.join(ALLOW_FILE), b"").expect("let the server run");
    draw(cx);
    let restart = cx
        .debug_bounds("code-language-server-restart")
        .expect("the Restart button is painted");
    cx.simulate_click(restart.center(), Modifiers::default());
    wait_until(cx, "the server running again", |cx| {
        server_state(&view, cx) == Some(ServerState::Running)
    });
    wait_until(cx, "tokens after the restart", |cx| has_layer3(&view, cx));
    assert_eq!(notice(&view, cx), None);
    draw(cx);
    assert!(
        cx.debug_bounds(SERVER_NOTICE_TEXT).is_none(),
        "the banner is gone at the next frame"
    );
    assert_eq!(logged(&project.state, "textDocument/didOpen"), 3);
}

#[gpui::test]
fn an_initialize_error_shows_a_failed_banner_with_the_message(cx: &mut TestAppContext) {
    let project = project();
    let config = fake_config(true, "init-error", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "the failed state", |cx| {
        matches!(server_state(&view, cx), Some(ServerState::Failed { .. }))
    });
    let banner = notice(&view, cx).expect("a failed banner");
    assert!(banner.text.contains(INIT_ERROR), "{}", banner.text);
    draw(cx);
    assert!(cx.debug_bounds(SERVER_NOTICE_TEXT).is_some());
}

#[gpui::test]
fn a_missing_server_is_named_once_per_root_in_a_session(cx: &mut TestAppContext) {
    let project = project();
    let config = config(true, "paneflow-no-such-language-server", Vec::new());
    let (view, cx) = open(cx, config, &project);
    let banner = notice(&view, cx).expect("a missing banner");
    assert!(
        banner.text.contains("paneflow-no-such-language-server"),
        "{}",
        banner.text
    );
    assert!(banner.text.contains("not found"), "{}", banner.text);
    assert!(!banner.restart);
    draw(cx);
    assert!(cx.debug_bounds(SERVER_NOTICE_TEXT).is_some());

    let root = project.root.clone();
    let other = cx.update(|_window, cx| {
        gpui::AppContext::new(cx, move |cx| {
            let mut view = CodeView::new(root.join("src/lib.rs"), cx);
            view.set_workspace_root(Some(root));
            view
        })
    });
    cx.run_until_parked();
    assert_eq!(notice(&other, cx), None, "the same root does not ask again");
    assert!(other.read_with(cx, |view, _| matches!(
        view.language_server_link(),
        LanguageServerLink::Off
    )));
}

#[gpui::test]
fn a_server_without_semantic_tokens_shows_no_banner_and_gets_no_request(cx: &mut TestAppContext) {
    let project = project();
    let config = fake_config(true, "no-tokens", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "the running state", |cx| {
        server_state(&view, cx) == Some(ServerState::Running)
    });
    wait_until(cx, "the didOpen", |_| {
        logged(&project.state, "textDocument/didOpen") == 1
    });
    assert_eq!(notice(&view, cx), None);
    assert!(!has_layer3(&view, cx));
    assert_eq!(
        logged(&project.state, "textDocument/semanticTokens/full"),
        0
    );
}

#[gpui::test]
fn turning_semantic_tokens_off_removes_layer_3_at_the_next_paint(cx: &mut TestAppContext) {
    let project = project();
    let config = fake_config(true, "tokens", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "layer-3 tokens", |cx| has_layer3(&view, cx));

    let off = fake_config(false, "tokens", &project);
    cx.update(|_window, cx| crate::config_snapshot::publish(&off, cx));
    view.update(cx, |view, cx| view.refresh_language_server(cx));
    draw(cx);
    assert!(!has_layer3(&view, cx));
    assert!(layer3(&view, 1, cx).is_empty());
    let syntax_only = view.read_with(cx, |view, _| {
        view.highlighter()
            .map(|highlighter| highlighter.syntax_runs(1))
            .unwrap_or_default()
    });
    assert_eq!(
        painted(&view, 1, cx),
        syntax_only,
        "the row paints layers 2 and 1 only"
    );
}

#[gpui::test]
fn with_semantic_tokens_off_an_edit_allocates_no_layer_3_storage(cx: &mut TestAppContext) {
    let project = project();
    let config = fake_config(false, "tokens", &project);
    let (view, cx) = open(cx, config, &project);
    type_at(&view, 0, "// edit\n", cx);
    draw(cx);
    assert!(!has_layer3(&view, cx));
    assert!(view.read_with(cx, |view, _| matches!(
        view.language_server_link(),
        LanguageServerLink::Off
    )));
    assert!(!project.state.join(LOG_FILE).exists(), "no server started");
}

#[gpui::test]
fn tokens_for_an_older_revision_never_reach_the_layer_3_store(cx: &mut TestAppContext) {
    let project = project();
    let config = fake_config(true, "tokens", &project);
    let (view, cx) = open(cx, config, &project);
    wait_until(cx, "layer-3 tokens", |cx| has_layer3(&view, cx));
    type_at(&view, 0, "\n", cx);
    let before = layer3(&view, 2, cx);
    let stale = [paneflow_lsp::semantic_tokens::SemanticToken {
        line: 2,
        start: 0,
        end: 1,
        token_type: 0,
        modifiers: 0,
    }];
    let legend = paneflow_lsp::protocol::TokenLegend {
        token_types: fake::LEGEND.iter().map(|name| name.to_string()).collect(),
        token_modifiers: Vec::new(),
    };
    view.update(cx, |view, cx| {
        let document = view
            .language_server_link()
            .attachment()
            .expect("attached")
            .document();
        view.apply_semantic_tokens(document, 0, &stale, &legend, cx);
    });
    assert_eq!(
        layer3(&view, 2, cx),
        before,
        "revision 0 is older than the edit"
    );
}
