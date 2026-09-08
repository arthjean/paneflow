use std::collections::BTreeMap;

use crate::domain::validate_url;
use crate::frames::FrameLedger;
use crate::{
    find_text_is_valid, zoom_percent_is_valid, Availability, BrowserError, BrowserId,
    BrowserPresentation, BrowserSession, Command, Document, Envelope, Event, OperationId, Owner,
    ProfileId, Reply, SessionState, CONTRACT_VERSION, MAX_BROWSERS_PER_SESSION, MAX_BROWSERS_TOTAL,
    MAX_LIVE_BROWSERS, MAX_TITLE_CHARS,
};

pub struct Controller {
    target: String,
    availability: Availability,
    sessions: BTreeMap<BrowserId, BrowserSession>,
    frames: BTreeMap<BrowserId, FrameLedger>,
    inspectors: BTreeMap<BrowserId, BrowserId>,
    pending: BTreeMap<OperationId, (Document, bool)>,
    next_generation: u64,
    next_operation: u64,
}

impl Controller {
    pub fn new(target: String, deterministic: bool) -> Self {
        Self {
            target,
            availability: if deterministic {
                Availability::Development
            } else {
                Availability::Absent
            },
            sessions: BTreeMap::new(),
            frames: BTreeMap::new(),
            inspectors: BTreeMap::new(),
            pending: BTreeMap::new(),
            next_generation: 1,
            next_operation: 1,
        }
    }

    pub fn dispatch(&mut self, scope: &Owner, message: Envelope) -> Reply {
        let result = if message.version != CONTRACT_VERSION {
            Err(BrowserError::IncompatibleVersion)
        } else {
            self.apply(scope, message.command)
        };
        Reply {
            version: CONTRACT_VERSION,
            operation: message.operation,
            result,
        }
    }

    pub fn renderer_crashed(
        &mut self,
        scope: &Owner,
        document: &Document,
    ) -> Result<Event, BrowserError> {
        let mut session = self.session(scope, document)?.clone();
        session.document.generation = self.generation()?;
        session.state = SessionState::Crashed;
        session.presentation = BrowserPresentation::unmounted();
        if let Some(frames) = self.frames.get_mut(&document.browser) {
            frames.invalidate();
        }
        self.pending
            .retain(|_, (target, _)| target.browser != document.browser);
        self.sessions
            .insert(document.browser.clone(), session.clone());
        Ok(Event::State { session })
    }

    fn owned_session(
        &self,
        scope: &Owner,
        document: &Document,
    ) -> Result<&BrowserSession, BrowserError> {
        if &document.owner != scope {
            return Err(BrowserError::AccessDenied);
        }
        let session = self
            .sessions
            .get(&document.browser)
            .ok_or(BrowserError::UnknownIdentity)?;
        if session.document.owner != document.owner {
            return Err(BrowserError::AccessDenied);
        }
        Ok(session)
    }

    fn session(&self, scope: &Owner, document: &Document) -> Result<&BrowserSession, BrowserError> {
        let session = self.owned_session(scope, document)?;
        if session.document.generation != document.generation {
            return Err(BrowserError::StaleGeneration);
        }
        Ok(session)
    }

    fn live_session(
        &self,
        scope: &Owner,
        document: &Document,
    ) -> Result<&BrowserSession, BrowserError> {
        let session = self.session(scope, document)?;
        if matches!(session.state, SessionState::Dormant | SessionState::Crashed) {
            return Err(BrowserError::Unavailable);
        }
        Ok(session)
    }

    fn generation(&mut self) -> Result<u64, BrowserError> {
        let generation = self.next_generation;
        self.next_generation = generation
            .checked_add(1)
            .ok_or(BrowserError::LimitReached)?;
        Ok(generation)
    }

