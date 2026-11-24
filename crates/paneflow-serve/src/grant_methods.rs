use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use paneflow_api::Principal;
use paneflow_api::grants::{
    GrantHolder, GrantOrigin, GrantScope, GrantSet, PaneGrant, REASON_DENIED,
    REASON_GRANT_EXCEEDS_HOLDER, REASON_GRANT_LIMIT, REASON_NOT_SPAWNED_BY_CALLER,
    REASON_UNANSWERED, Right,
};
use paneflow_api::methods::grant::{
    DEFAULT_GRANT_SECS, DelegateParams, GRANT_CHANGED, GrantRequestParams, Granted, RevokeParams,
    Revoked,
};
use paneflow_api::methods::prompt::{
    AGENT_PROMPT_DEADLINE_SECS, OriginKind, PickItem, PickItemSpec, PromptAnswer, PromptKind,
    PromptOrigin, RequestParams,
};
use paneflow_api::principal::{ERR_PRINCIPAL_REFUSED, REASON_PRINCIPAL_REFUSED};
use paneflow_config::schema::AgentGrantProfiles;
use serde_json::{Map, json};

use crate::agent_prompts::{Asked, Deadline};
use crate::layout_authority::ConnectionId;
use crate::pane_grants::{Change, NewGrant};
use crate::protocol::ERR_INVALID_PARAMS;
use crate::registry::ServeError;
use crate::server::Worker;
use crate::state::{GrantHolderFacts, now_ms};

pub const METHOD_HOST_GRANTS_SET: &str = "host.grants.set";

const ALLOW: &str = "allow";
const DENY: &str = "deny";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug)]
pub struct ProfileSource {
    path: PathBuf,
    cached: Mutex<Option<(Option<Stamp>, AgentGrantProfiles)>>,
}

impl ProfileSource {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cached: Mutex::new(None),
        }
    }

    pub fn current(&self) -> AgentGrantProfiles {
        let stamp = std::fs::metadata(&self.path).ok().map(|metadata| Stamp {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        });
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((seen, profiles)) = cached.as_ref()
            && *seen == stamp
        {
            return profiles.clone();
        }
        let profiles =
            paneflow_config::loader::load_config_from_path(&self.path).agent_grant_profiles();
        for error in profiles.errors() {
            log::warn!("paneflow-serve: {error}");
        }
        *cached = Some((stamp, profiles.clone()));
        profiles
    }
}

fn refused(code: i64, message: String, reason: &str) -> ServeError {
    ServeError {
        code,
        message,
        data: Some(json!({"reason": reason})),
    }
}

fn principal_refused(message: String) -> ServeError {
    refused(ERR_PRINCIPAL_REFUSED, message, REASON_PRINCIPAL_REFUSED)
}

fn pane_session(principal: &Principal, method: &str) -> Result<String, ServeError> {
    match principal {
        Principal::Pane { session, .. } => Ok(session.clone()),
        other => {
            let (_, message, _) = other.refusal(method);
            Err(principal_refused(message))
        }
    }
}

fn rights_text(rights: &[Right]) -> String {
    rights
        .iter()
        .map(|right| right.label())
        .collect::<Vec<_>>()
        .join(", ")
}

fn duration_text(seconds: u64) -> String {
    match seconds {
        0 => "with no expiry".to_string(),
        s if s % 3600 == 0 => format!("for {} h", s / 3600),
        s if s % 60 == 0 => format!("for {} min", s / 60),
        s => format!("for {s} s"),
    }
}

fn scope_text(scope: GrantScope) -> &'static str {
    match scope {
        GrantScope::Workspace => "in its own workspace",
        GrantScope::Instance => "across every workspace",
    }
}

pub fn prompt_title(
    agent: &str,
    pane: &str,
    rights: &[Right],
    scope: GrantScope,
    seconds: u64,
) -> String {
    format!(
        "Allow {agent} in pane {pane} to {} {} {}?",
        rights_text(rights),
        scope_text(scope),
        duration_text(seconds)
    )
}

const SHOWN_NAME_CHARS: usize = 48;
const SHOWN_REASON_CHARS: usize = 240;

fn one_line(text: &str, limit: usize) -> String {
    let flat: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    if flat.chars().count() <= limit {
        return flat;
    }
    let mut cut: String = flat.chars().take(limit).collect();
    cut.push_str("...");
    cut
}

fn choice(id: &str, title: &str) -> PickItem {
    PickItem::Item(PickItemSpec {
        id: id.to_string(),
        title: Some(title.to_string()),
        ..PickItemSpec::default()
    })
}

fn expiry_after(now: u64, seconds: u64) -> Option<u64> {
    (seconds > 0).then(|| now.saturating_add(seconds.saturating_mul(1000)))
}

