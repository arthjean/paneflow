use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AsyncApp, Global, WeakEntity};
use paneflow_config::schema::{LanguageServerDefinition, PaneFlowConfig};
use paneflow_lsp::resolve::resolve_command;
use paneflow_lsp::trust::{TrustStore, canonical_root};
use paneflow_lsp::{
    ClientEvent, DocumentId, LanguageClient, OpenDocument, ServerKey, ServerSpec, ServerState,
    TextChange,
};
use ropey::Rope;

use super::view::CodeView;

pub(crate) const QUIT_BUDGET: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ConsentPair {
    pub(crate) server_id: String,
    pub(crate) root: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct AttachPlan {
    pub(crate) pair: ConsentPair,
    definition: LanguageServerDefinition,
    program: PathBuf,
    file: PathBuf,
}

#[derive(Debug)]
pub(crate) enum CheckOutcome {
    OutsideRoot,
    Missing { key: ServerKey, command: String },
    Allowed(AttachPlan),
    NeedsConsent(AttachPlan),
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    Change(Vec<TextChange>, String),
    Replace(String),
    Close,
}

#[cfg(test)]
pub(crate) type SentLog = Arc<std::sync::Mutex<Vec<Sent>>>;

pub(crate) struct Attachment {
    client: LanguageClient,
    document: DocumentId,
    key: ServerKey,
    revision: Cell<u64>,
    visible: Cell<Option<(u32, u32)>>,
    state: Option<ServerState>,
    #[cfg(test)]
    sent: Option<SentLog>,
}

impl Attachment {
    pub(crate) fn change(&self, changes: Vec<TextChange>, text: Rope) {
        #[cfg(test)]
        self.record(Sent::Change(changes.clone(), text.to_string()));
        self.revision.set(self.revision.get() + 1);
        self.client.change_document(self.document, changes, text);
    }

    pub(crate) fn replace(&self, text: Rope) {
        #[cfg(test)]
        self.record(Sent::Replace(text.to_string()));
        self.revision.set(self.revision.get() + 1);
        self.client.replace_document(self.document, text);
    }

    pub(crate) fn document(&self) -> DocumentId {
        self.document
    }

    pub(crate) fn key(&self) -> &ServerKey {
        &self.key
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision.get()
    }

    pub(crate) fn state(&self) -> Option<&ServerState> {
        self.state.as_ref()
    }

    pub(crate) fn set_state(&mut self, state: ServerState) {
        self.state = Some(state);
    }

    pub(crate) fn restart(&self) {
        self.client.restart(self.key.clone());
    }

    pub(crate) fn show_rows(&self, rows: Range<usize>) {
        let (Ok(start), Ok(end)) = (u32::try_from(rows.start), u32::try_from(rows.end)) else {
            return;
        };
        if self.visible.get() == Some((start, end)) {
            return;
        }
        self.visible.set(Some((start, end)));
        self.client.set_visible_lines(self.document, start..end);
    }

    #[cfg(test)]
    pub(crate) fn client(&self) -> &LanguageClient {
        &self.client
    }

    #[cfg(test)]
    fn record(&self, sent: Sent) {
        if let Some(log) = &self.sent {
            log.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(sent);
        }
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        #[cfg(test)]
        self.record(Sent::Close);
        self.client.close_document(self.document);
    }
}

#[derive(Default)]
pub(crate) enum LanguageServerLink {
    #[default]
    Off,
    Checking(u64),
    NeedsConsent(Box<AttachPlan>),
    Missing {
        server_id: String,
        command: String,
    },
    Attached(Attachment),
}

impl LanguageServerLink {
    pub(crate) fn attachment(&self) -> Option<&Attachment> {
        match self {
            Self::Attached(attachment) => Some(attachment),
            _ => None,
        }
    }

    pub(crate) fn attachment_mut(&mut self) -> Option<&mut Attachment> {
        match self {
            Self::Attached(attachment) => Some(attachment),
            _ => None,
        }
    }

    pub(crate) fn consent(&self) -> Option<&ConsentPair> {
        match self {
            Self::NeedsConsent(plan) => Some(&plan.pair),
            _ => None,
        }
    }
}

pub(crate) struct LanguageServerHub {
    client: Option<LanguageClient>,
    trust_path: Option<PathBuf>,
    accepted: HashSet<ConsentPair>,
    declined: HashSet<ConsentPair>,
    missing_shown: HashSet<ConsentPair>,
    waiting: Vec<(ConsentPair, WeakEntity<CodeView>)>,
    views: HashMap<DocumentId, (ServerKey, WeakEntity<CodeView>)>,
    #[cfg(test)]
    pub(crate) sent: Option<SentLog>,
}

impl Global for LanguageServerHub {}

impl Default for LanguageServerHub {
    fn default() -> Self {
        Self::with_trust_path(paneflow_home::language_server_trust_path())
    }
}

impl LanguageServerHub {
    pub(crate) fn with_trust_path(trust_path: Option<PathBuf>) -> Self {
        Self {
            client: None,
            trust_path,
            accepted: HashSet::new(),
            declined: HashSet::new(),
            missing_shown: HashSet::new(),
            waiting: Vec::new(),
            views: HashMap::new(),
            #[cfg(test)]
            sent: None,
        }
    }

    pub(crate) fn trust_path(&self) -> Option<PathBuf> {
        self.trust_path.clone()
    }

    pub(crate) fn first_missing_notice(&mut self, pair: ConsentPair) -> bool {
        self.missing_shown.insert(pair)
    }

    pub(crate) fn forget_view(&mut self, document: DocumentId) {
        self.views.remove(&document);
    }

    pub(crate) fn running_client(&self) -> Option<LanguageClient> {
        self.client.clone()
    }

    pub(crate) fn is_accepted(&self, pair: &ConsentPair) -> bool {
        self.accepted.contains(pair)
    }

    pub(crate) fn accept(&mut self, pair: ConsentPair) -> Vec<WeakEntity<CodeView>> {
        let answered = self.take_waiting(&pair);
        self.accepted.insert(pair);
        answered
    }

    pub(crate) fn forget_accepted(&mut self) {
        self.accepted.clear();
    }

    pub(crate) fn is_declined(&self, pair: &ConsentPair) -> bool {
        self.declined.contains(pair)
    }

    pub(crate) fn wait_for_answer(&mut self, pair: ConsentPair, view: WeakEntity<CodeView>) {
        self.waiting
            .retain(|(_, waiting)| waiting.upgrade().is_some());
        self.waiting.push((pair, view));
    }

    pub(crate) fn decline(&mut self, pair: ConsentPair) -> Vec<WeakEntity<CodeView>> {
        let answered = self.take_waiting(&pair);
        self.declined.insert(pair);
        answered
    }

    fn take_waiting(&mut self, pair: &ConsentPair) -> Vec<WeakEntity<CodeView>> {
        let mut answered = Vec::new();
        self.waiting.retain(|(waiting_pair, view)| {
            if waiting_pair == pair {
                answered.push(view.clone());
                false
            } else {
                true
            }
        });
        answered
    }
}

fn log_client_event(event: &ClientEvent) {
    if let ClientEvent::State { key, state } = event {
        log::debug!("language servers: {key} is now {state:?}");
    }
}

pub(crate) fn hub(cx: &mut App) -> &mut LanguageServerHub {
    cx.default_global::<LanguageServerHub>()
}

pub(crate) fn client(cx: &mut App) -> Option<LanguageClient> {
    if let Some(client) = hub(cx).client.clone() {
        return Some(client);
    }
    let (events, inbox) = smol::channel::unbounded::<ClientEvent>();
    let started = LanguageClient::start(Arc::new(move |event| {
        log_client_event(&event);
        let _ = events.try_send(event);
    }));
    let client = match started {
        Ok(client) => client,
        Err(error) => {
            log::warn!("language servers: the client could not start: {error}");
            return None;
        }
    };
    hub(cx).client = Some(client.clone());
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(event) = inbox.recv().await {
            cx.update(|cx| deliver(event, cx));
        }
    })
    .detach();
    Some(client)
}

