use super::*;

const UP_PREFILL_FLOOR: Duration = Duration::from_millis(1800);

const UP_PREFILL_MAX: Duration = Duration::from_millis(8000);

const UP_PREFILL_POLL: Duration = Duration::from_millis(200);

const UP_LAUNCH_FLOOR: Duration = Duration::from_millis(700);

const UP_LAUNCH_MAX: Duration = Duration::from_millis(4000);

const UP_LAUNCH_POLL: Duration = Duration::from_millis(100);

pub(crate) struct PlannedPane {
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) command: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) env: Option<HashMap<String, String>>,
    pub(crate) profile: TerminalSurfaceProfile,
    pub(crate) focus: bool,
    pub(crate) label: Option<String>,
}

pub(super) fn parse_env_object(
    value: Option<&serde_json::Value>,
) -> Option<HashMap<String, String>> {
    let obj = value?.as_object()?;
    let map: HashMap<String, String> = obj
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect();
    (!map.is_empty()).then_some(map)
}

pub(super) fn parse_terminal_profile(value: Option<&serde_json::Value>) -> TerminalSurfaceProfile {
    match value.and_then(|v| v.as_str()) {
        Some("agent") => TerminalSurfaceProfile::Agent,
        Some("cached") => TerminalSurfaceProfile::Cached,
        _ => TerminalSurfaceProfile::Normal,
    }
}

pub(crate) fn parse_workspace_pane_plan(
    spec: &serde_json::Value,
) -> Result<PlannedPane, JsonRpcError> {
    let cwd = spec
        .get("cwd")
        .and_then(|c| c.as_str())
        .map(canonicalize_workspace_cwd)
        .transpose()?;
    Ok(PlannedPane {
        cwd,
        command: spec
            .get("command")
            .and_then(|c| c.as_str())
            .map(str::to_string),
        prompt: spec
            .get("prompt")
            .and_then(|c| c.as_str())
            .map(str::to_string),
        env: parse_env_object(spec.get("env")),
        profile: parse_terminal_profile(spec.get("profile")),
        focus: spec.get("focus").and_then(|f| f.as_bool()).unwrap_or(false),
        label: spec
            .get("label")
            .or_else(|| spec.get("name"))
            .and_then(|v| v.as_str())
            .and_then(sanitize_pane_name),
    })
}

pub(crate) fn dedupe_planned_pane_labels(planned: &mut [PlannedPane]) {
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    for pp in planned {
        if let Some(label) = pp.label.take() {
            let unique = crate::workspace::surface_naming::claim_unique(&mut taken, &label);
            if unique != label {
                log::warn!("workspace.up: duplicate label '{label}' in batch, using '{unique}'");
            }
            pp.label = Some(unique);
        }
    }
}

pub(crate) fn build_up_layout(
    preset: &str,
    panes: Vec<Entity<Pane>>,
    focus_idx: usize,
) -> Option<LayoutTree> {
    match preset {
        "even_v" => LayoutTree::from_panes_equal(SplitDirection::Horizontal, panes),
        "main_vertical" => {
            let main = panes.get(focus_idx).or_else(|| panes.first())?.clone();
            let others: Vec<_> = panes.into_iter().filter(|p| *p != main).collect();
            LayoutTree::main_vertical(main, others)
        }
        "tiled" => LayoutTree::tiled(panes),
        _ => LayoutTree::from_panes_equal(SplitDirection::Vertical, panes),
    }
}

pub(crate) fn group_up_panes_by_worktree(
    worktrees: &[Option<String>],
) -> Vec<(Option<String>, Vec<usize>)> {
    let mut unbound: Vec<usize> = Vec::new();
    let mut bound: Vec<(String, Vec<usize>)> = Vec::new();
    for (idx, worktree) in worktrees.iter().enumerate() {
        match worktree {
            None => unbound.push(idx),
            Some(path) => match bound.iter_mut().find(|(known, _)| known == path) {
                Some((_, panes)) => panes.push(idx),
                None => bound.push((path.clone(), vec![idx])),
            },
        }
    }

    let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::with_capacity(bound.len() + 1);
    if !unbound.is_empty() {
        groups.push((None, unbound));
    }
    groups.extend(bound.into_iter().map(|(path, panes)| (Some(path), panes)));
    groups
}

