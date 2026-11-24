use serde_json::Value;

use crate::descriptor::{Audience, Descriptor, Gate, ScopeRule};
use crate::grants::{GrantView, pane_gates};
use crate::policy::{Gates, Policy};
use crate::principal::Principal;

pub const REASON_UNVERIFIED_CALLER: &str = "unverified_caller";
pub const REASON_SCOPE_REFUSED: &str = "scope_refused";
pub const REASON_SOURCE_MISMATCH: &str = "source_mismatch";
pub const REASON_GATE_CLOSED: &str = "gate_closed";

pub const UNVERIFIED_ANSWERS: [&str; 3] =
    ["system.whoami", "system.capabilities", "system.describe"];

const SCOPE_SESSION: &str = "scope_session";
const SOURCE_SESSION: &str = "source_session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Instance,
    Claimed {
        session: String,
    },
    Pane {
        session: String,
        workspace: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    PrincipalRefused,
    UnverifiedCaller,
    ScopeRefused,
    SourceMismatch,
    GrantsUnsynced,
    GateClosed { refusal: &'static str },
}

impl Refusal {
    pub const fn reason(self) -> &'static str {
        match self {
            Self::PrincipalRefused => crate::principal::REASON_PRINCIPAL_REFUSED,
            Self::UnverifiedCaller => REASON_UNVERIFIED_CALLER,
            Self::ScopeRefused => REASON_SCOPE_REFUSED,
            Self::SourceMismatch => REASON_SOURCE_MISMATCH,
            Self::GrantsUnsynced => crate::grants::REASON_GRANTS_UNSYNCED,
            Self::GateClosed { .. } => REASON_GATE_CLOSED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed { scope: Scope, gates: Gates },
    Refused { reason: Refusal },
}

impl Decision {
    pub fn label(&self) -> String {
        match self {
            Self::Allowed { scope, .. } => format!(
                "allowed:{}",
                match scope {
                    Scope::Instance => "instance",
                    Scope::Claimed { .. } => "claimed",
                    Scope::Pane { .. } => "pane",
                }
            ),
            Self::Refused { reason } => format!("refused:{}", reason.reason()),
        }
    }
}

fn param_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn admits_audience(principal: &Principal, audience: Audience) -> bool {
    match audience {
        Audience::Anyone => true,
        Audience::NotPane => !principal.is_pane(),
        Audience::PaneOnly => principal.is_pane(),
        Audience::PaneflowOnly => matches!(principal, Principal::Window | Principal::Worker),
    }
}

fn pane_scope(rule: ScopeRule, params: &Value, gates: &Gates, own: Scope) -> Scope {
    match rule {
        ScopeRule::Unscoped => Scope::Instance,
        ScopeRule::ClaimedSession if params.get(SCOPE_SESSION).is_some() => own,
        ScopeRule::ClaimedSession => Scope::Instance,
        ScopeRule::Workspace if gates.reach => Scope::Instance,
        ScopeRule::Workspace => own,
        ScopeRule::WorkspaceUnlessAll
            if gates.reach && param_str(params, "scope") == Some("all") =>
        {
            Scope::Instance
        }
        ScopeRule::WorkspaceUnlessAll => own,
    }
}

pub fn authorize(
    principal: &Principal,
    descriptor: &Descriptor,
    params: &Value,
    policy: &Policy,
) -> Decision {
    decide(
        principal,
        descriptor,
        params,
        policy,
        &GrantView::NONE,
        true,
    )
}

pub fn authorize_with(
    principal: &Principal,
    descriptor: &Descriptor,
    params: &Value,
    policy: &Policy,
    grants: &GrantView<'_>,
) -> Decision {
    decide(principal, descriptor, params, policy, grants, true)
}

pub fn admit_with(
    principal: &Principal,
    descriptor: &Descriptor,
    params: &Value,
    policy: &Policy,
    grants: &GrantView<'_>,
) -> Decision {
    decide(principal, descriptor, params, policy, grants, false)
}

fn needs_unsynced_grant(descriptor: &Descriptor, params: &Value, gates: &Gates) -> bool {
    if !gates.stale_grants {
        return false;
    }
    let spec = &descriptor.spec;
    let scripting_closed = matches!(spec.gate, Gate::Scripting { .. }) && !gates.scripting;
    let reach_closed = !gates.reach
        && spec.scope == ScopeRule::WorkspaceUnlessAll
        && param_str(params, "scope") == Some("all");
    scripting_closed || reach_closed
}

fn decide(
    principal: &Principal,
    descriptor: &Descriptor,
    params: &Value,
    policy: &Policy,
    grants: &GrantView<'_>,
    check_gate: bool,
) -> Decision {
    let spec = &descriptor.spec;
    if let Principal::Unverified { .. } = principal {
        return if UNVERIFIED_ANSWERS.contains(&spec.name) {
            Decision::Allowed {
                scope: Scope::Instance,
                gates: Gates::CLOSED,
            }
        } else {
            Decision::Refused {
                reason: Refusal::UnverifiedCaller,
            }
        };
    }
    if !admits_audience(principal, spec.audience) {
        return Decision::Refused {
            reason: Refusal::PrincipalRefused,
        };
    }
    let gates = pane_gates(principal, spec, policy.gates_for(principal), grants);
    if principal.is_pane() && needs_unsynced_grant(descriptor, params, &gates) {
        return Decision::Refused {
            reason: Refusal::GrantsUnsynced,
        };
    }
    if let Gate::Scripting { refusal } = spec.gate
        && check_gate
        && !gates.scripting
    {
        return Decision::Refused {
            reason: Refusal::GateClosed { refusal },
        };
    }
    let scope = match principal {
        Principal::Pane { session, workspace } => {
            if param_str(params, SOURCE_SESSION).is_some_and(|source| source != session) {
                return Decision::Refused {
                    reason: Refusal::SourceMismatch,
                };
            }
            let own = Scope::Pane {
                session: session.clone(),
                workspace: workspace.clone(),
            };
            pane_scope(spec.scope, params, &gates, own)
        }
        _ => match param_str(params, SCOPE_SESSION) {
            Some(session) if spec.scope != ScopeRule::Unscoped => Scope::Claimed {
                session: session.to_string(),
            },
            _ => Scope::Instance,
        },
    };
    Decision::Allowed { scope, gates }
}

impl Scope {
    pub fn admits(&self, session: &str, workspace: Option<&str>) -> bool {
        match self {
            Self::Instance | Self::Claimed { .. } => true,
            Self::Pane {
                workspace: Some(own),
                ..
            } => workspace == Some(own.as_str()),
            Self::Pane {
                session: own,
                workspace: None,
            } => session == own,
        }
    }

    pub fn check(&self, session: &str, workspace: Option<&str>) -> Result<(), Refusal> {
        if self.admits(session, workspace) {
            Ok(())
        } else {
            Err(Refusal::ScopeRefused)
        }
    }

    pub fn bind(&self, principal: &Principal, params: &mut Value) {
        if !principal.is_pane() {
            return;
        }
        let Some(fields) = params.as_object_mut() else {
            return;
        };
        match self {
            Self::Pane { session, .. } => {
                fields.insert(SCOPE_SESSION.into(), Value::String(session.clone()));
            }
            Self::Instance | Self::Claimed { .. } => {
                fields.remove(SCOPE_SESSION);
            }
        }
    }
}

pub const fn scope_refusal(gates: &Gates) -> Refusal {
    if gates.stale_grants {
        Refusal::GrantsUnsynced
    } else {
        Refusal::ScopeRefused
    }
}

pub fn scope_refusal_message(method: &str, target: &str, workspace: Option<&str>) -> String {
    let place = workspace.map_or_else(
        || "no workspace".to_string(),
        |workspace| format!("workspace {workspace}"),
    );
    format!(
        "{method} refused: session {target} belongs to {place}, outside the workspace of the calling pane; \
         reaching another workspace needs ipc.orchestration"
    )
}

pub fn refusal_data(principal: &Principal, refusal: Refusal) -> Value {
    serde_json::json!({"reason": refusal.reason(), "principal": principal.kind()})
}

pub fn unverified_message(method: &str, principal: &Principal) -> String {
    match principal {
        Principal::Unverified { reason } => format!(
            "Paneflow could not identify the calling process: {reason}; {method} needs an identified caller"
        ),
        _ => format!("{method} needs an identified caller"),
    }
}

pub fn source_mismatch_message(method: &str) -> String {
    format!("{method} refused: source_session must be the calling pane's own session")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::methods::{host, layout, session, surface, system};
    use crate::policy::{Section, Switches};
    use serde_json::json;

    fn pane() -> Principal {
        Principal::Pane {
            session: "s-1".into(),
            workspace: Some("w-1".into()),
        }
    }

    fn closed() -> Policy {
        Policy::default()
    }

    fn orchestration() -> Policy {
        Policy {
            section: Section::Set(Switches::from_keys(None, Some(true))),
            ..Policy::default()
        }
    }

    #[test]
    fn the_authorize_module_reads_no_file_socket_or_environment() {
        let source = include_str!("authorize.rs");
        let body = source.split("#[cfg(test)]").next().expect("a body");
        for banned in [
            "std::fs",
            "std::env",
            "std::net",
            "UnixStream",
            "File::",
            "env::var",
        ] {
            assert!(!body.contains(banned), "{banned}");
        }
    }

    #[test]
    fn a_pane_is_refused_paneflow_only_and_not_pane_methods() {
        for descriptor in [
            Descriptor::of::<layout::Launch>(),
            Descriptor::of::<layout::Sweep>(),
            Descriptor::of::<layout::Attach>(),
            Descriptor::of::<host::PeerClassify>(),
            Descriptor::of::<session::HostShutdown>(),
            Descriptor::of::<session::ApprovalDecide>(),
        ] {
            assert_eq!(
                authorize(&pane(), &descriptor, &json!({}), &closed()),
                Decision::Refused {
                    reason: Refusal::PrincipalRefused
                },
                "{}",
                descriptor.name()
            );
        }
        let attach = Descriptor::of::<layout::Attach>();
        assert!(matches!(
            authorize(&Principal::External, &attach, &json!({}), &closed()),
            Decision::Allowed { .. }
        ));
        let launch = Descriptor::of::<layout::Launch>();
        assert!(matches!(
            authorize(&Principal::External, &launch, &json!({}), &closed()),
            Decision::Refused {
                reason: Refusal::PrincipalRefused
            }
        ));
    }

    #[test]
    fn a_pane_claiming_another_scope_session_is_scoped_to_its_own_workspace() {
        let read = Descriptor::of::<surface::Read>();
        let params = json!({"scope_session": "s-other"});
        let decision = authorize(&pane(), &read, &params, &closed());
        let Decision::Allowed { scope, .. } = decision else {
            panic!("a read is allowed");
        };
        assert_eq!(
            scope,
            Scope::Pane {
                session: "s-1".into(),
                workspace: Some("w-1".into())
            }
        );
        let mut bound = params.clone();
        scope.bind(&pane(), &mut bound);
        assert_eq!(bound, json!({"scope_session": "s-1"}));
    }

    #[test]
    fn a_pane_reaching_another_workspace_with_orchestration_closed_is_scope_refused() {
        let stop = Descriptor::of::<session::SessionStop>();
        let Decision::Allowed { scope, .. } =
            authorize(&pane(), &stop, &json!({"session": "s-2"}), &closed())
        else {
            panic!("stop is allowed within the workspace");
        };
        assert_eq!(scope.check("s-2", Some("w-2")), Err(Refusal::ScopeRefused));
        assert_eq!(scope.check("s-3", Some("w-1")), Ok(()));
        let Decision::Allowed { scope, .. } =
            authorize(&pane(), &stop, &json!({"session": "s-2"}), &orchestration())
        else {
            panic!("stop is allowed");
        };
        assert_eq!(scope, Scope::Instance);
    }

    #[test]
    fn an_external_scope_session_is_honored_as_it_is_sent() {
        let send = Descriptor::of::<surface::SendText>();
        let open = Policy {
            section: Section::Set(Switches::OPEN),
            ..Policy::default()
        };
        let mut params = json!({"scope_session": "s-claimed", "text": "x"});
        let Decision::Allowed { scope, .. } =
            authorize(&Principal::External, &send, &params, &open)
        else {
            panic!("send is allowed");
        };
        assert_eq!(
            scope,
            Scope::Claimed {
                session: "s-claimed".into()
            }
        );
        scope.bind(&Principal::External, &mut params);
        assert_eq!(params["scope_session"], "s-claimed");
    }

    #[test]
    fn an_unverified_caller_reaches_only_the_three_introspection_methods() {
        let unverified = Principal::Unverified {
            reason: "gone".into(),
        };
        for descriptor in crate::host_catalog().expect("the catalog builds").iter() {
            let decision = authorize(&unverified, descriptor, &json!({}), &closed());
            if UNVERIFIED_ANSWERS.contains(&descriptor.name()) {
                assert!(matches!(decision, Decision::Allowed { .. }));
            } else {
                assert_eq!(
                    decision,
                    Decision::Refused {
                        reason: Refusal::UnverifiedCaller
                    },
                    "{}",
                    descriptor.name()
                );
            }
        }
        assert!(
            UNVERIFIED_ANSWERS
                .iter()
                .all(|name| crate::catalog().unwrap().get(name).is_some())
        );
        let _ = system::WhoAmI;
    }

    #[test]
    fn a_pane_writing_from_another_source_session_is_refused() {
        let write = Descriptor::of::<host::PaneWrite>();
        let open = Policy {
            ai_unrestricted: true,
            ..Policy::default()
        };
        assert_eq!(
            authorize(
                &pane(),
                &write,
                &json!({"source_session": "s-9", "session": "s-2", "text": "x"}),
                &open
            ),
            Decision::Refused {
                reason: Refusal::SourceMismatch
            }
        );
        assert!(matches!(
            authorize(
                &pane(),
                &write,
                &json!({"source_session": "s-1", "session": "s-2", "text": "x"}),
                &open
            ),
            Decision::Allowed { .. }
        ));
    }

    fn held(
        rights: &[crate::grants::Right],
        scope: crate::grants::GrantScope,
    ) -> crate::grants::PaneGrant {
        crate::grants::PaneGrant {
            id: "g-1".into(),
            holder: crate::grants::GrantHolder {
                session: "s-1".into(),
                generation: 3,
                workspace: Some("w-1".into()),
            },
            rights: rights.to_vec(),
            scope,
            expires_at_ms: Some(1_000),
            delegated_from: None,
            origin: crate::grants::GrantOrigin::Prompt,
            granted_at_ms: 0,
        }
    }

    #[test]
    fn a_live_grant_opens_the_gate_it_covers_and_only_an_instance_grant_reaches_further() {
        use crate::grants::{GrantScope, GrantView, Right};
        let send = Descriptor::of::<surface::SendText>();
        let stop = Descriptor::of::<session::SessionStop>();
        let workspace = [held(&[Right::Write], GrantScope::Workspace)];
        let view = GrantView {
            grants: &workspace,
            now_ms: 10,
            generation: Some(3),
            synced: true,
        };
        let Decision::Allowed { scope, gates } =
            authorize_with(&pane(), &send, &json!({"scope": "all"}), &closed(), &view)
        else {
            panic!("a write grant opens send_text");
        };
        assert!(gates.scripting);
        assert_eq!(
            scope,
            Scope::Pane {
                session: "s-1".into(),
                workspace: Some("w-1".into())
            }
        );
        let Decision::Allowed { scope, .. } =
            authorize_with(&pane(), &stop, &json!({}), &closed(), &view)
        else {
            panic!("stop is allowed");
        };
        assert_eq!(scope.check("s-9", Some("w-9")), Err(Refusal::ScopeRefused));
        let instance = [held(&[Right::Write], GrantScope::Instance)];
        let view = GrantView {
            grants: &instance,
            ..view
        };
        let Decision::Allowed { scope, .. } =
            authorize_with(&pane(), &stop, &json!({}), &closed(), &view)
        else {
            panic!("stop is allowed");
        };
        assert_eq!(scope, Scope::Instance);
        let expired = GrantView {
            now_ms: 1_000,
            ..view
        };
        assert!(matches!(
            authorize_with(&pane(), &send, &json!({}), &closed(), &expired),
            Decision::Refused {
                reason: Refusal::GateClosed { .. }
            }
        ));
    }

    #[test]
    fn a_stale_grant_set_refuses_what_needs_a_grant_and_lets_the_rest_through() {
        use crate::grants::GrantView;
        let stale = GrantView {
            grants: &[],
            now_ms: 0,
            generation: None,
            synced: false,
        };
        let send = Descriptor::of::<surface::SendText>();
        assert_eq!(
            authorize_with(&pane(), &send, &json!({}), &closed(), &stale),
            Decision::Refused {
                reason: Refusal::GrantsUnsynced
            }
        );
        let write = Descriptor::of::<host::PaneWrite>();
        assert_eq!(
            authorize_with(
                &pane(),
                &write,
                &json!({"source_session": "s-1", "scope": "all"}),
                &closed(),
                &stale
            ),
            Decision::Refused {
                reason: Refusal::GrantsUnsynced
            }
        );
        let read = Descriptor::of::<surface::Read>();
        assert!(matches!(
            authorize_with(&pane(), &read, &json!({}), &closed(), &stale),
            Decision::Allowed { .. }
        ));
        assert!(matches!(
            authorize_with(&Principal::External, &send, &json!({}), &closed(), &stale),
            Decision::Refused {
                reason: Refusal::GateClosed { .. }
            }
        ));
        let open = Policy {
            section: Section::Set(Switches::OPEN),
            ..Policy::default()
        };
        assert!(matches!(
            authorize_with(&pane(), &send, &json!({}), &open, &stale),
            Decision::Allowed { .. }
        ));
    }

    #[test]
    fn a_closed_scripting_gate_refuses_with_the_declared_message() {
        let send = Descriptor::of::<surface::SendText>();
        let Decision::Refused {
            reason: Refusal::GateClosed { refusal },
        } = authorize(&pane(), &send, &json!({}), &closed())
        else {
            panic!("closed");
        };
        assert_eq!(
            refusal,
            "surface.send_text disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable"
        );
        assert!(matches!(
            authorize(&Principal::Window, &send, &json!({}), &closed()),
            Decision::Allowed { .. }
        ));
    }
}
