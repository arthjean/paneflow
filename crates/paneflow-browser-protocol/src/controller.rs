use std::collections::BTreeMap;

use crate::domain::validate_url;
use crate::frames::FrameLedger;
use crate::{
    zoom_percent_is_valid, Availability, BrowserError, BrowserId, BrowserPresentation,
    BrowserSession, Command, Document, Envelope, Event, OperationId, Owner, ProfileId, Reply,
    SessionState, CONTRACT_VERSION, MAX_BROWSERS_PER_SESSION, MAX_BROWSERS_TOTAL,
    MAX_LIVE_BROWSERS, MAX_TITLE_CHARS,
};

pub struct Controller {
    target: String,
    availability: Availability,
    sessions: BTreeMap<BrowserId, BrowserSession>,
    frames: BTreeMap<BrowserId, FrameLedger>,
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
        if session.state == SessionState::Dormant {
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
        if self.sessions.len() >= MAX_BROWSERS_TOTAL
            || self
                .sessions
                .values()
                .filter(|session| &session.document.owner == scope)
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
            Command::State { document } => Ok(Event::State {
                session: self.session(scope, &document)?.clone(),
            }),
            Command::Start { document } => {
                let session = self.session(scope, &document)?;
                if session.state != SessionState::Dormant {
                    return Err(BrowserError::Busy);
                }
                if self
                    .sessions
                    .values()
                    .filter(|session| session.state != SessionState::Dormant)
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
            Command::Navigate { document, url } => {
                let mut session = self.session(scope, &document)?.clone();
                validate_url(&url)?;
                if session.state == SessionState::Dormant {
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
                Ok(Event::State { session })
            }
            Command::Present {
                document,
                presentation,
            } => {
                let mut session = self.session(scope, &document)?.clone();
                if session.state == SessionState::Dormant {
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
                if session.state == SessionState::Dormant {
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
                if session.state == SessionState::Dormant {
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
            | Command::Mute { document, .. } => {
                self.live_session(scope, &document)?;
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