    fn create(
        &mut self,
        scope: &Owner,
        browser: BrowserId,
        profile: ProfileId,
        url: String,
        title: String,
    ) -> Result<Event, BrowserError> {
        if self.availability == Availability::Absent {
            return Err(BrowserError::Unavailable);
        }
        validate_url(&url)?;
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(BrowserError::TooLarge);
        }
        if self.sessions.contains_key(&browser) {
            return Err(BrowserError::Busy);
        }
        if self
            .sessions
            .keys()
            .filter(|id| !self.inspectors.contains_key(*id))
            .count()
            >= MAX_BROWSERS_TOTAL
            || self
                .sessions
                .values()
                .filter(|session| {
                    &session.document.owner == scope
                        && !self.inspectors.contains_key(&session.document.browser)
                })
                .count()
                >= MAX_BROWSERS_PER_SESSION
        {
            return Err(BrowserError::LimitReached);
        }
        if self.sessions.values().any(|session| {
            (session.document.owner.workspace == scope.workspace) != (session.profile == profile)
        }) {
            return Err(BrowserError::AccessDenied);
        }
        let session = BrowserSession {
            document: Document {
                owner: scope.clone(),
                browser: browser.clone(),
                generation: self.generation()?,
            },
            profile,
            url,
            title,
            state: SessionState::Dormant,
            presentation: BrowserPresentation::unmounted(),
        };
        self.sessions.insert(browser.clone(), session.clone());
        self.frames.insert(browser, FrameLedger::default());
        Ok(Event::State { session })
    }

    fn apply(&mut self, scope: &Owner, command: Command) -> Result<Event, BrowserError> {
        let agent_navigation = matches!(&command, Command::AgentNavigate { .. });
        match command {
            Command::Capabilities => Ok(Event::Capabilities {
                target: self.target.clone(),
                availability: self.availability,
                contract_version: CONTRACT_VERSION,
                terminal_available: true,
            }),
            Command::Create {
                owner,
                browser,
                profile,
                url,
                title,
            } => {
                if &owner != scope {
                    return Err(BrowserError::AccessDenied);
                }
                self.create(scope, browser, profile, url, title)
            }
            Command::CreateDevTools { document, browser } => {
                let target = self.live_session(scope, &document)?.clone();
                if self.inspectors.contains_key(&document.browser)
                    || self
                        .inspectors
                        .values()
                        .any(|target| target == &document.browser)
                    || self.sessions.contains_key(&browser)
                {
                    return Err(BrowserError::Busy);
                }
                let session = BrowserSession {
                    document: Document {
                        owner: scope.clone(),
                        browser: browser.clone(),
                        generation: self.generation()?,
                    },
                    profile: target.profile,
                    url: target.url,
                    title: "Developer Tools".to_owned(),
                    state: SessionState::Dormant,
                    presentation: BrowserPresentation::unmounted(),
                };
                self.inspectors.insert(browser.clone(), document.browser);
                self.frames.insert(browser.clone(), FrameLedger::default());
                self.sessions.insert(browser, session.clone());
                Ok(Event::State { session })
            }
            Command::State { document } => Ok(Event::State {
                session: self.session(scope, &document)?.clone(),
            }),
            Command::Start { document } => {
                let session = self.session(scope, &document)?;
                if session.state != SessionState::Dormant {
                    return Err(BrowserError::Busy);
                }
                if !self.inspectors.contains_key(&document.browser)
                    && self
                        .sessions
                        .values()
                        .filter(|session| {
                            !matches!(session.state, SessionState::Dormant | SessionState::Crashed)
                                && !self.inspectors.contains_key(&session.document.browser)
                        })
                        .count()
                        >= MAX_LIVE_BROWSERS
                {
                    return Err(BrowserError::LimitReached);
                }
                let session = self
                    .sessions
                    .get_mut(&document.browser)
                    .ok_or(BrowserError::UnknownIdentity)?;
                session.state = SessionState::Hidden;
                Ok(Event::State {
                    session: session.clone(),
                })
            }
            Command::Sleep { document } => {
                let mut session = self.session(scope, &document)?.clone();
                if session.state != SessionState::Dormant {
                    session.state = SessionState::Dormant;
                    session.presentation = BrowserPresentation::unmounted();
                    self.frames
                        .get_mut(&document.browser)
                        .ok_or(BrowserError::UnknownIdentity)?
                        .invalidate();
                    self.pending
                        .retain(|_, (target, _)| target.browser != document.browser);
                    self.sessions.insert(document.browser, session.clone());
                }
                Ok(Event::State { session })
            }
            Command::Navigate { document, url } | Command::AgentNavigate { document, url } => {
                if self.inspectors.contains_key(&document.browser) {
                    return Err(BrowserError::AccessDenied);
                }
                let mut session = self.session(scope, &document)?.clone();
                validate_url(&url)?;
                if matches!(session.state, SessionState::Dormant | SessionState::Crashed) {
                    return Err(BrowserError::Unavailable);
                }
                session.document.generation = self.generation()?;
                session.url = url;
                session.presentation = BrowserPresentation::unmounted();
                session.state = SessionState::Hidden;
                self.frames
                    .get_mut(&document.browser)
                    .ok_or(BrowserError::UnknownIdentity)?
                    .invalidate();
                self.pending
                    .retain(|_, (target, _)| target.browser != document.browser);
                self.sessions.insert(document.browser, session.clone());
                if agent_navigation {
                    Ok(Event::NavigationStarted { session })
                } else {
                    Ok(Event::State { session })
                }
            }
            Command::Screenshot { document } => {
                let session = self.session(scope, &document)?;
                if matches!(session.state, SessionState::Dormant | SessionState::Crashed) {
                    return Err(BrowserError::Unavailable);
                }
                Ok(Event::ScreenshotAccepted)
            }
            Command::Present {
                document,
                presentation,
            } => {
                let mut session = self.session(scope, &document)?.clone();
                if matches!(session.state, SessionState::Dormant | SessionState::Crashed) {
                    return Err(BrowserError::Unavailable);
                }
                if !presentation.is_valid() {
                    return Err(BrowserError::InvalidFrame);
                }
                let changed = session.presentation.width != presentation.width
                    || session.presentation.height != presentation.height
                    || session.presentation.scale_percent != presentation.scale_percent;
                if changed || session.presentation.generation != presentation.generation {
                    if presentation.generation <= session.presentation.generation {
                        return Err(BrowserError::StaleGeneration);
                    }
                    self.frames
                        .get_mut(&document.browser)
                        .ok_or(BrowserError::UnknownIdentity)?
                        .resize(document.generation, presentation.generation)?;
                }
                session.state = if presentation.visible {
                    SessionState::Visible
                } else {
                    SessionState::Hidden
                };
                session.presentation = presentation;
                self.sessions.insert(document.browser, session.clone());
                Ok(Event::State { session })
            }
            Command::Close { document } => {
                self.session(scope, &document)?;
                if self
                    .frames
                    .get(&document.browser)
                    .is_some_and(FrameLedger::has_outstanding)
                {
                    return Err(BrowserError::Busy);
                }
                let children: Vec<_> = self
                    .inspectors
                    .iter()
                    .filter(|(_, target)| *target == &document.browser)
                    .map(|(id, _)| id.clone())
                    .collect();
                for child in children {
                    self.sessions.remove(&child);
                    self.frames.remove(&child);
                    self.inspectors.remove(&child);
                    self.pending
                        .retain(|_, (target, _)| target.browser != child);
                }
                self.inspectors.remove(&document.browser);
                self.sessions.remove(&document.browser);
                self.frames.remove(&document.browser);
                self.pending
                    .retain(|_, (target, _)| target.browser != document.browser);
                Ok(Event::Closed { document })
            }
            Command::BeginOperation {
                document,
                mutation,
                text,
            } => {
                let session = self.session(scope, &document)?;
                if matches!(session.state, SessionState::Dormant | SessionState::Crashed) {
                    return Err(BrowserError::Unavailable);
                }
                if text.len() > 64 * 1024 {
                    return Err(BrowserError::TooLarge);
                }
                if self.pending.len() >= 16
                    || self
                        .pending
                        .values()
                        .filter(|(target, _)| target.owner.workspace == document.owner.workspace)
                        .count()
                        >= 4
                    || (mutation
                        && self.pending.values().any(|(target, mutating)| {
                            target.browser == document.browser && *mutating
                        }))
                {
                    return Err(BrowserError::Busy);
                }
                let operation = OperationId::try_from(format!("operation-{}", self.next_operation))
                    .map_err(|_| BrowserError::LimitReached)?;
                self.next_operation = self
                    .next_operation
                    .checked_add(1)
                    .ok_or(BrowserError::LimitReached)?;
                self.pending.insert(operation.clone(), (document, mutation));
                Ok(Event::Accepted { operation })
            }
            Command::CompleteOperation { document, pending } => {
                self.session(scope, &document)?;
                if self.pending.get(&pending).map(|(target, _)| target) != Some(&document) {
                    return Err(BrowserError::UnknownIdentity);
                }
                self.pending.remove(&pending);
                Ok(Event::Completed { operation: pending })
            }
            Command::Frame {
                document,
                contract_version,
                pool_generation,
                buffer,
                sequence,
            } => {
                let session = self.session(scope, &document)?;
                if contract_version != CONTRACT_VERSION {
                    return Err(BrowserError::IncompatibleVersion);
                }
                if !session.presentation.visible {
                    return Err(BrowserError::Unavailable);
                }
                self.frames
                    .get_mut(&document.browser)
                    .ok_or(BrowserError::UnknownIdentity)?
                    .receive(document.generation, pool_generation, buffer, sequence)?;
                Ok(Event::FrameAccepted)
            }
            Command::Input { document, input } => {
                let session = self.session(scope, &document)?;
                if matches!(session.state, SessionState::Dormant | SessionState::Crashed) {
                    return Err(BrowserError::Unavailable);
                }
                if !input.is_valid() {
                    return Err(BrowserError::InvalidMessage);
                }
                Ok(Event::InputAccepted)
            }
            Command::History { document, .. }
            | Command::Reload { document, .. }
            | Command::Stop { document }
            | Command::StopFinding { document }
            | Command::Mute { document, .. } => {
                self.live_session(scope, &document)?;
                Ok(Event::NavigationAccepted)
            }
            Command::Find { document, text, .. } => {
                self.live_session(scope, &document)?;
                if !find_text_is_valid(&text) {
                    return Err(BrowserError::InvalidMessage);
                }
                Ok(Event::NavigationAccepted)
            }
            Command::Zoom { document, percent } => {
                self.live_session(scope, &document)?;
                if !zoom_percent_is_valid(percent) {
                    return Err(BrowserError::InvalidMessage);
                }
                Ok(Event::NavigationAccepted)
            }
            Command::ReleaseFrame {
                document,
                pool_generation,
                buffer,
                sequence,
            } => {
                self.owned_session(scope, &document)?;
                self.frames
                    .get_mut(&document.browser)
                    .ok_or(BrowserError::UnknownIdentity)?
                    .release(document.generation, pool_generation, buffer, sequence)?;
                Ok(Event::FrameReleased)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionId, WorkspaceId, MAX_FIND_TEXT_BYTES};

    fn state(event: Event) -> BrowserSession {
        match event {
            Event::State { session } => Some(session),
            _ => None,
        }
        .expect("expected session state")
    }

    fn dispatch(
        controller: &mut Controller,
        owner: &Owner,
        command: Command,
    ) -> Result<Event, BrowserError> {
        let envelope = Envelope {
            version: CONTRACT_VERSION,
            operation: OperationId::try_from("find-test".to_owned())
                .map_err(|_| BrowserError::InvalidMessage)?,
            command,
        };
        let encoded = crate::encode_message(&envelope).map_err(|_| BrowserError::InvalidMessage)?;
        let decoded =
            crate::read_message(&mut encoded.as_slice())?.ok_or(BrowserError::InvalidMessage)?;
        controller.dispatch(owner, decoded).result
    }

    #[test]
    fn find_dispatch_enforces_live_owned_generation_and_text_bounds() {
        let owner = Owner {
            workspace: WorkspaceId::try_from("workspace".to_owned()).unwrap(),
            session: SessionId::try_from("session".to_owned()).unwrap(),
        };
        let mut controller = Controller::new("test".to_owned(), true);
        let session = state(
            dispatch(
                &mut controller,
                &owner,
                Command::Create {
                    owner: owner.clone(),
                    browser: BrowserId::try_from("browser".to_owned()).unwrap(),
                    profile: ProfileId::try_from("profile".to_owned()).unwrap(),
                    url: crate::BLANK_URL.to_owned(),
                    title: String::new(),
                },
            )
            .unwrap(),
        );
        let document = session.document;
        let find = |document: Document, text: String| Command::Find {
            document,
            text,
            forward: true,
            find_next: false,
        };
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                find(document.clone(), "word".into())
            )
            .unwrap_err(),
            BrowserError::Unavailable
        );
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::StopFinding {
                    document: document.clone()
                }
            )
            .unwrap_err(),
            BrowserError::Unavailable
        );
        dispatch(
            &mut controller,
            &owner,
            Command::Start {
                document: document.clone(),
            },
        )
        .unwrap();
        for text in [String::new(), "é".repeat(MAX_FIND_TEXT_BYTES / 2)] {
            assert!(matches!(
                dispatch(&mut controller, &owner, find(document.clone(), text)),
                Ok(Event::NavigationAccepted)
            ));
        }
        for text in [
            "before\0after".to_owned(),
            "é".repeat(MAX_FIND_TEXT_BYTES / 2 + 1),
        ] {
            assert_eq!(
                dispatch(&mut controller, &owner, find(document.clone(), text)).unwrap_err(),
                BrowserError::InvalidMessage
            );
        }
        assert!(matches!(
            dispatch(
                &mut controller,
                &owner,
                Command::StopFinding {
                    document: document.clone()
                }
            ),
            Ok(Event::NavigationAccepted)
        ));
        let mut stale = document.clone();
        stale.generation += 1;
        let mut foreign = document.clone();
        foreign.owner.session = SessionId::try_from("other".to_owned()).unwrap();
        for (target, error) in [
            (stale, BrowserError::StaleGeneration),
            (foreign, BrowserError::AccessDenied),
        ] {
            assert_eq!(
                dispatch(&mut controller, &owner, find(target.clone(), "word".into())).unwrap_err(),
                error
            );
            assert_eq!(
                dispatch(
                    &mut controller,
                    &owner,
                    Command::StopFinding { document: target }
                )
                .unwrap_err(),
                error
            );
        }
        dispatch(
            &mut controller,
            &owner,
            Command::Close {
                document: document.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                find(document.clone(), "word".into())
            )
            .unwrap_err(),
            BrowserError::UnknownIdentity
        );
        assert_eq!(
            dispatch(&mut controller, &owner, Command::StopFinding { document }).unwrap_err(),
            BrowserError::UnknownIdentity
        );
    }
    #[test]
    fn devtools_dispatch_preserves_browser_quotas_and_target_lifetime() {
        let owner = Owner {
            workspace: WorkspaceId::try_from("workspace".to_owned()).unwrap(),
            session: SessionId::try_from("session".to_owned()).unwrap(),
        };
        let mut controller = Controller::new("test".to_owned(), true);
        let mut documents = Vec::new();
        for index in 0..MAX_BROWSERS_PER_SESSION {
            let session = state(
                dispatch(
                    &mut controller,
                    &owner,
                    Command::Create {
                        owner: owner.clone(),
                        browser: format!("browser{index}").try_into().unwrap(),
                        profile: "profile".to_owned().try_into().unwrap(),
                        url: crate::BLANK_URL.to_owned(),
                        title: String::new(),
                    },
                )
                .unwrap(),
            );
            let document = session.document;
            if index == 0 {
                assert_eq!(
                    dispatch(
                        &mut controller,
                        &owner,
                        Command::CreateDevTools {
                            document: document.clone(),
                            browser: "inspector".to_owned().try_into().unwrap()
                        }
                    )
                    .unwrap_err(),
                    BrowserError::Unavailable
                );
            }
            dispatch(
                &mut controller,
                &owner,
                Command::Start {
                    document: document.clone(),
                },
            )
            .unwrap();
            documents.push(document);
            if index == 0 {
                let inspector = state(
                    dispatch(
                        &mut controller,
                        &owner,
                        Command::CreateDevTools {
                            document: documents[0].clone(),
                            browser: "inspector".to_owned().try_into().unwrap(),
                        },
                    )
                    .unwrap(),
                );
                assert_eq!(inspector.profile, session.profile);
                assert_eq!(inspector.document.owner, owner);
                dispatch(
                    &mut controller,
                    &owner,
                    Command::Start {
                        document: inspector.document,
                    },
                )
                .unwrap();
            }
        }
        let target = documents.remove(0);
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::CreateDevTools {
                    document: target.clone(),
                    browser: "duplicate".to_owned().try_into().unwrap()
                }
            )
            .unwrap_err(),
            BrowserError::Busy
        );
        let inspector = controller
            .sessions
            .get(&"inspector".to_owned().try_into().unwrap())
            .unwrap()
            .document
            .clone();
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::Navigate {
                    document: inspector.clone(),
                    url: crate::BLANK_URL.to_owned()
                }
            )
            .unwrap_err(),
            BrowserError::AccessDenied
        );
        dispatch(
            &mut controller,
            &owner,
            Command::Close {
                document: target.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::State {
                    document: inspector
                }
            )
            .unwrap_err(),
            BrowserError::UnknownIdentity
        );
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::CreateDevTools {
                    document: target,
                    browser: "late".to_owned().try_into().unwrap()
                }
            )
            .unwrap_err(),
            BrowserError::UnknownIdentity
        );
    }
    #[test]
    fn renderer_crash_retires_generation_and_refuses_live_commands() {
        let owner = Owner {
            workspace: WorkspaceId::try_from("workspace".to_owned()).unwrap(),
            session: SessionId::try_from("session".to_owned()).unwrap(),
        };
        let mut controller = Controller::new("test".to_owned(), true);
        let session = state(
            dispatch(
                &mut controller,
                &owner,
                Command::Create {
                    owner: owner.clone(),
                    browser: "browser".to_owned().try_into().unwrap(),
                    profile: "profile".to_owned().try_into().unwrap(),
                    url: crate::BLANK_URL.to_owned(),
                    title: String::new(),
                },
            )
            .unwrap(),
        );
        let old = session.document;
        dispatch(
            &mut controller,
            &owner,
            Command::Start {
                document: old.clone(),
            },
        )
        .unwrap();
        let session = state(controller.renderer_crashed(&owner, &old).unwrap());
        assert_eq!(session.state, SessionState::Crashed);
        assert_ne!(session.document.generation, old.generation);
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::Reload {
                    document: old,
                    ignore_cache: false
                }
            )
            .unwrap_err(),
            BrowserError::StaleGeneration
        );
        assert_eq!(
            dispatch(
                &mut controller,
                &owner,
                Command::Reload {
                    document: session.document.clone(),
                    ignore_cache: false
                }
            )
            .unwrap_err(),
            BrowserError::Unavailable
        );
        dispatch(
            &mut controller,
            &owner,
            Command::Close {
                document: session.document,
            },
        )
        .unwrap();
    }
}
