use std::path::PathBuf;

use gpui::Context;
use paneflow_lsp::TextChange;

use super::super::language_server::{
    self, CheckOutcome, LanguageServerLink, attach, check_blocking, remember_consent_blocking,
};
use super::super::spawn_blocking_then;
use super::CodeView;

impl CodeView {
    pub(crate) fn set_workspace_root(&mut self, root: Option<PathBuf>) {
        self.workspace_root = root;
    }

    pub(crate) fn language_server_link(&self) -> &LanguageServerLink {
        &self.language_server
    }

    pub(crate) fn refresh_language_server(&mut self, cx: &mut Context<Self>) {
        let config = crate::config_snapshot::current(cx);
        let definition = self
            .state
            .document()
            .and_then(|doc| language_server::server_for(&config, doc.ext()));
        let Some(definition) = definition else {
            self.detach_language_server(cx);
            return;
        };
        if !matches!(self.language_server, LanguageServerLink::Off) {
            return;
        }
        let Some(root) = self.workspace_root.clone() else {
            return;
        };
        let path = self.path.clone();
        let trust_path = language_server::hub(cx).trust_path();
        self.language_server_generation += 1;
        let generation = self.language_server_generation;
        self.language_server = LanguageServerLink::Checking(generation);
        spawn_blocking_then(
            cx,
            move || check_blocking(definition, &root, &path, trust_path.as_deref()),
            move |view: &mut Self, outcome, cx| {
                if matches!(view.language_server, LanguageServerLink::Checking(at) if at == generation)
                {
                    view.apply_language_server_check(outcome, cx);
                }
            },
        );
    }

    fn apply_language_server_check(&mut self, outcome: CheckOutcome, cx: &mut Context<Self>) {
        match outcome {
            CheckOutcome::OutsideRoot => self.language_server = LanguageServerLink::Off,
            CheckOutcome::Missing { key, command } => {
                self.language_server = LanguageServerLink::Off;
                if let Some(client) = language_server::hub(cx).client() {
                    client.report_missing(key, command);
                }
            }
            CheckOutcome::Allowed(plan) => {
                self.language_server = LanguageServerLink::NeedsConsent(Box::new(plan));
                self.attach_consented_language_server(cx);
            }
            CheckOutcome::NeedsConsent(plan)
                if language_server::hub(cx).is_accepted(&plan.pair) =>
            {
                self.language_server = LanguageServerLink::NeedsConsent(Box::new(plan));
                self.attach_consented_language_server(cx);
            }
            CheckOutcome::NeedsConsent(plan) => {
                if language_server::hub(cx).is_declined(&plan.pair) {
                    self.language_server = LanguageServerLink::Off;
                    return;
                }
                let pair = plan.pair.clone();
                self.language_server = LanguageServerLink::NeedsConsent(Box::new(plan));
                let weak = cx.weak_entity();
                language_server::hub(cx).wait_for_answer(pair, weak);
                cx.notify();
            }
        }
    }

    fn attach_consented_language_server(&mut self, cx: &mut Context<Self>) {
        let LanguageServerLink::NeedsConsent(plan) = std::mem::take(&mut self.language_server)
        else {
            return;
        };
        if let Some(text) = self.state.document().map(|doc| doc.text().clone())
            && let Some(attachment) = attach(language_server::hub(cx), *plan, text)
        {
            self.language_server = LanguageServerLink::Attached(attachment);
        }
        cx.notify();
    }

    pub(crate) fn detach_language_server(&mut self, cx: &mut Context<Self>) {
        if matches!(self.language_server, LanguageServerLink::Off) {
            return;
        }
        self.language_server = LanguageServerLink::Off;
        cx.notify();
    }

    pub(super) fn start_language_server(&mut self, cx: &mut Context<Self>) {
        let Some(pair) = self.language_server.consent().cloned() else {
            return;
        };
        let trust_path = language_server::hub(cx).trust_path();
        let me = cx.entity_id();
        let others: Vec<_> = language_server::hub(cx)
            .accept(pair.clone())
            .into_iter()
            .filter(|view| view.entity_id() != me)
            .collect();
        spawn_blocking_then(
            cx,
            move || remember_consent_blocking(trust_path.as_deref(), &pair),
            move |view: &mut Self, (), cx| {
                view.attach_consented_language_server(cx);
                for other in others {
                    let _ =
                        other.update(cx, |other, cx| other.attach_consented_language_server(cx));
                }
            },
        );
    }

