use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::descriptor::{Gate, Spec};
use crate::policy::Gates;
use crate::principal::Principal;

pub const REASON_GRANTS_UNSYNCED: &str = "grants_unsynced";
pub const REASON_GRANT_EXCEEDS_HOLDER: &str = "grant_exceeds_holder";
pub const REASON_NOT_SPAWNED_BY_CALLER: &str = "not_spawned_by_caller";
pub const REASON_DENIED: &str = "denied";
pub const REASON_UNANSWERED: &str = "unanswered";

pub const GRANTS_UNSYNCED_MESSAGE: &str = "Grants are being synchronized; retry the call";

pub const REASON_GRANT_LIMIT: &str = "grant_limit";

pub const MAX_GRANTS: usize = 128;
pub const MAX_GRANTS_PER_HOLDER: usize = 16;

pub const WORKSPACE_SPAWN_METHODS: [&str; 1] = ["surface.split"];

pub const LAYOUT_RIGHT_METHODS: [&str; 4] = [
    "layout.apply",
    "layout.adopt",
    "workspace.restore_layout",
    "workspace.up",
];

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Right {
    Write,
    Spawn,
    Layout,
    Browser,
}

impl Right {
    pub const ALL: [Self; 4] = [Self::Write, Self::Spawn, Self::Layout, Self::Browser];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Spawn => "spawn",
            Self::Layout => "layout",
            Self::Browser => "browser",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|right| right.label() == raw)
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum GrantScope {
    #[default]
    Workspace,
    Instance,
}

impl GrantScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Instance => "instance",
        }
    }

    pub const fn covers(self, needed: Self) -> bool {
        matches!((self, needed), (Self::Instance, _) | (_, Self::Workspace))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantHolder {
    pub session: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GrantOrigin {
    Prompt,
    Profile { profile: String },
    Delegation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaneGrant {
    pub id: String,
    pub holder: GrantHolder,
    pub rights: Vec<Right>,
    pub scope: GrantScope,
    #[schemars(description = "Unix time in milliseconds after which the grant no longer applies")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_from: Option<String>,
    pub origin: GrantOrigin,
    pub granted_at_ms: u64,
}

pub fn normalized_rights(rights: &[Right]) -> Vec<Right> {
    let mut rights = rights.to_vec();
    rights.sort_unstable();
    rights.dedup();
    rights
}

impl PaneGrant {
    pub fn is_live_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_none_or(|expiry| now_ms < expiry)
    }

    pub fn is_held_by(&self, session: &str, generation: Option<u64>) -> bool {
        self.holder.session == session && generation == Some(self.holder.generation)
    }

    pub fn covers(&self, right: Right, scope: GrantScope) -> bool {
        self.rights.contains(&right) && self.scope.covers(scope)
    }

    pub fn contains(&self, narrower: &Self) -> bool {
        narrower
            .rights
            .iter()
            .all(|right| self.rights.contains(right))
            && self.scope.covers(narrower.scope)
            && match (self.expires_at_ms, narrower.expires_at_ms) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(limit), Some(expiry)) => expiry <= limit,
            }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantSet {
    pub revision: u64,
    pub grants: Vec<PaneGrant>,
}

#[derive(Debug, Clone, Copy)]
pub struct GrantView<'a> {
    pub grants: &'a [PaneGrant],
    pub now_ms: u64,
    pub generation: Option<u64>,
    pub synced: bool,
}

impl GrantView<'static> {
    pub const NONE: Self = Self {
        grants: &[],
        now_ms: 0,
        generation: None,
        synced: true,
    };
}

impl GrantView<'_> {
    fn held<'g>(&'g self, session: &'g str) -> impl Iterator<Item = &'g PaneGrant> + 'g {
        self.grants
            .iter()
            .filter(move |grant| grant.is_held_by(session, self.generation))
            .filter(move |grant| grant.is_live_at(self.now_ms))
    }

    pub fn covers(&self, session: &str, right: Right, scope: GrantScope) -> bool {
        self.held(session).any(|grant| grant.covers(right, scope))
    }
}

