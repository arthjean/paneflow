use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use paneflow_api::authorize::{
    refusal_data, scope_refusal_message, source_mismatch_message, unverified_message,
};
use paneflow_api::methods::config::InvokeContext;
use paneflow_api::methods::layout::{ERR_LAYOUT_REFUSED, NOT_ATTACHED};
use paneflow_api::principal::ERR_PRINCIPAL_REFUSED;
use paneflow_api::{Decision, Descriptor, Gates, Principal, Refusal};
use serde_json::Value;

use crate::protocol::{ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND};
use crate::registry::ServeError;
use crate::server::Worker;

const ACTING_PREFIX: &str = "acting:";
const GRANT_SEPARATOR: char = '|';

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Authority {
    pub(crate) principal: Principal,
    pub(crate) gates: Gates,
    pub(crate) switches: Gates,
}

impl Authority {
    pub(crate) const fn may_spawn(&self) -> bool {
        self.gates.orchestration
    }

    pub(crate) const fn switch_open_for_variables(&self) -> bool {
        self.switches.orchestration
    }
}

fn switches_for(policy: &paneflow_api::Policy, principal: &Principal, gates: Gates) -> Gates {
    match principal {
        Principal::Window | Principal::Worker => policy.gates_for(&Principal::External),
        _ => gates,
    }
}

pub(crate) fn refusal_error(method: &str, principal: &Principal, refusal: Refusal) -> ServeError {
    let data = Some(refusal_data(principal, refusal));
    match refusal {
        Refusal::PrincipalRefused
            if *principal == Principal::External
                && matches!(method, "layout.launch" | "layout.sweep") =>
        {
            ServeError {
                code: ERR_LAYOUT_REFUSED,
                message: format!("{method} needs a connection attached with layout.attach"),
                data: Some(serde_json::json!({
                    "reason": NOT_ATTACHED,
                    "principal": principal.kind(),
                })),
            }
        }
        Refusal::PrincipalRefused => {
            let (code, message, _) = principal.refusal(method);
            ServeError {
                code: if method.starts_with("layout.") {
                    ERR_LAYOUT_REFUSED
                } else {
                    code
                },
                message,
                data,
            }
        }
        Refusal::UnverifiedCaller => ServeError {
            code: ERR_PRINCIPAL_REFUSED,
            message: unverified_message(method, principal),
            data,
        },
        Refusal::GateClosed { refusal } => ServeError {
            code: ERR_METHOD_NOT_FOUND,
            message: refusal.to_string(),
            data,
        },
        Refusal::SourceMismatch => ServeError {
            code: ERR_INVALID_PARAMS,
            message: source_mismatch_message(method),
            data,
        },
        Refusal::ScopeRefused => ServeError {
            code: ERR_INVALID_PARAMS,
            message: scope_refusal_message(method, "?", None),
            data,
        },
    }
}

pub(crate) fn authorize_call(
    worker: &Worker,
    principal: &Principal,
    descriptor: &Descriptor,
    params: &mut Value,
) -> Result<Authority, ServeError> {
    let policy = worker.policy.current();
    match paneflow_api::authorize(principal, descriptor, params, &policy) {
        Decision::Refused { reason } => Err(refusal_error(descriptor.name(), principal, reason)),
        Decision::Allowed { scope, gates } => {
            if let Some(notice) = paneflow_api::policy::deprecation_notice(&policy, &gates) {
                log::warn!("paneflow-serve: {notice}");
            }
            scope.bind(principal, params);
            Ok(Authority {
                principal: principal.clone(),
                gates,
                switches: switches_for(&policy, principal, gates),
            })
        }
    }
}

pub(crate) fn acting_principal(invoker: &Principal, context: &InvokeContext) -> Principal {
    match invoker {
        Principal::Window | Principal::External
            if matches!(context.origin.as_deref(), Some("key" | "palette")) =>
        {
            Principal::Window
        }
        other => other.clone(),
    }
}

#[derive(Debug, Default)]
pub(crate) struct ActingRuns {
    runs: Mutex<HashMap<String, Principal>>,
}

pub(crate) struct ActingRun<'a> {
    runs: &'a ActingRuns,
    token: String,
}

impl Drop for ActingRun<'_> {
    fn drop(&mut self) {
        self.runs
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.token);
    }
}

fn fresh_token() -> String {
    paneflow_config::schema::SessionId::new().to_string()
}

impl ActingRuns {
    pub(crate) fn start(&self, principal: Principal, context: &mut InvokeContext) -> ActingRun<'_> {
        let token = fresh_token();
        self.runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token.clone(), principal);
        let window_grant = context.grant.take().unwrap_or_default();
        context.grant = Some(format!(
            "{ACTING_PREFIX}{token}{GRANT_SEPARATOR}{window_grant}"
        ));
        ActingRun { runs: self, token }
    }

    pub(crate) fn resolve<'g>(&self, grant: Option<&'g str>) -> (Principal, Option<&'g str>) {
        let Some(grant) = grant else {
            return (Principal::Worker, None);
        };
        let Some(rest) = grant.strip_prefix(ACTING_PREFIX) else {
            return (Principal::Worker, Some(grant));
        };
        let (token, window) = rest.split_once(GRANT_SEPARATOR).unwrap_or((rest, ""));
        let window = (!window.is_empty()).then_some(window);
        let principal = self
            .runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(token)
            .cloned()
            .unwrap_or_else(|| Principal::Unverified {
                reason: "the action that started this call has ended".into(),
            });
        (principal, window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Principal {
        Principal::Pane {
            session: "s-1".into(),
            workspace: Some("w-1".into()),
        }
    }

    fn context(origin: &str) -> InvokeContext {
        InvokeContext {
            origin: Some(origin.into()),
            ..InvokeContext::default()
        }
    }

    #[test]
    fn a_run_from_a_key_acts_as_the_window_and_one_from_a_pane_acts_as_that_pane() {
        assert_eq!(
            acting_principal(&Principal::External, &context("key")),
            Principal::Window
        );
        assert_eq!(
            acting_principal(&Principal::External, &context("palette")),
            Principal::Window
        );
        assert_eq!(
            acting_principal(&Principal::External, &context("cli")),
            Principal::External
        );
        assert_eq!(acting_principal(&pane(), &context("key")), pane());
    }

    #[test]
    fn a_run_token_names_its_principal_until_the_run_ends_and_keeps_the_window_grant() {
        let runs = ActingRuns::default();
        let mut ctx = InvokeContext {
            grant: Some("g-window".into()),
            ..InvokeContext::default()
        };
        let grant = {
            let _run = runs.start(pane(), &mut ctx);
            let grant = ctx.grant.clone().expect("the run carries a token");
            assert_eq!(runs.resolve(Some(&grant)), (pane(), Some("g-window")));
            grant
        };
        assert!(matches!(
            runs.resolve(Some(&grant)).0,
            Principal::Unverified { .. }
        ));
        assert_eq!(runs.resolve(None), (Principal::Worker, None));
        assert_eq!(
            runs.resolve(Some("g-window")),
            (Principal::Worker, Some("g-window"))
        );
    }
}