    pub(super) fn decline_language_server(&mut self, cx: &mut Context<Self>) {
        let Some(pair) = self.language_server.consent().cloned() else {
            return;
        };
        let me = cx.entity_id();
        let others = language_server::hub(cx).decline(pair);
        self.detach_language_server(cx);
        for other in others.into_iter().filter(|view| view.entity_id() != me) {
            let _ = other.update(cx, |other, cx| other.detach_language_server(cx));
        }
    }

    pub(super) fn wants_language_server_edits(&self) -> bool {
        self.language_server.attachment().is_some()
    }

    pub(super) fn send_language_server_edits(&self, changes: Vec<TextChange>) {
        let (Some(attachment), Some(doc)) =
            (self.language_server.attachment(), self.state.document())
        else {
            return;
        };
        if self.replacing_from_disk {
            attachment.replace(doc.text().clone());
        } else {
            attachment.change(changes, doc.text().clone());
        }
    }

    pub(super) fn language_server_document_replaced(&mut self, cx: &mut Context<Self>) {
        match (self.language_server.attachment(), self.state.document()) {
            (Some(attachment), Some(doc)) => attachment.replace(doc.text().clone()),
            _ => self.refresh_language_server(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use gpui::{
        AppContext, Entity, EntityInputHandler, Modifiers, TestAppContext, VisualTestContext, px,
        size,
    };
    use paneflow_config::schema::{LanguageServerEntry, LanguageServersConfig, PaneFlowConfig};
    use paneflow_lsp::trust::{TrustStore, canonical_root};
    use paneflow_lsp::{ServerState, TextChange};

    use super::super::super::language_server::{
        LanguageServerHub, LanguageServerLink, Sent, SentLog,
    };
    use super::super::render::{CONSENT_BUTTONS, CONSENT_TEXT, consent_text};
    use super::CodeView;

    const PROBE_ARGS: [&str; 2] = ["--exact", "language_server_probe_matches_no_test"];

    struct Project {
        _dir: tempfile::TempDir,
        root: PathBuf,
        trust: PathBuf,
    }

    fn project() -> Project {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = canonical_root(dir.path())
            .expect("canonical")
            .join("project");
        std::fs::create_dir_all(root.join("src")).expect("project dir");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main.rs");
        std::fs::write(root.join("src/lib.rs"), "pub fn lib() {}\n").expect("lib.rs");
        let trust = dir.path().join("language-server-trust.json");
        Project {
            _dir: dir,
            root,
            trust,
        }
    }

    fn config(enabled: bool, command: &str) -> PaneFlowConfig {
        let mut config = PaneFlowConfig::default();
        config.editor.semantic_tokens = Some(enabled);
        config.language_servers = LanguageServersConfig(BTreeMap::from([(
            "rust-analyzer".to_string(),
            LanguageServerEntry {
                command: Some(command.to_string()),
                args: Some(PROBE_ARGS.iter().map(|arg| arg.to_string()).collect()),
                ..LanguageServerEntry::default()
            },
        )]));
        config
    }

    fn probe_command() -> String {
        std::env::current_exe()
            .expect("test binary")
            .to_string_lossy()
            .into_owned()
    }

    fn install(cx: &mut TestAppContext, config: &PaneFlowConfig, trust: &Path) -> SentLog {
        let sent = SentLog::default();
        let trust = trust.to_path_buf();
        let config = config.clone();
        let log = sent.clone();
        cx.update(move |cx| {
            crate::config_snapshot::publish(&config, cx);
            let mut hub = LanguageServerHub::with_trust_path(Some(trust));
            hub.sent = Some(log);
            cx.set_global(hub);
        });
        sent
    }

    fn open_first<'a>(
        cx: &'a mut TestAppContext,
        path: PathBuf,
        root: &Path,
    ) -> (Entity<CodeView>, &'a mut VisualTestContext) {
        super::super::disk_sync::WATCHER_OFF.with(|off| off.set(true));
        let root = root.to_path_buf();
        let (view, cx) = cx.add_window_view(move |_window, cx| {
            let mut view = CodeView::new(path, cx);
            view.set_workspace_root(Some(root));
            view
        });
        cx.run_until_parked();
        (view, cx)
    }

    fn open_another(cx: &mut VisualTestContext, path: PathBuf, root: &Path) -> Entity<CodeView> {
        let root = root.to_path_buf();
        let view = cx.update(|_window, cx| {
            cx.new(move |cx| {
                let mut view = CodeView::new(path, cx);
                view.set_workspace_root(Some(root));
                view
            })
        });
        cx.run_until_parked();
        view
    }

    fn running_client(cx: &mut VisualTestContext) -> bool {
        cx.update(|_window, cx| {
            cx.try_global::<LanguageServerHub>()
                .and_then(LanguageServerHub::running_client)
                .is_some()
        })
    }

    fn is_off(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> bool {
        view.read_with(cx, |view, _| {
            matches!(view.language_server_link(), LanguageServerLink::Off)
        })
    }

    fn is_attached(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> bool {
        view.read_with(cx, |view, _| {
            view.language_server_link().attachment().is_some()
        })
    }

    fn wait_for_server_state(view: &Entity<CodeView>, cx: &mut VisualTestContext) -> ServerState {
        let (client, key) = view.read_with(cx, |view, _| {
            let attachment = view
                .language_server_link()
                .attachment()
                .expect("an attached document");
            (attachment.client().clone(), attachment.key().clone())
        });
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(state) = client.state(&key) {
                return state;
            }
            assert!(
                Instant::now() < deadline,
                "the server for {key} never started"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn allow(project: &Project) {
        TrustStore::load(&project.trust)
            .allow("rust-analyzer", &project.root)
            .expect("store the answer");
    }

    #[gpui::test]
    fn with_semantic_tokens_off_a_rust_file_starts_no_language_server(cx: &mut TestAppContext) {
        let project = project();
        allow(&project);
        install(cx, &config(false, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(is_off(&view, cx));
        assert!(!running_client(cx));
    }

    #[gpui::test]
    fn a_file_outside_the_files_dock_root_involves_no_server(cx: &mut TestAppContext) {
        let project = project();
        allow(&project);
        install(cx, &config(true, &probe_command()), &project.trust);
        let outside = project.root.with_file_name("outside.rs");
        std::fs::write(&outside, "fn outside() {}\n").expect("outside file");
        let (view, cx) = open_first(cx, outside, &project.root);
        assert!(is_off(&view, cx));
        assert!(!running_client(cx));
    }

    #[gpui::test]
    fn an_unresolved_command_leaves_the_server_missing_and_starts_no_process(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        allow(&project);
        install(
            cx,
            &config(true, "paneflow-no-such-language-server"),
            &project.trust,
        );
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(is_off(&view, cx));
        let client = cx.update(|_window, cx| {
            cx.try_global::<LanguageServerHub>()
                .and_then(LanguageServerHub::running_client)
                .expect("the client records the missing server")
        });
        let key = paneflow_lsp::ServerKey {
            server_id: "rust-analyzer".to_string(),
            root: project.root.clone(),
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        while client.state(&key).is_none() {
            assert!(Instant::now() < deadline, "the missing state never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            client.state(&key),
            Some(ServerState::Missing {
                command: "paneflow-no-such-language-server".to_string()
            })
        );
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_missing_server_installed_before_a_restart_starts_with_the_next_file(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        allow(&project);
        let installed = project
            .root
            .with_file_name("bin")
            .join("fake-language-server");
        let command = installed.to_string_lossy().into_owned();
        install(cx, &config(true, &command), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(is_off(&view, cx));

        std::fs::create_dir_all(installed.parent().expect("bin dir")).expect("bin dir");
        std::os::unix::fs::symlink(std::env::current_exe().expect("test binary"), &installed)
            .expect("install the server");
        let restarted = config(true, &command);
        let trust = project.trust.clone();
        cx.update(move |_window, cx| {
            crate::config_snapshot::publish(&restarted, cx);
            cx.set_global(LanguageServerHub::with_trust_path(Some(trust)));
        });
        let next = open_another(cx, project.root.join("src/lib.rs"), &project.root);
        assert!(is_attached(&next, cx));
    }

    #[gpui::test]
    fn a_new_root_asks_first_and_start_stores_the_answer_then_starts_the_server(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        let pair = view
            .read_with(cx, |view, _| view.language_server_link().consent().cloned())
            .expect("a consent banner");
        assert_eq!(pair.server_id, "rust-analyzer");
        assert_eq!(pair.root, project.root);
        let banner = consent_text(&pair.server_id, &pair.root);
        assert!(banner.contains("rust-analyzer"));
        assert!(banner.contains(&project.root.display().to_string()));
        assert!(banner.contains("build scripts"));
        assert!(!running_client(cx), "no server runs before the answer");
        let other = open_another(cx, project.root.join("src/lib.rs"), &project.root);
        assert!(other.read_with(cx, |view, _| {
            view.language_server_link().consent().is_some()
        }));

        view.update(cx, |view, cx| view.start_language_server(cx));
        cx.run_until_parked();

        assert!(TrustStore::load(&project.trust).is_allowed("rust-analyzer", &project.root));
        assert!(is_attached(&view, cx));
        assert!(
            is_attached(&other, cx),
            "every file of that root starts with the answer"
        );
        wait_for_server_state(&view, cx);
    }

    #[gpui::test]
    fn without_a_paneflow_home_start_lasts_for_the_session_until_permissions_are_forgotten(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        let config = config(true, &probe_command());
        cx.update(move |cx| {
            crate::config_snapshot::publish(&config, cx);
            cx.set_global(LanguageServerHub::with_trust_path(None));
        });
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(view.read_with(cx, |view, _| {
            view.language_server_link().consent().is_some()
        }));
        view.update(cx, |view, cx| view.start_language_server(cx));
        cx.run_until_parked();
        assert!(is_attached(&view, cx));

        let next = open_another(cx, project.root.join("src/lib.rs"), &project.root);
        assert!(
            is_attached(&next, cx),
            "a file of an accepted root opened later in the session starts with no banner"
        );

        cx.update(|_window, cx| super::language_server::hub(cx).forget_accepted());
        let after_forget = open_another(cx, project.root.join("src/main.rs"), &project.root);
        assert!(
            after_forget.read_with(cx, |view, _| {
                view.language_server_link().consent().is_some()
            }),
            "forgetting the permissions asks again"
        );
    }

    #[gpui::test]
    fn in_a_narrow_dock_the_consent_banner_wraps_and_its_start_button_is_clickable(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        let width = px(280.);
        cx.simulate_resize(size(width, px(500.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let text = cx
            .debug_bounds(CONSENT_TEXT)
            .expect("the banner text is painted");
        let buttons = cx
            .debug_bounds(CONSENT_BUTTONS)
            .expect("the banner buttons are painted");
        assert!(
            buttons.right() <= width,
            "the buttons end at {:?}, past the {width:?} view",
            buttons.right()
        );
        assert!(text.right() <= buttons.left());
        assert!(
            text.size.height > buttons.size.height,
            "the long root wraps onto several lines"
        );

        let start = cx
            .debug_bounds("code-language-server-start")
            .expect("the Start button is painted");
        cx.simulate_click(start.center(), Modifiers::default());
        cx.run_until_parked();

        assert!(TrustStore::load(&project.trust).is_allowed("rust-analyzer", &project.root));
        assert!(is_attached(&view, cx));
    }

    #[gpui::test]
    fn a_stored_answer_starts_the_server_after_a_restart_with_no_banner(cx: &mut TestAppContext) {
        let project = project();
        allow(&project);
        install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(is_attached(&view, cx));
        wait_for_server_state(&view, cx);
    }

    #[gpui::test]
    fn not_now_starts_nothing_and_asks_no_more_for_that_root_this_session(cx: &mut TestAppContext) {
        let project = project();
        install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(view.read_with(cx, |view, _| {
            view.language_server_link().consent().is_some()
        }));
        view.update(cx, |view, cx| view.decline_language_server(cx));
        cx.run_until_parked();
        assert!(is_off(&view, cx));
        let other = open_another(cx, project.root.join("src/lib.rs"), &project.root);
        assert!(
            is_off(&other, cx),
            "no banner for another file of the declined root"
        );
        assert!(!running_client(cx));
        assert!(!project.trust.exists());
    }

    #[cfg(unix)]
    #[gpui::test]
    fn a_symlinked_path_to_an_allowed_root_matches_the_stored_canonical_root(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        allow(&project);
        let link = project.root.with_file_name("linked");
        std::os::unix::fs::symlink(&project.root, &link).expect("symlink");
        install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, link.join("src/main.rs"), &link);
        assert!(is_attached(&view, cx));
    }

    #[gpui::test]
    fn an_unparseable_trust_file_asks_again_and_is_left_unchanged(cx: &mut TestAppContext) {
        let project = project();
        std::fs::write(&project.trust, b"{ broken").expect("corrupt trust file");
        install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(view.read_with(cx, |view, _| {
            view.language_server_link().consent().is_some()
        }));
        assert_eq!(
            std::fs::read(&project.trust).expect("trust file"),
            b"{ broken"
        );
    }

    #[gpui::test]
    fn edits_reach_the_server_as_byte_changes_and_a_disk_reload_as_the_full_text(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        allow(&project);
        let sent = install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(is_attached(&view, cx));

        view.update_in(cx, |view, window, cx| {
            view.replace_text_in_range(Some(0..0), "// a\n", window, cx);
        });
        view.update(cx, |view, cx| view.adopt_disk_text("fn disk() {}\n", cx));
        cx.run_until_parked();

        let sent = sent.lock().expect("sent log").clone();
        assert_eq!(
            sent,
            vec![
                Sent::Change(
                    vec![TextChange {
                        start_byte: 0,
                        old_end_byte: 0,
                        text: "// a\n".to_string()
                    }],
                    "// a\nfn main() {}\n".to_string()
                ),
                Sent::Replace("fn disk() {}\n".to_string()),
            ]
        );
    }

    #[gpui::test]
    fn forgetting_the_permissions_keeps_running_servers_until_their_last_tab_closes(
        cx: &mut TestAppContext,
    ) {
        let project = project();
        allow(&project);
        let sent = install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        assert!(is_attached(&view, cx));
        wait_for_server_state(&view, cx);

        super::super::super::language_server::forget_consents_blocking(Some(&project.trust))
            .expect("forget");
        cx.run_until_parked();

        assert!(!TrustStore::load(&project.trust).is_allowed("rust-analyzer", &project.root));
        assert!(is_attached(&view, cx), "the open document keeps its server");
        assert!(
            sent.lock().expect("sent log").is_empty(),
            "nothing was closed"
        );
    }

    #[gpui::test]
    fn closing_a_tab_or_turning_semantic_tokens_off_closes_its_document(cx: &mut TestAppContext) {
        let project = project();
        allow(&project);
        let sent = install(cx, &config(true, &probe_command()), &project.trust);
        let (view, cx) = open_first(cx, project.root.join("src/main.rs"), &project.root);
        let other = open_another(cx, project.root.join("src/lib.rs"), &project.root);
        assert!(is_attached(&other, cx));

        drop(other);
        cx.update(|_window, _cx| {});
        cx.run_until_parked();
        assert_eq!(sent.lock().expect("sent log").as_slice(), [Sent::Close]);

        let off = config(false, &probe_command());
        cx.update(|_window, cx| crate::config_snapshot::publish(&off, cx));
        view.update(cx, |view, cx| view.refresh_language_server(cx));
        cx.run_until_parked();
        assert!(is_off(&view, cx));
        assert_eq!(
            sent.lock().expect("sent log").as_slice(),
            [Sent::Close, Sent::Close]
        );
    }
}