pub fn needed_right(spec: &Spec) -> Option<Right> {
    match spec.gate {
        Gate::Open => None,
        Gate::Scripting { .. } => Some(Right::Write),
        Gate::OrchestrationWhenSpawning if LAYOUT_RIGHT_METHODS.contains(&spec.name) => {
            Some(Right::Layout)
        }
        Gate::OrchestrationWhenSpawning => Some(Right::Spawn),
        Gate::Browser { .. } => Some(Right::Browser),
    }
}

pub fn reach_right(spec: &Spec) -> Right {
    needed_right(spec).unwrap_or(Right::Write)
}

pub fn pane_gates(principal: &Principal, spec: &Spec, base: Gates, view: &GrantView<'_>) -> Gates {
    let Principal::Pane { session, .. } = principal else {
        return base;
    };
    let mut gates = base;
    if !view.synced {
        gates.stale_grants = true;
        return gates;
    }
    if let Some(right) = needed_right(spec) {
        let everywhere = view.covers(session, right, GrantScope::Instance);
        let own_workspace = view.covers(session, right, GrantScope::Workspace);
        match right {
            Right::Write if own_workspace => gates.scripting = true,
            Right::Browser if own_workspace => gates.browser = true,
            Right::Spawn | Right::Layout if everywhere => gates.orchestration = true,
            Right::Spawn
                if own_workspace
                    && !gates.orchestration
                    && WORKSPACE_SPAWN_METHODS.contains(&spec.name) =>
            {
                gates.orchestration = true;
                gates.spawn_confined = true;
            }
            _ => {}
        }
    }
    if view.covers(session, reach_right(spec), GrantScope::Instance) {
        gates.reach = true;
    }
    gates
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::Descriptor;
    use crate::methods::{layout, session, surface, workspace};

    fn grant(rights: &[Right], scope: GrantScope, expires_at_ms: Option<u64>) -> PaneGrant {
        PaneGrant {
            id: "g-1".into(),
            holder: GrantHolder {
                session: "s-1".into(),
                generation: 1,
                workspace: Some("w-1".into()),
            },
            rights: normalized_rights(rights),
            scope,
            expires_at_ms,
            delegated_from: None,
            origin: GrantOrigin::Prompt,
            granted_at_ms: 0,
        }
    }

    fn pane() -> Principal {
        Principal::Pane {
            session: "s-1".into(),
            workspace: Some("w-1".into()),
        }
    }

    #[test]
    fn a_gated_method_needs_the_right_its_gate_names() {
        assert_eq!(
            needed_right(&Descriptor::of::<surface::SendText>().spec),
            Some(Right::Write)
        );
        assert_eq!(
            needed_right(&Descriptor::of::<surface::Split>().spec),
            Some(Right::Spawn)
        );
        assert_eq!(
            needed_right(&Descriptor::of::<layout::Apply>().spec),
            Some(Right::Layout)
        );
        assert_eq!(
            needed_right(&Descriptor::of::<workspace::Up>().spec),
            Some(Right::Layout)
        );
        assert_eq!(
            needed_right(&Descriptor::of::<session::SessionStop>().spec),
            None
        );
    }

    #[test]
    fn a_live_grant_of_the_holder_opens_only_what_it_names() {
        let grants = [grant(&[Right::Write], GrantScope::Workspace, Some(100))];
        let view = GrantView {
            grants: &grants,
            now_ms: 50,
            generation: Some(1),
            synced: true,
        };
        let send = Descriptor::of::<surface::SendText>().spec;
        let gates = pane_gates(&pane(), &send, Gates::CLOSED, &view);
        assert!(gates.scripting && !gates.orchestration && !gates.reach);
        let split = Descriptor::of::<surface::Split>().spec;
        assert_eq!(
            pane_gates(&pane(), &split, Gates::CLOSED, &view),
            Gates::CLOSED
        );
        let expired = GrantView {
            now_ms: 100,
            ..view
        };
        assert_eq!(
            pane_gates(&pane(), &send, Gates::CLOSED, &expired),
            Gates::CLOSED
        );
        let restarted = GrantView {
            generation: Some(2),
            ..view
        };
        assert_eq!(
            pane_gates(&pane(), &send, Gates::CLOSED, &restarted),
            Gates::CLOSED
        );
        let unknown = GrantView {
            generation: None,
            ..view
        };
        assert_eq!(
            pane_gates(&pane(), &send, Gates::CLOSED, &unknown),
            Gates::CLOSED
        );
        assert_eq!(
            pane_gates(&Principal::External, &send, Gates::CLOSED, &view),
            Gates::CLOSED
        );
    }

    #[test]
    fn a_stale_view_applies_no_grant_and_marks_the_gates_stale() {
        let grants = [grant(&[Right::Write], GrantScope::Instance, None)];
        let view = GrantView {
            grants: &grants,
            now_ms: 0,
            generation: Some(1),
            synced: false,
        };
        let send = Descriptor::of::<surface::SendText>().spec;
        let gates = pane_gates(&pane(), &send, Gates::CLOSED, &view);
        assert!(gates.stale_grants && !gates.scripting && !gates.reach);
        assert!(!pane_gates(&Principal::External, &send, Gates::CLOSED, &view).stale_grants);
    }

    #[test]
    fn an_instance_grant_reaches_other_workspaces_and_a_workspace_grant_does_not() {
        let stop = Descriptor::of::<session::SessionStop>().spec;
        let workspace = [grant(&[Right::Write], GrantScope::Workspace, None)];
        let view = GrantView {
            grants: &workspace,
            now_ms: 0,
            generation: Some(1),
            synced: true,
        };
        assert!(!pane_gates(&pane(), &stop, Gates::CLOSED, &view).reach);
        let instance = [grant(&[Right::Write], GrantScope::Instance, None)];
        let view = GrantView {
            grants: &instance,
            ..view
        };
        assert!(pane_gates(&pane(), &stop, Gates::CLOSED, &view).reach);
    }

    #[test]
    fn a_workspace_grant_opens_no_spawn_it_cannot_confine_to_the_workspace() {
        fn view_of(grants: &[PaneGrant]) -> GrantView<'_> {
            GrantView {
                grants,
                now_ms: 0,
                generation: Some(1),
                synced: true,
            }
        }
        let rights = [Right::Spawn, Right::Layout];
        let workspace = &[grant(&rights, GrantScope::Workspace, None)];
        let instance = &[grant(&rights, GrantScope::Instance, None)];
        let split = Descriptor::of::<surface::Split>().spec;
        let confined = pane_gates(&pane(), &split, Gates::CLOSED, &view_of(workspace));
        assert!(confined.orchestration && confined.spawn_confined && !confined.reach);
        let unconfined = [
            Descriptor::of::<workspace::Create>().spec,
            Descriptor::of::<workspace::Up>().spec,
            Descriptor::of::<workspace::RestoreLayout>().spec,
            Descriptor::of::<layout::Apply>().spec,
            Descriptor::of::<session::SessionCreate>().spec,
        ];
        for spec in &unconfined {
            assert_eq!(
                pane_gates(&pane(), spec, Gates::CLOSED, &view_of(workspace)),
                Gates::CLOSED,
                "{}",
                spec.name
            );
            let open = pane_gates(&pane(), spec, Gates::CLOSED, &view_of(instance));
            assert!(open.orchestration && !open.spawn_confined, "{}", spec.name);
        }
        let everywhere = pane_gates(&pane(), &split, Gates::CLOSED, &view_of(instance));
        assert!(everywhere.orchestration && !everywhere.spawn_confined);
        let by_policy = pane_gates(&pane(), &split, Gates::OPEN, &view_of(workspace));
        assert!(by_policy.orchestration && !by_policy.spawn_confined);
    }

    #[test]
    fn a_grant_contains_only_narrower_rights_scope_and_expiry() {
        let holder = grant(
            &[Right::Write, Right::Spawn],
            GrantScope::Workspace,
            Some(100),
        );
        assert!(holder.contains(&grant(&[Right::Write], GrantScope::Workspace, Some(90))));
        assert!(!holder.contains(&grant(&[Right::Layout], GrantScope::Workspace, Some(90))));
        assert!(!holder.contains(&grant(&[Right::Write], GrantScope::Instance, Some(90))));
        assert!(!holder.contains(&grant(&[Right::Write], GrantScope::Workspace, Some(101))));
        assert!(!holder.contains(&grant(&[Right::Write], GrantScope::Workspace, None)));
    }

    #[test]
    fn a_right_parses_only_its_label() {
        for right in Right::ALL {
            assert_eq!(Right::parse(right.label()), Some(right));
        }
        assert_eq!(Right::parse("admin"), None);
    }
}