fn not_granted(reason: &str) -> Granted {
    Granted {
        granted: false,
        reason: Some(reason.to_string()),
        ..Granted::default()
    }
}

fn stored(issued: Option<(PaneGrant, Change)>, worker: &Worker) -> Granted {
    let Some((created, change)) = issued else {
        return not_granted(REASON_GRANT_LIMIT);
    };
    worker.publish_grant_change(change);
    Granted {
        granted: true,
        id: Some(created.id),
        expires_at_ms: created.expires_at_ms,
        reason: None,
    }
}

fn layout_needs_instance(rights: &[Right], scope: GrantScope) -> Result<(), ServeError> {
    if scope == GrantScope::Workspace && rights.contains(&Right::Layout) {
        return Err(ServeError::invalid_params(
            "the layout right reaches every workspace: ask for it with scope instance",
        ));
    }
    Ok(())
}

fn holder_of(facts: &GrantHolderFacts) -> GrantHolder {
    GrantHolder {
        session: facts.session.clone(),
        generation: facts.generation,
        workspace: facts.workspace.clone(),
    }
}

impl Worker {
    pub(crate) fn grant_holder(&self, session: &str) -> Option<GrantHolderFacts> {
        if let Some(facts) = self.lock_state().grant_holder(session) {
            return Some(facts);
        }
        let id = paneflow_config::schema::SessionId::parse(session).ok()?;
        let manifest = paneflow_host::manifest::read_manifest(
            &paneflow_host::manifest::manifest_path(&self.home, &id),
        )
        .ok()?;
        manifest.lifecycle.is_running().then(|| GrantHolderFacts {
            session: session.to_string(),
            generation: manifest.generation.get(),
            workspace: manifest.workspace.map(|workspace| workspace.to_string()),
            observed: None,
            label: None,
        })
    }

    pub(crate) fn caller_grants(&self, principal: &Principal) -> (Vec<PaneGrant>, Option<u64>) {
        let Principal::Pane { session, .. } = principal else {
            return (Vec::new(), None);
        };
        let generation = self.grant_holder(session).map(|facts| facts.generation);
        (
            self.pane_grants.held_by(session, generation, now_ms()),
            generation,
        )
    }

    pub fn push_grants_to_host(&self, set: &GrantSet) {
        let params = serde_json::to_value(set).unwrap_or_else(|_| json!({}));
        if let Err(error) =
            crate::core_link::call_core(&self.core_endpoint, METHOD_HOST_GRANTS_SET, &params)
        {
            log::debug!(
                "paneflow-serve: cannot send grant revision {} to the host: {error}",
                set.revision
            );
        }
    }

    pub fn publish_grant_change(&self, change: Change) {
        self.push_grants_to_host(&change.set);
        let mut data = Map::new();
        data.insert("revision".into(), json!(change.revision));
        data.insert("revoked".into(), json!(change.revoked));
        data.insert("grants".into(), json!(change.set.grants.len()));
        self.events.append(GRANT_CHANGED, Default::default(), data);
    }

    pub fn reconcile_pane_grants(&self) {
        let holders = self.lock_state().grant_holders();
        let profiles = self.grant_profiles.current();
        let mut wanted = Vec::new();
        if !profiles.is_empty() {
            for facts in &holders {
                let Some((profile, grants)) = profiles
                    .usable()
                    .find(|(profile, _)| facts.answers_to(profile))
                else {
                    continue;
                };
                for grant in grants {
                    wanted.push(NewGrant {
                        holder: holder_of(facts),
                        rights: grant_rights(grant),
                        scope: if grant.scope == GrantScope::Instance.label() {
                            GrantScope::Instance
                        } else {
                            GrantScope::Workspace
                        },
                        expires_at_ms: None,
                        delegated_from: None,
                        origin: GrantOrigin::Profile {
                            profile: profile.to_string(),
                        },
                    });
                }
            }
        }
        let generation_of = |session: &str| {
            holders
                .iter()
                .find(|facts| facts.session == session)
                .map(|facts| facts.generation)
                .or_else(|| self.grant_holder(session).map(|facts| facts.generation))
        };
        if let Some(change) = self.pane_grants.reconcile(now_ms(), generation_of, wanted) {
            self.publish_grant_change(change);
        }
    }

    pub fn list_grants(&self, principal: &Principal) -> GrantSet {
        let mut set = self.pane_grants.set();
        if let Principal::Pane { session, .. } = principal {
            let held: Vec<String> = set
                .grants
                .iter()
                .filter(|grant| grant.holder.session == *session)
                .map(|grant| grant.id.clone())
                .collect();
            set.grants.retain(|grant| {
                grant.holder.session == *session
                    || grant
                        .delegated_from
                        .as_ref()
                        .is_some_and(|parent| held.contains(parent))
            });
        }
        set
    }