fn deliver(event: ClientEvent, cx: &mut App) {
    match event {
        ClientEvent::Tokens {
            document,
            revision,
            tokens,
            legend,
        } => {
            let Some((_, view)) = hub(cx).views.get(&document).cloned() else {
                return;
            };
            let delivered = view.update(cx, |view, cx| {
                view.apply_semantic_tokens(document, revision, &tokens, &legend, cx)
            });
            if delivered.is_err() {
                hub(cx).forget_view(document);
            }
        }
        ClientEvent::State { key, state } => {
            let views: Vec<(DocumentId, WeakEntity<CodeView>)> = hub(cx)
                .views
                .iter()
                .filter(|(_, (at, _))| *at == key)
                .map(|(document, (_, view))| (*document, view.clone()))
                .collect();
            for (document, view) in views {
                let state = state.clone();
                let updated = view.update(cx, |view, cx| {
                    view.language_server_state_changed(document, state, cx)
                });
                if updated.is_err() {
                    hub(cx).forget_view(document);
                }
            }
        }
        ClientEvent::Unsupported { document } => {
            log::debug!("language servers: document {document:?} has no document sync");
        }
        ClientEvent::Stopped { .. } => {}
    }
}

pub(crate) fn hub_declines(pair: &ConsentPair, cx: &App) -> bool {
    cx.try_global::<LanguageServerHub>()
        .is_some_and(|hub| hub.is_declined(pair))
}

