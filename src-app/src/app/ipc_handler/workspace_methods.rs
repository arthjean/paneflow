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

pub(crate) fn parse_workspace_pane_plan(
    spec: &serde_json::Value,
) -> Result<PlannedPane, JsonRpcError> {
    let command = opt_str(spec, "command")?.map(str::to_string);
    let prompt = opt_str(spec, "prompt")?.map(str::to_string);
    let env = opt_env(spec, "env")?;
    let profile = opt_profile(spec)?;
    let focus = opt_bool(spec, "focus")?.unwrap_or(false);
    let label = opt_label(spec)?;
    let cwd = opt_str(spec, "cwd")?.map(PathBuf::from);
    Ok(PlannedPane {
        cwd,
        command,
        prompt,
        env,
        profile,
        focus,
        label,
    })
}

pub(crate) struct WorkspaceUpRequest {
    name: String,
    preset: String,
    planned: Vec<PlannedPane>,
    managed_worktrees: Vec<crate::workspace::worktree::ManagedWorktree>,
    pane_worktrees: Vec<Option<String>>,
}

impl WorkspaceUpRequest {
    fn resolve_cwds(mut self) -> Result<Self, JsonRpcError> {
        for (i, pane) in self.planned.iter_mut().enumerate() {
            if let Some(raw) = pane.cwd.take() {
                let resolved =
                    canonicalize_workspace_cwd(&raw.to_string_lossy()).map_err(|err| {
                        JsonRpcError::invalid_params(format!("pane {i}: {}", err.message))
                    })?;
                pane.cwd = Some(resolved);
            }
        }
        Ok(self)
    }
}

pub(crate) fn workspace_up_request(
    params: &serde_json::Value,
) -> Result<WorkspaceUpRequest, JsonRpcError> {
    let name = opt_str(params, "name")?.unwrap_or("Workspace").to_string();
    let preset = opt_str(params, "layout")?.unwrap_or("even_h").to_string();
    let pane_specs = match params.get("panes").and_then(|p| p.as_array()) {
        Some(a) if !a.is_empty() => a,
        _ => {
            return Err(JsonRpcError::invalid_params(
                "`panes` must be a non-empty array",
            ));
        }
    };
    if pane_specs.len() > MAX_PANES {
        return Err(JsonRpcError::invalid_params(format!(
            "layout exceeds maximum pane count ({MAX_PANES})"
        )));
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
        let plan = parse_workspace_pane_plan(spec)
            .map_err(|err| JsonRpcError::invalid_params(format!("pane {i}: {}", err.message)))?;
        planned.push(plan);
    }
    if pane_specs.iter().any(pane_spec_requires_orchestration) && !ipc_orchestration_enabled() {
        return Err(orchestration_disabled_error("workspace.up"));
    }
    dedupe_planned_pane_labels(&mut planned);
    Ok(WorkspaceUpRequest {
        name,
        preset,
        planned,
        managed_worktrees,
        pane_worktrees,
    })
}

pub(crate) struct WorkspaceCreateRequest {
    name: String,
    cwd: Option<String>,
    layout: Option<LayoutNode>,
}

pub(crate) fn workspace_create_request(
    params: &serde_json::Value,
) -> Result<WorkspaceCreateRequest, JsonRpcError> {
    let name = opt_str(params, "name")?.unwrap_or("Terminal").to_string();
    let cwd = opt_str(params, "cwd")?.map(str::to_string);
    let layout = parse_layout_param(params)?;
    if layout.as_ref().is_some_and(layout_requires_orchestration) && !ipc_orchestration_enabled() {
        return Err(orchestration_disabled_error("workspace.create"));
    }
    Ok(WorkspaceCreateRequest { name, cwd, layout })
}