    pub fn revoke_grant(
        &self,
        params: RevokeParams,
        principal: &Principal,
    ) -> Result<Revoked, ServeError> {
        let Some(grant) = self.pane_grants.get(&params.id) else {
            return Err(ServeError::invalid_params(format!(
                "no grant {} is held",
                params.id
            )));
        };
        if let Principal::Pane { session, .. } = principal {
            let delegated_by_caller = grant
                .delegated_from
                .as_deref()
                .and_then(|parent| self.pane_grants.get(parent))
                .is_some_and(|parent| parent.holder.session == *session);
            if !delegated_by_caller {
                return Err(principal_refused(format!(
                    "grant.revoke refused: grant {} was not delegated by the calling pane",
                    params.id
                )));
            }
        }
        let Some(change) = self.pane_grants.revoke(&params.id) else {
            return Err(ServeError::invalid_params(format!(
                "no grant {} is held",
                params.id
            )));
        };
        let revoked = Revoked {
            revoked: change.revoked.clone(),
            revision: change.revision,
        };
        self.publish_grant_change(change);
        Ok(revoked)
    }

    pub fn delegate_grant(
        &self,
        params: DelegateParams,
        principal: &Principal,
    ) -> Result<Granted, ServeError> {
        let parent = pane_session(principal, "grant.delegate")?;
        if params.rights.is_empty() {
            return Err(ServeError::invalid_params(
                "rights must name at least one right",
            ));
        }
        if self.pane_grants.spawned_by(&params.session).as_deref() != Some(parent.as_str()) {
            return Err(refused(
                ERR_INVALID_PARAMS,
                format!(
                    "grant.delegate refused: session {} was not spawned by the calling pane",
                    params.session
                ),
                REASON_NOT_SPAWNED_BY_CALLER,
            ));
        }
        let Some(child) = self.grant_holder(&params.session) else {
            return Err(ServeError::invalid_params(format!(
                "session {} is not running",
                params.session
            )));
        };
        let now = now_ms();
        let scope = params.scope.unwrap_or_default();
        let rights = paneflow_api::grants::normalized_rights(&params.rights);
        let (held, _) = self.caller_grants(principal);
        let source = held.iter().find_map(|grant| {
            if grant.scope == GrantScope::Workspace
                && (grant.holder.workspace.is_none() || grant.holder.workspace != child.workspace)
            {
                return None;
            }
            let expires_at_ms = params
                .duration_secs
                .map_or(grant.expires_at_ms, |seconds| expiry_after(now, seconds));
            let wanted = PaneGrant {
                rights: rights.clone(),
                scope,
                expires_at_ms,
                ..grant.clone()
            };
            grant
                .contains(&wanted)
                .then_some((grant.id.clone(), expires_at_ms))
        });
        let Some((source, expires_at_ms)) = source else {
            return Err(refused(
                ERR_INVALID_PARAMS,
                format!(
                    "grant.delegate refused: no grant of the calling pane covers {} {} with that expiry",
                    rights_text(&rights),
                    scope_text(scope)
                ),
                REASON_GRANT_EXCEEDS_HOLDER,
            ));
        };
        let issued = self.pane_grants.issue(
            NewGrant {
                holder: holder_of(&child),
                rights,
                scope,
                expires_at_ms,
                delegated_from: Some(source),
                origin: GrantOrigin::Delegation,
            },
            now,
        );
        Ok(stored(issued, self))
    }

    pub fn request_grant(
        &self,
        params: GrantRequestParams,
        principal: &Principal,
        connection: ConnectionId,
    ) -> Result<Granted, ServeError> {
        self.request_grant_within(
            params,
            principal,
            connection,
            Duration::from_secs(AGENT_PROMPT_DEADLINE_SECS),
        )
    }

