use super::*;
use paneflow_config::schema::CommandDefinition;
use serde_json::{Value, json};

use crate::agent_launcher::TerminalAgent;
use crate::app::ipc_handler::{
    build_up_layout, canonicalize_workspace_cwd, dedupe_planned_pane_labels,
    parse_workspace_pane_plan,
};
use templates::*;

impl PaneFlowApp {
    pub(crate) fn workspace_template_for_workspace(&self, workspace_idx: usize) -> Option<usize> {
        let workspace = self.workspaces.get(workspace_idx)?;
        let project = canonicalize_workspace_cwd(&workspace.cwd).ok()?;
        self.cached_config
            .commands
            .iter()
            .enumerate()
            .find_map(|(idx, command)| {
                let workflow = command.workspace()?;
                if template_surfaces(workflow).is_empty() {
                    return None;
                }
                let workflow_cwd = workflow.cwd.as_deref()?.trim();
                if workflow_cwd.is_empty() {
                    return None;
                }
                let workflow_project = canonicalize_workspace_cwd(workflow_cwd).ok()?;
                paths_equal(&project, &workflow_project).then_some(idx)
            })
    }

    pub(crate) fn run_saved_workspace_template_for_workspace(
        &mut self,
        workspace_idx: usize,
        template_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let commands = self.cached_config.commands.clone();
        let name = commands
            .get(template_idx)
            .map(|command| command.name.clone())
            .unwrap_or_else(|| "Workflow".to_string());
        let result = self
            .workspace_up_params(&commands, template_idx)
            .and_then(|params| {
                self.launch_workspace_params_in_open_workspace(&params, workspace_idx, cx)
            });

        match result {
            Ok(_) => self.show_toast(format!("{name} started"), cx),
            Err(message) => self.show_toast(format!("Workflow failed: {message}"), cx),
        }
    }

    pub(crate) fn open_workspace_index_for_project(&self, project: &str) -> Option<usize> {
        let project = canonicalize_workspace_cwd(project).ok()?;
        if let Some(active) = self.workspaces.get(self.active_idx)
            && workspace_cwd_matches(&active.cwd, &project)
        {
            return Some(self.active_idx);
        }
        self.workspaces
            .iter()
            .enumerate()
            .find_map(|(idx, workspace)| {
                workspace_cwd_matches(&workspace.cwd, &project).then_some(idx)
            })
    }

    pub(crate) fn launch_workspace_params_in_open_workspace(
        &mut self,
        params: &Value,
        target_idx: usize,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let preset = params
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or("even_h");
        let pane_specs = params
            .get("panes")
            .and_then(Value::as_array)
            .filter(|panes| !panes.is_empty())
            .ok_or_else(|| "add at least one pane before running".to_string())?;
        let Some(workspace) = self.workspaces.get(target_idx) else {
            return Err("open project workspace no longer exists".to_string());
        };
        if workspace.is_zoomed() {
            return Err("unzoom the project before running the template here".to_string());
        }
        if pane_specs.len() > MAX_PANES {
            return Err(format!("maximum pane count reached ({MAX_PANES})"));
        }

        let mut planned = pane_specs
            .iter()
            .enumerate()
            .map(|(idx, spec)| {
                parse_workspace_pane_plan(spec)
                    .map_err(|err| format!("pane {idx}: {}", err.message))
            })
            .collect::<Result<Vec<_>, _>>()?;
        dedupe_planned_pane_labels(&mut planned);

        let ws_id = self.workspaces[target_idx].id;
        let focus_idx = planned.iter().position(|plan| plan.focus).unwrap_or(0);
        let mut launches = Vec::with_capacity(planned.len());
        let mut panes = Vec::with_capacity(planned.len());
        for plan in planned {
            let env = plan.env.clone();
            let terminal = cx.new(|cx| {
                TerminalView::with_cwd_env_and_profile(
                    ws_id,
                    plan.cwd.clone(),
                    None,
                    env,
                    plan.profile,
                    cx,
                )
            });
            if let Some(label) = plan.label {
                terminal.update(cx, |view, _cx| {
                    view.terminal.custom_name = Some(label);
                });
            }
            let new_pane = self.create_pane(terminal.clone(), ws_id, cx);
            launches.push((
                terminal,
                plan.command.filter(|command| !command.is_empty()),
                plan.prompt.filter(|prompt| !prompt.is_empty()),
            ));
            panes.push(new_pane);
        }
        let focus_pane = panes.get(focus_idx).cloned();
        let tree = build_up_layout(preset, panes, focus_idx)
            .ok_or_else(|| "could not build layout from panes".to_string())?;
        if let Some(workspace) = self.workspaces.get_mut(target_idx) {
            workspace.active_tab_mut().root = Some(tree);
            workspace.active_tab_mut().saved_layout = None;
            if let Some(first_cwd) = launches
                .iter()
                .find_map(|(terminal, _, _)| terminal.read(cx).terminal.cwd_now())
            {
                workspace.cwd = first_cwd.display().to_string();
            }
        }

        if self.active_idx != target_idx {
            self.active_idx = target_idx;
            self.sync_files_sidebar_session(cx);
        }
        if let Some(pane) = focus_pane {
            self.pending_pane_focus = Some(pane);
        }
        for (pane_idx, (terminal, command, prompt)) in launches.into_iter().enumerate() {
            if let Some(command) = command {
                Self::schedule_launch_command(&terminal, command, prompt, pane_idx, cx);
            } else if let Some(prompt) = prompt {
                Self::schedule_prompt_prefill(&terminal, prompt, pane_idx, cx);
            }
        }
        self.save_session(cx);
        cx.notify();
        Ok(())
    }

