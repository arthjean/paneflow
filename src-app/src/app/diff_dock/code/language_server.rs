use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, Global, WeakEntity};
use paneflow_config::schema::{LanguageServerDefinition, PaneFlowConfig};
use paneflow_lsp::resolve::resolve_command;
use paneflow_lsp::trust::{TrustStore, canonical_root};
use paneflow_lsp::{
    ClientEvent, DocumentId, LanguageClient, OpenDocument, ServerKey, ServerSpec, TextChange,
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
    #[cfg(test)]
    key: ServerKey,
    #[cfg(test)]
    sent: Option<SentLog>,
}

impl Attachment {
    pub(crate) fn change(&self, changes: Vec<TextChange>, text: Rope) {
        #[cfg(test)]
        self.record(Sent::Change(changes.clone(), text.to_string()));
        self.client.change_document(self.document, changes, text);
    }

    pub(crate) fn replace(&self, text: Rope) {
        #[cfg(test)]
        self.record(Sent::Replace(text.to_string()));
        self.client.replace_document(self.document, text);
    }

    #[cfg(test)]
    pub(crate) fn key(&self) -> &ServerKey {
        &self.key
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
    Attached(Attachment),
}

impl LanguageServerLink {
    pub(crate) fn attachment(&self) -> Option<&Attachment> {
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
    waiting: Vec<(ConsentPair, WeakEntity<CodeView>)>,
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
            waiting: Vec::new(),
            #[cfg(test)]
            sent: None,
        }
    }

    pub(crate) fn trust_path(&self) -> Option<PathBuf> {
        self.trust_path.clone()
    }

    pub(crate) fn client(&mut self) -> Option<LanguageClient> {
        if self.client.is_none() {
            match LanguageClient::start(Arc::new(log_client_event)) {
                Ok(client) => self.client = Some(client),
                Err(error) => log::warn!("language servers: the client could not start: {error}"),
            }
        }
        self.client.clone()
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

fn log_client_event(event: ClientEvent) {
    if let ClientEvent::State { key, state } = event {
        log::debug!("language servers: {key} is now {state:?}");
    }
}

pub(crate) fn hub(cx: &mut App) -> &mut LanguageServerHub {
    cx.default_global::<LanguageServerHub>()
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
    hub: &mut LanguageServerHub,
    plan: AttachPlan,
    text: Rope,
) -> Option<Attachment> {
    let client = hub.client()?;
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
        #[cfg(test)]
        key.clone(),
        #[cfg(not(test))]
        key,
        spec,
        OpenDocument {
            path: plan.file,
            language_id: plan.definition.language_id,
            text,
        },
    );
    Some(Attachment {
        client,
        document,
        #[cfg(test)]
        key,
        #[cfg(test)]
        sent: hub.sent.clone(),
    })
}