pub(super) fn parse_managed_worktree(
    value: Option<&serde_json::Value>,
) -> Option<crate::workspace::worktree::ManagedWorktree> {
    let mw = value.filter(|v| !v.is_null())?;
    let path = mw.get("path").and_then(|p| p.as_str()).unwrap_or("");
    let repo_root = mw.get("repo_root").and_then(|p| p.as_str()).unwrap_or("");
    let branch = mw.get("branch").and_then(|b| b.as_str()).unwrap_or("");
    let teardown = mw.get("teardown").and_then(|t| t.as_str()).unwrap_or("");
    crate::workspace::worktree::managed_worktree_from_record(path, repo_root, branch, teardown)
}

pub(crate) fn canonicalize_workspace_cwd(raw: &str) -> Result<std::path::PathBuf, JsonRpcError> {
    let expanded = expand_tilde(raw);
    let canonical = std::fs::canonicalize(&expanded).map_err(|e| {
        JsonRpcError::invalid_params(format!("cwd does not exist or is unreadable: {raw} ({e})"))
    })?;
    let meta = std::fs::metadata(&canonical).map_err(|e| {
        JsonRpcError::invalid_params(format!("cwd metadata read failed for {raw}: {e}"))
    })?;
    if !meta.is_dir() {
        return Err(JsonRpcError::invalid_params(format!(
            "cwd is not a directory: {raw}"
        )));
    }
    let spawn_cwd = crate::runtime_paths::strip_verbatim_prefix(canonical.clone());
    log::info!(
        "ipc::workspace.create: canonical cwd resolved {raw:?} -> {canonical:?}; spawn cwd {spawn_cwd:?}"
    );
    Ok(spawn_cwd)
}

fn expand_tilde(raw: &str) -> PathBuf {
    expand_tilde_with_home(raw, dirs::home_dir().as_deref())
}

fn expand_tilde_with_home(raw: &str, home: Option<&std::path::Path>) -> PathBuf {
    match raw {
        "~" => home
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(raw)),
        _ => raw
            .strip_prefix("~/")
            .or_else(|| raw.strip_prefix("~\\"))
            .and_then(|rest| home.map(|home| home.join(rest)))
            .unwrap_or_else(|| PathBuf::from(raw)),
    }
}

pub(crate) fn parse_layout_param(
    params: &serde_json::Value,
) -> Result<Option<LayoutNode>, JsonRpcError> {
    let Some(raw) = params.get("layout") else {
        return Ok(None);
    };
    if raw.is_null() {
        return Ok(None);
    }
    serde_json::from_value::<LayoutNode>(raw.clone())
        .map(Some)
        .map_err(|e| JsonRpcError::invalid_params(format!("invalid layout: {e}")))
}

impl PaneFlowApp {
    pub(crate) fn handle_workspace_up(
        &mut self,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return JsonRpcError::invalid_params("Workspace limit reached").into_value();
        }
        let name = params
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("Workspace")
            .to_string();
        let preset = params
            .get("layout")
            .and_then(|l| l.as_str())
            .unwrap_or("even_h");
        let pane_specs = match params.get("panes").and_then(|p| p.as_array()) {
            Some(a) if !a.is_empty() => a,
            _ => {
                return JsonRpcError::invalid_params("`panes` must be a non-empty array")
                    .into_value();
            }
        };
        if pane_specs.len() > MAX_PANES {
            return JsonRpcError::invalid_params(format!(
                "layout exceeds maximum pane count ({MAX_PANES})"
            ))
            .into_value();
        }
        if pane_specs.iter().any(pane_spec_requires_orchestration) && !ipc_orchestration_enabled() {
            return orchestration_disabled_error("workspace.up").into_value();
        }