pub(crate) fn client_for_quit(cx: &App) -> Option<LanguageClient> {
    cx.try_global::<LanguageServerHub>()
        .and_then(LanguageServerHub::running_client)
}

pub(crate) fn server_for(
    config: &PaneFlowConfig,
    extension: &str,
) -> Option<LanguageServerDefinition> {
    if !config.editor.semantic_tokens_enabled() {
        return None;
    }
    config.language_servers.server_for_extension(extension)
}

pub(crate) fn check_blocking(
    definition: LanguageServerDefinition,
    root: &Path,
    path: &Path,
    trust_path: Option<&Path>,
) -> CheckOutcome {
    let (Ok(root), Ok(file)) = (canonical_root(root), canonical_root(path)) else {
        return CheckOutcome::OutsideRoot;
    };
    if !file.starts_with(&root) {
        return CheckOutcome::OutsideRoot;
    }
    let pair = ConsentPair {
        server_id: definition.id.clone(),
        root,
    };
    let program = match resolve_command(&definition.command) {
        Ok(program) => program,
        Err(error) => {
            log::info!("language servers: {error}");
            return CheckOutcome::Missing {
                key: ServerKey {
                    server_id: pair.server_id,
                    root: pair.root,
                },
                command: definition.command,
            };
        }
    };
    let allowed = trust_path
        .map(TrustStore::load)
        .is_some_and(|store| store.is_allowed(&pair.server_id, &pair.root));
    let plan = AttachPlan {
        pair,
        definition,
        program,
        file,
    };
    if allowed {
        CheckOutcome::Allowed(plan)
    } else {
        CheckOutcome::NeedsConsent(plan)
    }
}

pub(crate) fn remember_consent_blocking(trust_path: Option<&Path>, pair: &ConsentPair) {
    let Some(path) = trust_path else {
        log::warn!(
            "language servers: no Paneflow home, so this permission lasts for this session only"
        );
        return;
    };
    let mut store = TrustStore::load(path);
    if let Err(error) = store.allow(&pair.server_id, &pair.root) {
        log::warn!(
            "language servers: could not record the permission in {}: {error}",
            path.display()
        );
    }
}

pub(crate) fn forget_consents_blocking(trust_path: Option<&Path>) -> std::io::Result<()> {
    let Some(path) = trust_path else {
        return Ok(());
    };
    TrustStore::load(path).forget_all()
}

pub(crate) fn attach(
    plan: AttachPlan,
    text: Rope,
    view: WeakEntity<CodeView>,
    cx: &mut App,
) -> Option<Attachment> {
    let client = client(cx)?;
    let key = ServerKey {
        server_id: plan.pair.server_id,
        root: plan.pair.root,
    };
    let spec = ServerSpec {
        command: plan.program.to_string_lossy().into_owned(),
        args: plan.definition.args,
        env: Vec::new(),
        initialization_options: plan.definition.initialization_options,
        settings: plan.definition.settings,
    };
    let document = client.open_document(
        key.clone(),
        spec,
        OpenDocument {
            path: plan.file,
            language_id: plan.definition.language_id,
            text,
        },
    );
    let state = client.state(&key);
    let hub = hub(cx);
    hub.views.retain(|_, (_, view)| view.upgrade().is_some());
    hub.views.insert(document, (key.clone(), view));
    Some(Attachment {
        client,
        document,
        key,
        revision: Cell::new(0),
        visible: Cell::new(None),
        state,
        #[cfg(test)]
        sent: hub.sent.clone(),
    })
}