    pub(crate) fn workspace_up_params(
        &self,
        commands: &[CommandDefinition],
        idx: usize,
    ) -> Result<Value, String> {
        let command = commands
            .get(idx)
            .ok_or_else(|| "selected workspace no longer exists".to_string())?;
        let workspace = command
            .workspace()
            .ok_or_else(|| "selected command is not a workspace".to_string())?;
        let project = workspace
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.trim().is_empty())
            .ok_or_else(|| "project path is required".to_string())?;
        let panes = template_surfaces(workspace);
        if panes.is_empty() {
            return Err("add at least one pane before running".to_string());
        }
        let visible_agents = TerminalAgent::visible(&self.cached_config);
        let mut pane_values = Vec::with_capacity(panes.len());
        for (pane_idx, pane) in panes.iter().enumerate() {
            let has_command = pane
                .command
                .as_deref()
                .is_some_and(|command| !command.trim().is_empty());
            let has_agent = pane
                .agent
                .as_deref()
                .is_some_and(|agent| !agent.trim().is_empty());
            if has_command && has_agent {
                return Err(format!("pane {} has both agent and command", pane_idx + 1));
            }

            let mut spec = serde_json::Map::new();
            spec.insert(
                "cwd".to_string(),
                Value::String(pane.cwd.clone().unwrap_or_else(|| project.to_string())),
            );
            if let Some(label) = pane
                .name
                .as_deref()
                .or(pane.custom_name.as_deref())
                .filter(|label| !label.trim().is_empty())
            {
                spec.insert("name".to_string(), Value::String(label.trim().to_string()));
            }
            if let Some(true) = pane.focus {
                spec.insert("focus".to_string(), Value::Bool(true));
            }
            if let Some(env) = pane.env.as_ref().filter(|env| !env.is_empty()) {
                spec.insert("env".to_string(), json!(env));
            }
            if let Some(agent_tag) = pane.agent.as_deref().filter(|tag| !tag.trim().is_empty()) {
                let Some(agent) = TerminalAgent::from_tag(agent_tag) else {
                    return Err(format!("pane {} uses an unknown agent", pane_idx + 1));
                };
                if !visible_agents.contains(&agent) {
                    return Err(format!(
                        "enable {} in Agents settings before running",
                        agent.display_name()
                    ));
                }
                spec.insert(
                    "command".to_string(),
                    Value::String(agent.launch_command(&self.cached_config)),
                );
                spec.insert("profile".to_string(), Value::String("agent".to_string()));
            } else if let Some(command) = pane.command.as_deref().filter(|c| !c.trim().is_empty()) {
                spec.insert(
                    "command".to_string(),
                    Value::String(command.trim().to_string()),
                );
            }
            if let Some(prompt) = pane
                .prompt
                .as_deref()
                .filter(|prompt| !prompt.trim().is_empty())
            {
                spec.insert("prompt".to_string(), Value::String(prompt.to_string()));
            }
            pane_values.push(Value::Object(spec));
        }

        Ok(json!({
            "name": workspace
                .name
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .unwrap_or(command.name.as_str()),
            "layout": workspace_layout_preset(workspace),
            "panes": pane_values,
        }))
    }
}

fn workspace_cwd_matches(cwd: &str, project: &std::path::Path) -> bool {
    canonicalize_workspace_cwd(cwd)
        .ok()
        .is_some_and(|cwd| paths_equal(&cwd, project))
}

fn paths_equal(left: &std::path::Path, right: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy().to_lowercase() == right.to_string_lossy().to_lowercase()
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}
