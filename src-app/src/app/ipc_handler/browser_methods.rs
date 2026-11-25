use std::collections::HashSet;

use gpui::{App, Context};
use paneflow_api::{Method, Principal};
use paneflow_browser_control::{CallContext, Identity};
use paneflow_browser_proto::ProfileName;

use super::surface_methods::session_directory;
use super::{Call, IpcReply, JsonRpcError};
use crate::PaneFlowApp;
use crate::workspace::Workspace;

fn profile_of(workspace: &Workspace) -> Option<ProfileName> {
    ProfileName::new(workspace.durable_id.to_string()).ok()
}

pub(super) fn browser_context(
    workspaces: &[Workspace],
    active: Option<&Workspace>,
    caller: &Principal,
    raw: &serde_json::Value,
    reach: bool,
    cx: &App,
) -> Result<CallContext, JsonRpcError> {
    let directory = session_directory(workspaces, cx);
    let live: HashSet<String> = directory
        .iter()
        .map(|entry| entry.session.clone())
        .collect();
    match caller {
        Principal::Pane { session, workspace } => {
            let entry = directory.iter().find(|entry| entry.session == *session);
            let own = entry
                .and_then(|entry| workspaces.get(entry.workspace_idx))
                .and_then(profile_of)
                .or_else(|| {
                    workspace
                        .as_deref()
                        .and_then(|workspace| ProfileName::new(workspace).ok())
                })
                .ok_or_else(|| {
                    JsonRpcError::invalid_params(format!(
                        "the calling pane's session {session} is open in no workspace of this window"
                    ))
                })?;
            let everywhere =
                reach && raw.get("scope").and_then(serde_json::Value::as_str) == Some("all");
            let agent = entry
                .and_then(|entry| entry.agent.as_deref())
                .map(|binary| {
                    crate::agent_launcher::TerminalAgent::from_binary(binary).map_or_else(
                        || binary.to_string(),
                        |agent| agent.display_name().to_string(),
                    )
                });
            Ok(CallContext {
                identity: Identity::Agent {
                    session: session.clone(),
                    agent,
                },
                workspace: Some(own.clone()),
                visible: (!everywhere).then_some(own),
                live_sessions: Some(live),
            })
        }
        Principal::Unverified { .. } => Err(JsonRpcError::invalid_params(
            "the browser needs an identified caller",
        )),
        Principal::Window | Principal::Worker | Principal::External => {
            let requested = raw.get("workspace").and_then(serde_json::Value::as_str);
            let workspace = match requested {
                Some(requested) => Some(
                    workspaces
                        .iter()
                        .find(|workspace| workspace.durable_id.to_string() == requested)
                        .and_then(profile_of)
                        .ok_or_else(|| {
                            JsonRpcError::invalid_params(format!(
                                "no workspace of this window has the durable id {requested}"
                            ))
                        })?,
                ),
                None => active.and_then(profile_of),
            };
            Ok(CallContext {
                identity: Identity::Human,
                workspace,
                visible: None,
                live_sessions: Some(live),
            })
        }
    }
}

impl PaneFlowApp {
    pub(super) fn browser_reply<M: Method>(
        &mut self,
        call: Call<'_, M::Params>,
        cx: &mut Context<Self>,
    ) -> IpcReply {
        let context = match browser_context(
            &self.workspaces,
            self.active_workspace(),
            call.caller,
            call.raw,
            self.ipc_gates.reach,
            cx,
        ) {
            Ok(context) => context,
            Err(error) => return IpcReply::Ready(error.into_value()),
        };
        let control = match crate::browser_control::control() {
            Ok(control) => control,
            Err(message) => return IpcReply::Ready(JsonRpcError::internal(message).into_value()),
        };
        let params = call.raw.clone();
        IpcReply::Deferred(Box::new(move || {
            crate::browser_control::answer(control, M::SPEC.name, params, &context)
        }))
    }
}