        let mut managed_worktrees: Vec<crate::workspace::worktree::ManagedWorktree> = Vec::new();
        let mut pane_worktrees: Vec<Option<String>> = Vec::with_capacity(pane_specs.len());
        let mut planned: Vec<PlannedPane> = Vec::with_capacity(pane_specs.len());
        for (i, spec) in pane_specs.iter().enumerate() {
            match parse_managed_worktree(spec.get("managed_worktree")) {
                Some(mw) => {
                    pane_worktrees.push(Some(mw.path.to_string_lossy().into_owned()));
                    managed_worktrees.push(mw);
                }
                None => pane_worktrees.push(None),
            }
            match parse_workspace_pane_plan(spec) {
                Ok(plan) => planned.push(plan),
                Err(err) => {
                    return JsonRpcError::invalid_params(format!("pane {i}: {}", err.message))
                        .into_value();
                }
            }
        }

        dedupe_planned_pane_labels(&mut planned);

        let labels: Vec<serde_json::Value> = planned
            .iter()
            .map(|p| {
                p.label
                    .clone()
                    .map_or(serde_json::Value::Null, serde_json::Value::String)
            })
            .collect();

        let ws_id = next_workspace_id();
        let mut panes: Vec<Entity<Pane>> = Vec::with_capacity(planned.len());
        let mut launches: Vec<(Entity<TerminalView>, Option<String>, Option<String>)> =
            Vec::with_capacity(planned.len());
        for pp in &planned {
            let env = pp.env.clone();
            let terminal = cx.new(|cx| {
                TerminalView::with_cwd_env_and_profile(
                    ws_id,
                    pp.cwd.clone(),
                    None,
                    env,
                    pp.profile,
                    cx,
                )
            });
            if let Some(label) = pp.label.clone() {
                terminal.update(cx, |view, _cx| {
                    view.terminal.custom_name = Some(label);
                });
            }
            let pane = self.create_pane(terminal.clone(), ws_id, cx);
            launches.push((terminal, pp.command.clone(), pp.prompt.clone()));
            panes.push(pane);
        }

        let focus_idx = planned.iter().position(|p| p.focus).unwrap_or(0);

        let groups = group_up_panes_by_worktree(&pane_worktrees);
        let mut tabs: Vec<crate::workspace::Tab> = Vec::with_capacity(groups.len());
        let mut active_tab = 0;
        for (tab_idx, (worktree, pane_idxs)) in groups.iter().enumerate() {
            if pane_idxs.contains(&focus_idx) {
                active_tab = tab_idx;
            }
            let group_panes: Vec<Entity<Pane>> =
                pane_idxs.iter().map(|i| panes[*i].clone()).collect();
            let local_focus = pane_idxs.iter().position(|i| *i == focus_idx).unwrap_or(0);
            let Some(tree) = build_up_layout(preset, group_panes, local_focus) else {
                return JsonRpcError::invalid_params("could not build layout from panes")
                    .into_value();
            };
            let title = worktree
                .as_ref()
                .and_then(|path| {
                    managed_worktrees
                        .iter()
                        .find(|mw| mw.path.to_string_lossy() == path.as_str())
                        .map(|mw| mw.branch.clone())
                })
                .unwrap_or_default();
            tabs.push(crate::workspace::Tab::restored(
                title,
                paneflow_config::schema::TabTitleSource::Preset,
                Some(tree),
                worktree.as_ref().map(std::path::PathBuf::from),
            ));
        }

        let ws_cwd = groups
            .iter()
            .find(|(worktree, _)| worktree.is_none())
            .and_then(|(_, pane_idxs)| pane_idxs.iter().find_map(|i| planned[*i].cwd.clone()))
            .or_else(|| planned.iter().find_map(|p| p.cwd.clone()))
            .unwrap_or_else(crate::launch_cwd::implicit_launch_cwd);
        let mut ws = Workspace::restored_with_id(ws_id, &name, ws_cwd, tabs, active_tab);
        ws.managed_worktrees = managed_worktrees;
        self.watch_git_dir(&ws);
        Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
        self.workspaces.push(ws);
        let idx = self.workspaces.len() - 1;
        self.activate_workspace_without_window(idx, cx);