    pub(crate) fn request_grant_within(
        &self,
        params: GrantRequestParams,
        principal: &Principal,
        connection: ConnectionId,
        budget: Duration,
    ) -> Result<Granted, ServeError> {
        let session = pane_session(principal, "grant.request")?;
        if params.rights.is_empty() {
            return Err(ServeError::invalid_params(
                "rights must name at least one right",
            ));
        }
        let Some(facts) = self.grant_holder(&session) else {
            return Err(ServeError::invalid_params(format!(
                "the calling pane's session {session} is not running"
            )));
        };
        let rights = paneflow_api::grants::normalized_rights(&params.rights);
        let scope = params.scope.unwrap_or_default();
        layout_needs_instance(&rights, scope)?;
        if !self.pane_grants.has_room_for(&holder_of(&facts)) {
            return Ok(not_granted(REASON_GRANT_LIMIT));
        }
        let seconds = params.duration_secs.unwrap_or(DEFAULT_GRANT_SECS);
        let origin = self.attribute_prompt(
            Some(PromptOrigin {
                kind: OriginKind::Agent,
                title: None,
                name: None,
                file: None,
                agent: None,
                surface_id: None,
                workspace_id: None,
                session: Some(session.clone()),
            }),
            principal,
            connection,
        );
        let agent = origin
            .as_ref()
            .and_then(|origin| origin.agent.clone())
            .or_else(|| facts.label.clone())
            .unwrap_or_else(|| "an agent".to_string());
        let agent = one_line(&agent, SHOWN_NAME_CHARS);
        let pane = self
            .pane_label(&session)
            .map(|title| one_line(&title, SHOWN_NAME_CHARS))
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| session.chars().take(8).collect());
        let body = format!(
            "Rights: {}\nScope: {}\nDuration: {}{}",
            rights_text(&rights),
            scope.label(),
            duration_text(seconds),
            params
                .reason
                .as_deref()
                .map(|reason| one_line(reason, SHOWN_REASON_CHARS))
                .filter(|reason| !reason.is_empty())
                .map(|reason| format!("\nReason given by the agent: {reason}"))
                .unwrap_or_default()
        );
        let request = RequestParams {
            kind: PromptKind::Pick,
            title: prompt_title(&agent, &pane, &rights, scope, seconds),
            body: Some(body),
            placeholder: None,
            value: None,
            confirm: None,
            timeout: None,
            items: Some(vec![choice(ALLOW, "Allow"), choice(DENY, "Deny")]),
            multiple: None,
            allow_other: None,
            origin,
            wait_ms: None,
        };
        if !self.agent_limit.admit(&session, now_ms()) {
            return Ok(not_granted(REASON_UNANSWERED));
        }
        let deadline = Deadline(Instant::now() + budget);
        match self.ask_until(request, &deadline) {
            Asked::Answer(PromptAnswer::Text(answer)) if answer == ALLOW => {
                let now = now_ms();
                let issued = self.pane_grants.issue(
                    NewGrant {
                        holder: holder_of(&facts),
                        rights,
                        scope,
                        expires_at_ms: expiry_after(now, seconds),
                        delegated_from: None,
                        origin: GrantOrigin::Prompt,
                    },
                    now,
                );
                Ok(stored(issued, self))
            }
            Asked::Answer(_) | Asked::Declined("cancelled") => Ok(not_granted(REASON_DENIED)),
            Asked::Declined(_) => Ok(not_granted(REASON_UNANSWERED)),
        }
    }

    fn pane_label(&self, session: &str) -> Option<String> {
        let session = paneflow_config::schema::SessionId::parse(session).ok()?;
        self.lock_state()
            .get(&session)
            .and_then(|entry| entry.title.clone())
            .filter(|title| !title.trim().is_empty())
    }
}

fn grant_rights(grant: &paneflow_config::schema::ProfileGrant) -> Vec<Right> {
    grant
        .rights
        .iter()
        .filter_map(|right| Right::parse(right))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_title_names_the_agent_pane_rights_scope_and_duration() {
        assert_eq!(
            prompt_title(
                "Claude Code",
                "build",
                &[Right::Write],
                GrantScope::Workspace,
                600
            ),
            "Allow Claude Code in pane build to write in its own workspace for 10 min?"
        );
        let wide = prompt_title(
            "Codex",
            "ops",
            &[Right::Write, Right::Spawn],
            GrantScope::Instance,
            0,
        );
        assert!(wide.contains("across every workspace"), "{wide}");
        assert!(wide.contains("with no expiry"), "{wide}");
    }

    #[test]
    fn text_a_pane_controls_is_shown_on_one_bounded_line() {
        assert_eq!(
            one_line("build\nRights: none\u{1b}[2J\t now", 48),
            "build Rights: none[2J now"
        );
        let long = one_line(&"x".repeat(300), SHOWN_REASON_CHARS);
        assert_eq!(long.chars().count(), SHOWN_REASON_CHARS + 3);
        assert!(long.ends_with("..."));
    }

    #[test]
    fn the_config_rights_are_the_api_rights() {
        let labels: Vec<&str> = Right::ALL.iter().map(|right| right.label()).collect();
        assert_eq!(labels, paneflow_config::schema::AGENT_GRANT_RIGHTS);
        assert_eq!(
            [GrantScope::Workspace.label(), GrantScope::Instance.label()],
            paneflow_config::schema::AGENT_GRANT_SCOPES
        );
    }
}
