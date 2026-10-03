use super::*;
use crate::app::close_policy::{CloseIntent, CloseTarget};
use crate::tmux_compat::command::{self, PaneContext, TmuxCommand};
use crate::tmux_compat::teams::{LEADER, Team, TeamError};
use crate::tmux_compat::{TEAM_ENV, compat_dir, teammate_command};

const MAX_ARGV: usize = 256;
const MAX_ARGV_BYTES: usize = 256 * 1024;
const PLACEHOLDER_COMMAND: &str = "cat";

struct TmuxRequest {
    team: String,
    surface_id: u64,
    argv: Vec<String>,
    scope_session: Option<String>,
}

fn tmux_request(params: &serde_json::Value) -> Result<TmuxRequest, JsonRpcError> {
    let team = opt_str(params, "team")?
        .filter(|team| !team.is_empty() && team.len() <= 64)
        .ok_or_else(|| JsonRpcError::invalid_params("Missing or invalid 'team'"))?
        .to_string();
    let surface_id = opt_u64(params, "surface_id")?
        .ok_or_else(|| JsonRpcError::invalid_params("Missing 'surface_id'"))?;
    let argv = params
        .get("argv")
        .and_then(serde_json::Value::as_array)
        .filter(|argv| argv.len() <= MAX_ARGV)
        .and_then(|argv| {
            argv.iter()
                .map(|arg| arg.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .filter(|argv| argv.iter().map(String::len).sum::<usize>() <= MAX_ARGV_BYTES)
        .ok_or_else(|| JsonRpcError::invalid_params("'argv' must be a bounded string array"))?;
    let scope_session = opt_str(params, "scope_session")?.map(str::to_string);
    Ok(TmuxRequest {
        team,
        surface_id,
        argv,
        scope_session,
    })
}

fn outcome(stdout: String, stderr: String, exit: i32) -> serde_json::Value {
    serde_json::json!({ "stdout": stdout, "stderr": stderr, "exit": exit })
}

fn success(stdout: String) -> serde_json::Value {
    outcome(stdout, String::new(), 0)
}

fn failure(stderr: String) -> serde_json::Value {
    outcome(String::new(), stderr, 1)
}

fn pane_line(team: &Team, local: u32, caller: u32, format: &str) -> String {
    command::expand_format(
        format,
        &PaneContext {
            local,
            title: team.title(local),
            active: local == caller,
        },
    )
}

fn error_message(value: &serde_json::Value) -> String {
    value
        .get(JSONRPC_ERROR_KEY)
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("the pane could not be created")
        .to_string()
}

impl PaneFlowApp {
    pub(super) fn tmux_compat_reply(
        &mut self,
        params: &serde_json::Value,
        caller_pid: Option<i64>,
        cx: &mut Context<Self>,
    ) -> IpcReply {
        let request = match tmux_request(params) {
            Ok(request) => request,
            Err(error) => return IpcReply::Ready(error.into_value()),
        };
        let live: HashSet<u64> = self
            .collect_surface_generations(cx)
            .into_iter()
            .map(|(surface, _)| surface)
            .collect();
        let caller = match self
            .tmux_teams
            .enter(&request.team, request.surface_id, |surface| {
                live.contains(&surface)
            }) {
            Ok(caller) => caller,
            Err(error) => {
                log::warn!(
                    "tmux-compat: surface {} refused: {}",
                    request.surface_id,
                    error.message()
                );
                return IpcReply::Ready(failure(error.message()));
            }
        };
        let parsed = match command::parse(&request.argv) {
            Ok(parsed) => parsed,
            Err(message) => {
                return IpcReply::Ready(failure(format!("paneflow tmux-compat: {message}")));
            }
        };
        match self.run_tmux_command(&request, caller, parsed, caller_pid, cx) {
            Ok(reply) => reply,
            Err(error) => IpcReply::Ready(failure(error.message())),
        }
    }

    fn run_tmux_command(
        &mut self,
        request: &TmuxRequest,
        caller: u32,
        parsed: TmuxCommand,
        caller_pid: Option<i64>,
        cx: &mut Context<Self>,
    ) -> Result<IpcReply, TeamError> {
        let ready = |value| Ok(IpcReply::Ready(value));
        let team = self.tmux_teams.team_mut(&request.team)?;
        match parsed {
            TmuxCommand::Accepted => ready(success(String::new())),
            TmuxCommand::Display { target, format } => {
                let local = team.resolve(&target, caller)?;
                let text = format.map_or_else(String::new, |format| {
                    pane_line(team, local, caller, &format)
                });
                ready(success(text))
            }
            TmuxCommand::ListPanes { format } => {
                let lines: Vec<String> = team
                    .locals()
                    .into_iter()
                    .map(|local| pane_line(team, local, caller, &format))
                    .collect();
                ready(success(lines.join("\n")))
            }
            TmuxCommand::ListWindows { format } => {
                ready(success(pane_line(team, LEADER, caller, &format)))
            }
            TmuxCommand::Split {
                target,
                side_by_side,
                print,
                cwd,
                command,
            } => {
                let anchor = team.resolve(&target, caller)?;
                let local = team.reserve(anchor, side_by_side)?;
                let printed = print
                    .map(|format| pane_line(team, local, caller, &format))
                    .unwrap_or_default();
                match command.filter(|command| command.trim() != PLACEHOLDER_COMMAND) {
                    Some(command) => {
                        Ok(self.start_team_pane(request, local, cwd, command, printed, cx))
                    }
                    None => ready(success(printed)),
                }
            }
            TmuxCommand::SelectPane { target, title } => {
                let local = team.resolve(&target, caller)?;
                if let Some(title) = title {
                    let surface = team.surface_of(local);
                    team.set_title(local, title.clone());
                    if let Some(surface) = surface
                        && let Some(terminal) =
                            find_terminal_by_surface_id(&self.workspaces, surface, cx)
                        && let Some(name) = sanitize_pane_name(&title)
                    {
                        terminal.update(cx, |view, cx| {
                            view.terminal.custom_name = Some(name);
                            cx.notify();
                        });
                    }
                }
                ready(success(String::new()))
            }
            TmuxCommand::Respawn {
                target,
                cwd,
                command,
            } => {
                let local = team.resolve(&target, caller)?;
                let busy = match team.pane(local) {
                    None => Some("the team leader pane cannot be respawned".to_string()),
                    Some(pane) if pane.surface.is_some() || pane.spawning => {
                        Some(format!("pane %{local} is already running"))
                    }
                    Some(_) => None,
                };
                match busy {
                    Some(message) => ready(failure(format!("paneflow tmux-compat: {message}"))),
                    None => {
                        Ok(self.start_team_pane(request, local, cwd, command, String::new(), cx))
                    }
                }
            }
            TmuxCommand::Kill { target } => {
                let local = team.resolve(&target, caller)?;
                if local == LEADER {
                    return ready(failure(
                        "paneflow tmux-compat: the team leader pane is closed by its user, not by the team"
                            .to_string(),
                    ));
                }
                if let Some(surface) = team.remove(local).and_then(|pane| pane.surface) {
                    self.close_team_surface(surface, cx);
                }
                ready(success(String::new()))
            }
            TmuxCommand::SendKeys {
                target,
                literal,
                keys,
            } => {
                let local = team.resolve(&target, caller)?;
                let Some(surface) = team.surface_of(local) else {
                    return ready(failure(format!(
                        "paneflow tmux-compat: pane %{local} has not started"
                    )));
                };
                let mut params = serde_json::json!({
                    "surface_id": surface,
                    "text": command::keys_to_text(&keys, literal),
                    "paste": false,
                });
                if let Some(session) = &request.scope_session {
                    params["scope_session"] = serde_json::Value::String(session.clone());
                }
                let reply =
                    self.handle_surface_method("surface.send_text", &params, caller_pid, cx);
                if reply.get(JSONRPC_ERROR_KEY).is_some() {
                    return ready(failure(error_message(&reply)));
                }
                ready(success(String::new()))
            }
            TmuxCommand::Unsupported(verb) => {
                log::warn!(
                    "tmux-compat: surface {} used an unsupported verb {verb:?}",
                    request.surface_id
                );
                ready(failure(command::unsupported_verb_message(&verb)))
            }
        }
    }

    fn start_team_pane(
        &mut self,
        request: &TmuxRequest,
        local: u32,
        cwd: Option<String>,
        command: String,
        printed: String,
        cx: &mut Context<Self>,
    ) -> IpcReply {
        let token = request.team.clone();
        let prepared = self
            .tmux_teams
            .team_mut(&token)
            .map_err(|error| error.message())
            .and_then(|team| {
                let anchor = team.anchor_surface(local).ok_or_else(|| {
                    "paneflow tmux-compat: the team has no live pane to split".to_string()
                })?;
                let pane = team
                    .pane_mut(local)
                    .ok_or_else(|| TeamError::NoSuchPane(format!("%{local}")).message())?;
                pane.spawning = true;
                Ok((anchor, pane.side_by_side, pane.title.clone()))
            });
        let (anchor, side_by_side, title) = match prepared {
            Ok(prepared) => prepared,
            Err(message) => return IpcReply::Ready(failure(message)),
        };
        let wrapped = compat_dir()
            .ok_or_else(|| "the Paneflow home directory cannot be resolved".to_string())
            .and_then(|dir| teammate_command(&dir, local, &command));
        let wrapped = match wrapped {
            Ok(wrapped) => wrapped,
            Err(message) => {
                self.forget_team_pane(&token, local);
                return IpcReply::Ready(failure(format!("paneflow tmux-compat: {message}")));
            }
        };
        let mut params = serde_json::json!({
            "direction": if side_by_side { "vertical" } else { "horizontal" },
            "surface_id": anchor,
            "command": wrapped,
            "env": { TEAM_ENV: token },
        });
        if !title.is_empty() {
            params["name"] = serde_json::Value::String(title);
        }
        if let Some(cwd) = cwd {
            params["cwd"] = serde_json::Value::String(cwd);
        }
        let task = match self.prepare_split(&params, true, cx) {
            Ok(task) => task,
            Err(error) => {
                self.forget_team_pane(&token, local);
                return IpcReply::Ready(failure(format!(
                    "paneflow tmux-compat: {}",
                    error.message
                )));
            }
        };
        IpcReply::Async(cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let result = task.await;
            let surface = result.get("surface_id").and_then(serde_json::Value::as_u64);
            let recorded = this.update(cx, |app, _cx| match surface {
                Some(surface) => {
                    if let Ok(team) = app.tmux_teams.team_mut(&token) {
                        team.attach(local, surface);
                    }
                }
                None => app.forget_team_pane(&token, local),
            });
            match (surface, recorded) {
                (Some(_), Ok(())) => success(printed),
                (None, _) => failure(format!("paneflow tmux-compat: {}", error_message(&result))),
                (Some(_), Err(_)) => app_shutting_down(),
            }
        }))
    }

    fn forget_team_pane(&mut self, token: &str, local: u32) {
        if let Ok(team) = self.tmux_teams.team_mut(token) {
            team.remove(local);
        }
    }

    fn close_team_surface(&mut self, surface: u64, cx: &mut Context<Self>) {
        let found = self.workspaces.iter().find_map(|ws| {
            ws.collect_panes().into_iter().find_map(|pane| {
                let pane_ref = pane.read(cx);
                let count = pane_ref.terminals().count();
                pane_ref
                    .terminals()
                    .find(|terminal| terminal.entity_id().as_u64() == surface)
                    .map(|terminal| (pane.clone(), terminal.clone(), count))
            })
        });
        let Some((pane, terminal, count)) = found else {
            return;
        };
        let target = if count == 1 {
            CloseTarget::Pane(pane)
        } else {
            CloseTarget::Surface { pane, terminal }
        };
        self.perform_close(target, CloseIntent::Stop, None, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_needs_a_team_a_surface_and_a_bounded_argv() {
        for params in [
            serde_json::json!({"surface_id": 1, "argv": ["list-panes"]}),
            serde_json::json!({"team": "", "surface_id": 1, "argv": ["list-panes"]}),
            serde_json::json!({"team": "t", "argv": ["list-panes"]}),
            serde_json::json!({"team": "t", "surface_id": 1, "argv": [1]}),
            serde_json::json!({"team": "t", "surface_id": 1, "argv": vec!["x"; MAX_ARGV + 1]}),
        ] {
            assert!(tmux_request(&params).is_err(), "{params}");
        }
        let request = tmux_request(&serde_json::json!({
            "team": "t", "surface_id": 4, "argv": ["list-panes"], "scope_session": "s"
        }))
        .expect("valid");
        assert_eq!(request.surface_id, 4);
        assert_eq!(request.scope_session.as_deref(), Some("s"));
    }

    #[test]
    fn listing_formats_every_team_pane_leader_first() {
        let mut teams = crate::tmux_compat::teams::TmuxTeams::default();
        let token = teams.issue();
        teams.enter(&token, 10, |_| true).expect("leader");
        let team = teams.team_mut(&token).expect("team");
        let first = team.reserve(LEADER, true).expect("reserve");
        team.set_title(first, "teammate-1".to_string());
        let lines: Vec<String> = team
            .locals()
            .into_iter()
            .map(|local| pane_line(team, local, LEADER, "#{pane_id} #{pane_title}"))
            .collect();
        assert_eq!(
            lines,
            vec!["%0 ".to_string(), format!("%{first} teammate-1")]
        );
    }
}