        let mut surface_ids: Vec<u64> = Vec::with_capacity(launches.len());
        for (i, (terminal, command, prompt)) in launches.into_iter().enumerate() {
            surface_ids.push(terminal.entity_id().as_u64());
            if let Some(cmd) = command.filter(|c| !c.is_empty()) {
                Self::schedule_launch_command(&terminal, cmd, prompt, i, cx);
            } else if let Some(prompt) = prompt.filter(|p| !p.is_empty()) {
                Self::schedule_prompt_prefill(&terminal, prompt, i, cx);
            }
        }

        let panes_n = self.active_workspace().map_or(0, |ws| ws.pane_count());
        self.save_session(cx);
        cx.notify();
        serde_json::json!({
            "index": idx, "title": name, "panes": panes_n,
            "surface_ids": surface_ids, "labels": labels
        })
    }

    pub(crate) fn schedule_prompt_prefill(
        terminal: &Entity<TerminalView>,
        prompt: String,
        pane_label: usize,
        cx: &mut Context<Self>,
    ) {
        let weak = terminal.downgrade();
        cx.spawn(async move |_, cx: &mut gpui::AsyncApp| {
            let Some(settled) = Self::wait_for_terminal_settle(
                &weak,
                UP_PREFILL_FLOOR,
                UP_PREFILL_MAX,
                UP_PREFILL_POLL,
                cx,
            )
            .await
            else {
                return;
            };
            cx.update(|cx| {
                if let Some(t) = weak.upgrade() {
                    if !settled {
                        log::warn!(
                            "prompt prefill: pane {pane_label} still producing output after \
                             {UP_PREFILL_MAX:?}; prompt prefilled best-effort"
                        );
                    }
                    t.read(cx).send_text(&prompt);
                }
            });
        })
        .detach();
    }

    pub(crate) fn schedule_launch_command(
        terminal: &Entity<TerminalView>,
        command: String,
        prompt: Option<String>,
        pane_label: usize,
        cx: &mut Context<Self>,
    ) {
        let prompt = prompt.filter(|p| !p.is_empty());
        terminal.update(cx, |view, _cx| view.declare_agent_from_command(&command));
        let weak = terminal.downgrade();
        cx.spawn(async move |_, cx: &mut gpui::AsyncApp| {
            let Some(settled) = Self::wait_for_terminal_settle(
                &weak,
                UP_LAUNCH_FLOOR,
                UP_LAUNCH_MAX,
                UP_LAUNCH_POLL,
                cx,
            )
            .await
            else {
                return;
            };
            cx.update(|cx| {
                if let Some(t) = weak.upgrade() {
                    if !settled {
                        log::warn!(
                            "workspace launch: pane {pane_label} shell still producing output after \
                             {UP_LAUNCH_MAX:?}; launch command sent best-effort"
                        );
                    }
                    t.read(cx).send_command(&command);
                }
            });

            let Some(prompt) = prompt else {
                return;
            };
            let Some(settled) = Self::wait_for_terminal_settle(
                &weak,
                UP_PREFILL_FLOOR,
                UP_PREFILL_MAX,
                UP_PREFILL_POLL,
                cx,
            )
            .await
            else {
                return;
            };
            cx.update(|cx| {
                if let Some(t) = weak.upgrade() {
                    if !settled {
                        log::warn!(
                            "prompt prefill: pane {pane_label} still producing output after \
                             {UP_PREFILL_MAX:?}; prompt prefilled best-effort"
                        );
                    }
                    t.read(cx).send_text(&prompt);
                }
            });
        })
        .detach();
    }

    async fn wait_for_terminal_settle(
        weak: &gpui::WeakEntity<TerminalView>,
        floor: Duration,
        max: Duration,
        poll: Duration,
        cx: &mut gpui::AsyncApp,
    ) -> Option<bool> {
        smol::Timer::after(floor).await;
        let gen_now = |cx: &mut gpui::AsyncApp| -> Option<u64> {
            cx.update(|cx| {
                weak.upgrade()
                    .map(|t| t.read(cx).terminal.output_generation)
            })
        };
        let mut last = gen_now(cx)?;
        let mut waited = floor;
        while waited < max {
            smol::Timer::after(poll).await;
            waited += poll;
            let now = gen_now(cx)?;
            if now == last {
                return Some(true);
            }
            last = now;
        }
        Some(false)
    }

    pub(super) fn handle_workspace_method(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        match method {
            "workspace.list" => {
                let list: Vec<_> = self
                    .workspaces
                    .iter()
                    .enumerate()
                    .map(|(i, ws)| {
                        serde_json::json!({
                            "index": i,
                            "title": ws.title,
                            "cwd": ws.cwd,
                            "panes": ws.pane_count(),
                            "active": i == self.active_idx,
                        })
                    })
                    .collect();
                serde_json::json!({"workspaces": list})
            }
            "workspace.current" => {
                if let Some(ws) = self.active_workspace() {
                    let layout = ws.serialize_layout(cx);
                    serde_json::json!({
                        "index": self.active_idx,
                        "title": ws.title,
                        "cwd": ws.cwd,
                        "panes": ws.pane_count(),
                        "layout": layout.and_then(|l| serde_json::to_value(l).ok()),
                    })
                } else {
                    serde_json::json!(null)
                }
            }
            "workspace.create" => {
                if self.workspaces.len() >= MAX_WORKSPACES {
                    return serde_json::json!({"error": "Workspace limit reached"});
                }
                let mut layout = match parse_layout_param(params) {
                    Ok(l) => l,
                    Err(e) => return e.into_value(),
                };
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("Terminal");
                let cwd = match params.get("cwd").and_then(|c| c.as_str()) {
                    Some(raw) => match canonicalize_workspace_cwd(raw) {
                        Ok(canonical) => Some(canonical),
                        Err(err) => return err.into_value(),
                    },
                    None => None,
                };
                let ws_id = next_workspace_id();
                let ws = if let Some(dir) = cwd {
                    let terminal =
                        cx.new(|cx| TerminalView::with_cwd(ws_id, Some(dir.clone()), None, cx));
                    let pane = self.create_pane(terminal, ws_id, cx);
                    Workspace::with_cwd_and_id(ws_id, name, dir, pane)
                } else {
                    let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
                    let pane = self.create_pane(terminal, ws_id, cx);
                    Workspace::with_id(ws_id, name, pane)
                };
                self.watch_git_dir(&ws);
                Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
                self.workspaces.push(ws);
                let idx = self.workspaces.len() - 1;

                let panes = if let Some(ref mut layout) = layout {
                    let previous_idx = self.active_idx;
                    self.active_idx = idx;
                    if let Err(e) = self.apply_layout_from_json(layout, cx) {
                        if let Some(dir) = self.workspaces[idx].git_dir.clone() {
                            self.unwatch_git_dir(&dir);
                        }
                        self.workspaces.remove(idx);
                        self.active_idx = previous_idx.min(self.workspaces.len().saturating_sub(1));
                        return JsonRpcError::invalid_params(format!(
                            "layout could not be applied: {e}"
                        ))
                        .into_value();
                    }
                    self.active_workspace().map_or(1, |ws| ws.pane_count())
                } else {
                    1
                };

                self.save_session(cx);
                cx.notify();
                serde_json::json!({"index": idx, "title": name, "panes": panes})
            }
            "workspace.up" => self.handle_workspace_up(params, cx),
            "workspace.select" => {
                let idx = params.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                if idx < self.workspaces.len() {
                    self.activate_workspace_without_window(idx, cx);
                    serde_json::json!({"selected": idx})
                } else {
                    serde_json::json!({"error": "Index out of bounds"})
                }
            }
            "workspace.close" => {
                if self.workspaces.len() <= 1 {
                    serde_json::json!({"error": "Cannot close last workspace"})
                } else {
                    let idx = params
                        .get("index")
                        .and_then(|i| i.as_u64())
                        .map(|i| i as usize)
                        .unwrap_or(self.active_idx);
                    if idx < self.workspaces.len() {
                        if let Some(dir) = self.workspaces[idx].git_dir.clone() {
                            self.unwatch_git_dir(&dir);
                        }
                        let worktrees = std::mem::take(&mut self.workspaces[idx].managed_worktrees);
                        self.spawn_worktree_teardown(worktrees, cx);
                        self.workspaces.remove(idx);
                        if self.active_idx >= self.workspaces.len() {
                            self.active_idx = self.workspaces.len() - 1;
                        }
                        self.save_session(cx);
                        cx.notify();
                        serde_json::json!({"closed": idx})
                    } else {
                        serde_json::json!({"error": "Index out of bounds"})
                    }
                }
            }
            "workspace.restore_layout" => {
                let Some(layout_value) = params.get("layout") else {
                    return serde_json::json!({"error": "Missing 'layout' parameter"});
                };
                let mut layout: LayoutNode = match serde_json::from_value(layout_value.clone()) {
                    Ok(l) => l,
                    Err(e) => {
                        return serde_json::json!({"error": format!("Invalid layout JSON: {e}")});
                    }
                };
                match self.apply_layout_from_json(&mut layout, cx) {
                    Ok(()) => {
                        let panes = self.active_workspace().map_or(0, |ws| ws.pane_count());
                        serde_json::json!({"restored": true, "panes": panes})
                    }
                    Err(e) => serde_json::json!({"error": e}),
                }
            }
            _ => JsonRpcError::method_not_found(format!("Method not found: {method}")).into_value(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_without_worktrees_is_the_single_tab_it_has_always_been() {
        let groups = group_up_panes_by_worktree(&[None, None, None]);
        assert_eq!(groups, vec![(None, vec![0, 1, 2])]);
        assert_eq!(group_up_panes_by_worktree(&[]), vec![]);
    }

    #[test]
    fn each_worktree_gets_its_own_tab_in_declaration_order() {
        let wt = |p: &str| Some(p.to_string());
        let groups = group_up_panes_by_worktree(&[
            wt("/r.worktrees/b"),
            None,
            wt("/r.worktrees/a"),
            wt("/r.worktrees/b"),
        ]);
        assert_eq!(
            groups,
            vec![
                (None, vec![1]),
                (wt("/r.worktrees/b"), vec![0, 3]),
                (wt("/r.worktrees/a"), vec![2]),
            ],
            "tab order follows first appearance, not path order"
        );
    }

    #[test]
    fn an_all_worktree_batch_opens_no_empty_main_tab() {
        let wt = |p: &str| Some(p.to_string());
        let groups = group_up_panes_by_worktree(&[wt("/r.worktrees/a"), wt("/r.worktrees/b")]);
        assert_eq!(
            groups,
            vec![
                (wt("/r.worktrees/a"), vec![0]),
                (wt("/r.worktrees/b"), vec![1])
            ],
            "no pane in the main checkout means no tab for it"
        );
    }

    #[test]
    fn parse_env_object_keeps_strings_and_drops_the_rest() {
        let env = parse_env_object(Some(&serde_json::json!({
            "RUST_LOG": "info",
            "PORT": 8080,
            "FLAG": true
        })))
        .expect("non-empty string map");
        assert_eq!(env.get("RUST_LOG").map(String::as_str), Some("info"));
        assert!(
            !env.contains_key("PORT"),
            "non-string value must be dropped"
        );
        assert!(!env.contains_key("FLAG"));
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn parse_env_object_absent_or_empty_is_none() {
        assert!(parse_env_object(None).is_none());
        assert!(parse_env_object(Some(&serde_json::json!({}))).is_none());
        assert!(parse_env_object(Some(&serde_json::json!({ "N": 1 }))).is_none());
    }

    #[test]
    fn parse_layout_param_absent_returns_none() {
        let params = serde_json::json!({"name": "ws"});
        assert!(parse_layout_param(&params).expect("ok").is_none());
    }

    #[test]
    fn parse_layout_param_null_returns_none() {
        let params = serde_json::json!({"layout": null});
        assert!(parse_layout_param(&params).expect("ok").is_none());
    }

    #[test]
    fn parse_layout_param_valid_pane_returns_some() {
        let params = serde_json::json!({
            "layout": { "type": "pane", "surfaces": [] }
        });
        let layout = parse_layout_param(&params).expect("ok").expect("some");
        assert_eq!(layout.leaf_count(), 1);
    }

    #[test]
    fn parse_layout_param_valid_split_returns_some() {
        let params = serde_json::json!({
            "layout": {
                "type": "split",
                "direction": "vertical",
                "ratios": [0.5, 0.5],
                "children": [
                    { "type": "pane", "surfaces": [] },
                    { "type": "pane", "surfaces": [] }
                ]
            }
        });
        let layout = parse_layout_param(&params).expect("ok").expect("some");
        assert_eq!(layout.leaf_count(), 2);
    }

    #[test]
    fn parse_layout_param_string_payload_returns_invalid_params() {
        let params = serde_json::json!({"layout": "not an object"});
        let err = parse_layout_param(&params).expect_err("err");
        assert_eq!(err.code, JsonRpcError::INVALID_PARAMS);
        assert!(
            err.message.starts_with("invalid layout:"),
            "got {:?}",
            err.message
        );
    }

    #[test]
    fn parse_layout_param_unknown_tag_returns_invalid_params() {
        let params = serde_json::json!({"layout": { "type": "unknown_kind" }});
        let err = parse_layout_param(&params).expect_err("err");
        assert_eq!(err.code, JsonRpcError::INVALID_PARAMS);
    }

    #[test]
    fn workspace_create_rejects_nonexistent_cwd() {
        let bogus = "/nonexistent/path/paneflow-us-014-fixture-xyz";
        assert!(
            !std::path::Path::new(bogus).exists(),
            "fixture precondition: path must not exist"
        );
        let err = super::canonicalize_workspace_cwd(bogus).expect_err("must reject missing cwd");
        assert_eq!(err.code, JsonRpcError::INVALID_PARAMS);
        assert!(
            err.message.contains("does not exist"),
            "error must mention non-existence, got: {}",
            err.message
        );
    }

    #[test]
    fn workspace_create_rejects_file_cwd() {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let path = tmp.path().to_string_lossy().into_owned();
        let err =
            super::canonicalize_workspace_cwd(&path).expect_err("must reject regular-file cwd");
        assert_eq!(err.code, JsonRpcError::INVALID_PARAMS);
        assert!(
            err.message.contains("not a directory"),
            "error must mention not-a-directory, got: {}",
            err.message
        );
    }

    #[test]
    fn workspace_create_accepts_existing_directory() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let resolved = super::canonicalize_workspace_cwd(tmp.path().to_str().expect("utf-8 path"))
            .expect("real dir must canonicalize");
        assert!(resolved.is_absolute());
        assert!(resolved.is_dir());
    }

    #[test]
    fn workspace_cwd_expands_home_prefix_before_canonicalize() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\Arthur"
        } else {
            "/home/arthur"
        });

        assert_eq!(
            super::expand_tilde_with_home("~", Some(&home)),
            home.clone()
        );
        assert_eq!(
            super::expand_tilde_with_home("~/dev/backend", Some(&home)),
            home.join("dev/backend")
        );
        assert_eq!(
            super::expand_tilde_with_home("~\\dev\\backend", Some(&home)),
            home.join("dev\\backend")
        );
        assert_eq!(
            super::expand_tilde_with_home("rel/~not-home", Some(&home)),
            PathBuf::from("rel/~not-home")
        );
    }

    #[cfg(windows)]
    #[test]
    fn workspace_create_returns_cmd_safe_windows_cwd() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let resolved = super::canonicalize_workspace_cwd(tmp.path().to_str().expect("utf-8 path"))
            .expect("real dir must canonicalize");
        assert!(
            !resolved.to_string_lossy().starts_with(r"\\?\"),
            "workspace cwd must be safe for cmd.exe spawn, got: {resolved:?}"
        );
        assert!(resolved.is_dir());
    }

    #[test]
    fn workspace_up_dedups_duplicate_labels_in_batch() {
        use crate::workspace::surface_naming::claim_unique;
        use std::collections::HashSet;
        let mut taken: HashSet<String> = HashSet::new();
        let resolved: Vec<String> = ["logs", "api", "logs", "logs"]
            .iter()
            .map(|l| claim_unique(&mut taken, l))
            .collect();
        assert_eq!(resolved, vec!["logs", "api", "logs-2", "logs-3"]);
        assert_eq!(
            super::sanitize_pane_name("  reviewer  ").as_deref(),
            Some("reviewer")
        );
        assert_eq!(super::sanitize_pane_name("   "), None);
    }
}