pub(crate) fn requested_focus(planned: &[PlannedPane]) -> Option<usize> {
    planned.iter().position(|p| p.focus)
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

pub(super) fn prompt_input(prompt: &str) -> std::borrow::Cow<'_, str> {
    if paneflow_ipc_client::send_text::text_contains_submit_byte(prompt) {
        std::borrow::Cow::Owned(paneflow_ipc_client::send_text::bracketed_paste_frame(
            prompt,
        ))
    } else {
        std::borrow::Cow::Borrowed(prompt)
    }
}

fn surplus_panes(tab: &crate::workspace::Tab, kept: usize) -> Vec<Entity<Pane>> {
    tab.saved_layout
        .as_ref()
        .or(tab.root.as_ref())
        .map(|tree| tree.collect_leaves().into_iter().skip(kept).collect())
        .unwrap_or_default()
}

fn index_out_of_bounds(index: usize, len: usize) -> JsonRpcError {
    JsonRpcError::invalid_params(format!("index {index} is out of bounds ({len} workspaces)"))
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
    pub(super) fn workspace_up_reply(
        &mut self,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> IpcReply {
        let request = match workspace_up_request(params) {
            Ok(request) => request,
            Err(error) => return IpcReply::Ready(error.into_value()),
        };
        IpcReply::Async(cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let request =
                match probe_off_thread(PATH_PROBE_TIMEOUT, move || request.resolve_cwds()).await {
                    None => return unresolved_path_error("a pane cwd").into_value(),
                    Some(Err(error)) => return error.into_value(),
                    Some(Ok(request)) => request,
                };
            this.update(cx, |app, cx| app.finish_workspace_up(request, cx))
                .unwrap_or_else(|_| app_shutting_down())
        }))
    }

    pub(super) fn workspace_create_reply(
        &mut self,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> IpcReply {
        let WorkspaceCreateRequest { name, cwd, layout } = match workspace_create_request(params) {
            Ok(request) => request,
            Err(error) => return IpcReply::Ready(error.into_value()),
        };
        let Some(raw) = cwd else {
            return IpcReply::Ready(self.finish_workspace_create(name, None, layout, cx));
        };
        IpcReply::Async(cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let cwd = match probe_off_thread(PATH_PROBE_TIMEOUT, move || {
                canonicalize_workspace_cwd(&raw)
            })
            .await
            {
                None => return unresolved_path_error("cwd").into_value(),
                Some(Err(error)) => return error.into_value(),
                Some(Ok(cwd)) => cwd,
            };
            this.update(cx, |app, cx| {
                app.finish_workspace_create(name, Some(cwd), layout, cx)
            })
            .unwrap_or_else(|_| app_shutting_down())
        }))
    }

    fn finish_workspace_create(
        &mut self,
        name: String,
        cwd: Option<PathBuf>,
        mut layout: Option<LayoutNode>,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        let ws_id = next_workspace_id();
        let ws = if layout.is_some() {
            Workspace::empty_with_cwd_and_id(
                ws_id,
                &name,
                cwd.unwrap_or_else(crate::launch_cwd::implicit_launch_cwd),
            )
        } else if let Some(dir) = cwd {
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, Some(dir.clone()), None, cx));
            let pane = self.create_pane(terminal, ws_id, cx);
            Workspace::with_cwd_and_id(ws_id, &name, dir, pane)
        } else {
            let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
            let pane = self.create_pane(terminal, ws_id, cx);
            Workspace::with_id(ws_id, &name, pane)
        };
        Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
        self.workspaces.push(ws);
        let idx = self.workspaces.len() - 1;

        let panes = if let Some(ref mut layout) = layout {
            let previous_idx = self.active_idx;
            self.active_idx = idx;
            if let Err(e) = self.apply_layout_from_json(layout, cx) {
                self.workspaces.remove(idx);
                self.active_idx = previous_idx.min(self.workspaces.len().saturating_sub(1));
                return JsonRpcError::invalid_params(format!("layout could not be applied: {e}"))
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

    fn finish_workspace_up(
        &mut self,
        request: WorkspaceUpRequest,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        let WorkspaceUpRequest {
            name,
            preset,
            planned,
            managed_worktrees,
            pane_worktrees,
        } = request;
        let preset = preset.as_str();

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

        let requested_focus = requested_focus(&planned);
        let focus_idx = requested_focus.unwrap_or(0);

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
        Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
        self.workspaces.push(ws);
        let idx = self.workspaces.len() - 1;
        self.activate_workspace_without_window(idx, cx);
        if let Some(pane) = requested_focus.and_then(|i| panes.get(i)) {
            self.pending_pane_focus = Some(pane.clone());
        }

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
                    t.read(cx).send_text(&prompt_input(&prompt));
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
                    t.read(cx).send_text(&prompt_input(&prompt));
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
                    let layout = ws.serialize_layout_without_scrollback(cx);
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
            "workspace.select" => {
                let idx = match required_index(params) {
                    Ok(idx) => idx,
                    Err(error) => return error.into_value(),
                };
                if idx < self.workspaces.len() {
                    self.activate_workspace_without_window(idx, cx);
                    serde_json::json!({"selected": idx})
                } else {
                    index_out_of_bounds(idx, self.workspaces.len()).into_value()
                }
            }
            "workspace.close" => {
                let idx = match required_index(params) {
                    Ok(idx) => idx,
                    Err(error) => return error.into_value(),
                };
                if self.workspaces.len() <= 1 {
                    return JsonRpcError::invalid_params("Cannot close the last workspace")
                        .into_value();
                }
                if idx >= self.workspaces.len() {
                    return index_out_of_bounds(idx, self.workspaces.len()).into_value();
                }
                match self
                    .close_without_prompt(crate::app::close_policy::CloseTarget::Workspace(idx), cx)
                {
                    Ok(()) => serde_json::json!({"closed": idx}),
                    Err(refusal) => {
                        JsonRpcError::confirmation_required(refusal.message()).into_value()
                    }
                }
            }
            "workspace.restore_layout" => {
                let mut layout = match parse_layout_param(params) {
                    Ok(Some(layout)) => layout,
                    Ok(None) => {
                        return JsonRpcError::invalid_params("missing 'layout' parameter")
                            .into_value();
                    }
                    Err(error) => return error.into_value(),
                };
                if layout_requires_orchestration(&layout) && !ipc_orchestration_enabled() {
                    return orchestration_disabled_error("workspace.restore_layout").into_value();
                }
                let needed = layout.leaf_count();
                if needed == 0 || needed > MAX_PANES {
                    return JsonRpcError::invalid_params(format!(
                        "layout must hold between 1 and {MAX_PANES} panes"
                    ))
                    .into_value();
                }
                let surplus = self
                    .active_workspace()
                    .map(|ws| surplus_panes(ws.active_tab(), needed))
                    .unwrap_or_default();
                for pane in &surplus {
                    let target = crate::app::close_policy::CloseTarget::Pane(pane.clone());
                    if let Some(refusal) = self.close_refusal(&target, cx) {
                        return JsonRpcError::confirmation_required(refusal.message()).into_value();
                    }
                }
                for pane in surplus {
                    self.perform_close(
                        crate::app::close_policy::CloseTarget::Pane(pane),
                        crate::app::close_policy::CloseIntent::Hold,
                        None,
                        cx,
                    );
                }
                match self.apply_layout_from_json(&mut layout, cx) {
                    Ok(()) => {
                        let panes = self.active_workspace().map_or(0, |ws| ws.pane_count());
                        serde_json::json!({"restored": true, "panes": panes})
                    }
                    Err(e) => JsonRpcError::invalid_params(e).into_value(),
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
    fn a_multiline_prompt_is_delivered_as_a_bracketed_paste() {
        assert_eq!(prompt_input("review the diff"), "review the diff");
        assert_eq!(
            prompt_input("line one\nline two\r\nthree"),
            "\u{1b}[200~line one\nline two\nthree\u{1b}[201~"
        );
    }

    #[test]
    fn the_focused_pane_is_the_one_the_plan_asks_for_whatever_the_preset() {
        let plan = |spec: serde_json::Value| parse_workspace_pane_plan(&spec).unwrap();
        let planned = vec![
            plan(serde_json::json!({})),
            plan(serde_json::json!({})),
            plan(serde_json::json!({"focus": true})),
        ];
        assert_eq!(requested_focus(&planned), Some(2));
        assert_eq!(
            requested_focus(&planned[..2]),
            None,
            "without a focus request no pane is pulled forward"
        );
    }

    #[test]
    fn pane_cwds_are_resolved_after_the_request_is_parsed() {
        let bogus = if cfg!(windows) {
            "C:\\paneflow-nonexistent-9d1f\\sub"
        } else {
            "/paneflow-nonexistent-9d1f/sub"
        };
        let request = workspace_up_request(&serde_json::json!({
            "panes": [{}, {"cwd": bogus}]
        }))
        .expect("parsing never touches the filesystem");
        let error = request
            .resolve_cwds()
            .err()
            .expect("a missing cwd is refused once resolved");
        assert_eq!(error.code, JsonRpcError::INVALID_PARAMS);
        assert!(error.message.starts_with("pane 1:"), "{}", error.message);

        let tmp = tempfile::tempdir().unwrap();
        let resolved = workspace_up_request(&serde_json::json!({
            "panes": [{"cwd": tmp.path().to_str().unwrap()}]
        }))
        .unwrap()
        .resolve_cwds()
        .expect("an existing folder resolves");
        assert!(
            resolved.planned[0]
                .cwd
                .as_ref()
                .is_some_and(|cwd| cwd.is_dir())
        );
    }

    #[test]
    fn a_workspace_create_request_defers_its_cwd() {
        let request = workspace_create_request(&serde_json::json!({
            "name": "api",
            "cwd": "/paneflow-nonexistent-9d1f"
        }))
        .expect("parsing never touches the filesystem");
        assert_eq!(request.cwd.as_deref(), Some("/paneflow-nonexistent-9d1f"));
        assert_eq!(request.name, "api");
    }

    #[test]
    fn a_mistyped_workspace_create_is_refused_before_the_orchestration_gate() {
        for params in [
            serde_json::json!({"name": 5, "layout": {"type": "pane", "surfaces": [{"env": {"A": "1"}}]}}),
            serde_json::json!({"cwd": ["/tmp"], "layout": {"type": "pane", "surfaces": [{"command": "make"}]}}),
        ] {
            let error = workspace_create_request(&params)
                .err()
                .unwrap_or_else(|| panic!("{params} must be refused"));
            assert_eq!(error.code, JsonRpcError::INVALID_PARAMS, "{params}");
        }
    }

    #[test]
    fn a_pane_plan_with_a_mistyped_field_is_refused() {
        for spec in [
            serde_json::json!({"env": {"PORT": 8080}}),
            serde_json::json!({"profile": "agnet"}),
            serde_json::json!({"command": 5}),
            serde_json::json!({"focus": "yes"}),
            serde_json::json!({"name": 3}),
        ] {
            let error = parse_workspace_pane_plan(&spec)
                .err()
                .unwrap_or_else(|| panic!("{spec} must be refused"));
            assert_eq!(error.code, JsonRpcError::INVALID_PARAMS, "{spec}");
        }
        let plan = parse_workspace_pane_plan(&serde_json::json!({
            "env": {"RUST_LOG": "info"},
            "profile": "agent",
            "name": "api"
        }))
        .expect("a typed plan is accepted");
        assert_eq!(plan.profile, TerminalSurfaceProfile::Agent);
        assert_eq!(plan.label.as_deref(), Some("api"));
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
