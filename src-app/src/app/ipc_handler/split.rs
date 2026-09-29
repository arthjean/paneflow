use std::path::Path;

use super::*;

pub(super) const PATH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) async fn probe_off_thread<T: Send + 'static>(
    timeout: Duration,
    probe: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let work = smol::unblock(probe);
    smol::future::or(async { Some(work.await) }, async {
        smol::Timer::after(timeout).await;
        None
    })
    .await
}

pub(super) fn unresolved_path_error(what: &str) -> JsonRpcError {
    JsonRpcError::invalid_params(format!(
        "{what} did not resolve within {} s",
        PATH_PROBE_TIMEOUT.as_secs()
    ))
}

pub(super) struct SplitSpec<'a> {
    pub(super) direction: &'a str,
    pub(super) surface_id: Option<u64>,
    pub(super) cwd: Option<&'a str>,
    pub(super) command: Option<&'a str>,
    pub(super) prompt: Option<&'a str>,
    pub(super) env: Option<HashMap<String, String>>,
    pub(super) profile: TerminalSurfaceProfile,
    pub(super) label: Option<String>,
}

pub(super) fn split_spec(params: &serde_json::Value) -> Result<SplitSpec<'_>, JsonRpcError> {
    Ok(SplitSpec {
        direction: opt_str(params, "direction")?.unwrap_or(""),
        surface_id: opt_u64(params, "surface_id")?,
        cwd: opt_str(params, "cwd")?,
        command: opt_str(params, "command")?,
        prompt: opt_str(params, "prompt")?,
        env: opt_env(params, "env")?,
        profile: opt_profile(params)?,
        label: opt_label(params)?,
    })
}

#[derive(Clone, Debug)]
pub(super) struct SplitProbe {
    pub(super) cwd: Option<String>,
    pub(super) worktree: Option<PathBuf>,
    pub(super) anchor_tab: u64,
    pub(super) tabs: Vec<(u64, Option<PathBuf>)>,
    pub(super) fallback_cwd: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SplitTarget {
    Tab(u64),
    NewTab(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SplitDecision {
    pub(super) target: SplitTarget,
    pub(super) cwd: Option<PathBuf>,
}

fn canonical_dir(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .map(crate::runtime_paths::strip_verbatim_prefix)
}

fn outside_worktree(cwd: &Path, worktree: &Path) -> JsonRpcError {
    JsonRpcError::invalid_params(format!(
        "cwd {} is outside the worktree {} bound to this tab",
        cwd.display(),
        worktree.display()
    ))
}

impl SplitProbe {
    fn anchor_worktree(&self) -> Option<&PathBuf> {
        self.tabs
            .iter()
            .find(|(id, _)| *id == self.anchor_tab)
            .and_then(|(_, worktree)| worktree.as_ref())
    }

    pub(super) fn decide(&self) -> Result<SplitDecision, JsonRpcError> {
        let explicit = self
            .cwd
            .as_deref()
            .map(canonicalize_workspace_cwd)
            .transpose()?;
        if let Some(worktree) = &self.worktree {
            let root = canonical_dir(worktree).ok_or_else(|| {
                JsonRpcError::invalid_params(format!(
                    "managed_worktree path {} does not exist",
                    worktree.display()
                ))
            })?;
            if let Some(cwd) = &explicit
                && !cwd.starts_with(&root)
            {
                return Err(outside_worktree(cwd, worktree));
            }
            let target = self
                .tabs
                .iter()
                .find(|(_, bound)| bound.as_deref().and_then(canonical_dir).as_ref() == Some(&root))
                .map_or_else(
                    || SplitTarget::NewTab(worktree.clone()),
                    |(id, _)| SplitTarget::Tab(*id),
                );
            return Ok(SplitDecision {
                target,
                cwd: Some(explicit.unwrap_or_else(|| worktree.clone())),
            });
        }
        let anchor = SplitTarget::Tab(self.anchor_tab);
        let Some(worktree) = self.anchor_worktree() else {
            return Ok(SplitDecision {
                target: anchor,
                cwd: explicit.or_else(|| self.fallback_cwd.clone()),
            });
        };
        let root = canonical_dir(worktree);
        let inside = |cwd: &Path| root.as_ref().is_some_and(|root| cwd.starts_with(root));
        match explicit {
            Some(cwd) if inside(&cwd) => Ok(SplitDecision {
                target: anchor,
                cwd: Some(cwd),
            }),
            Some(cwd) => Err(outside_worktree(&cwd, worktree)),
            None => {
                let fallback = self
                    .fallback_cwd
                    .as_deref()
                    .filter(|cwd| canonical_dir(cwd).is_some_and(|cwd| inside(&cwd)))
                    .map(Path::to_path_buf);
                Ok(SplitDecision {
                    target: anchor,
                    cwd: Some(fallback.unwrap_or_else(|| worktree.clone())),
                })
            }
        }
    }

    pub(super) fn decide_without_probing(&self) -> Result<SplitDecision, JsonRpcError> {
        if self.cwd.is_some() {
            return Err(unresolved_path_error("cwd"));
        }
        if self.worktree.is_some() {
            return Err(unresolved_path_error("managed_worktree"));
        }
        Ok(SplitDecision {
            target: SplitTarget::Tab(self.anchor_tab),
            cwd: self
                .anchor_worktree()
                .cloned()
                .or_else(|| self.fallback_cwd.clone()),
        })
    }
}

pub(super) async fn resolve_split(probe: SplitProbe) -> Result<SplitDecision, JsonRpcError> {
    let fallback = probe.clone();
    probe_off_thread(PATH_PROBE_TIMEOUT, move || probe.decide())
        .await
        .unwrap_or_else(|| fallback.decide_without_probing())
}

fn split_target_tab_idx(
    ws: &Workspace,
    tab_id: u64,
    anchor: Option<&Entity<Pane>>,
) -> Result<usize, JsonRpcError> {
    let tab_idx = ws
        .tabs()
        .iter()
        .position(|tab| tab.id == tab_id)
        .ok_or_else(|| JsonRpcError::invalid_params("the target tab was closed"))?;
    let tab = &ws.tabs()[tab_idx];
    if tab.is_zoomed() {
        return Err(JsonRpcError::invalid_params(
            "Unzoom before splitting panes",
        ));
    }
    let Some(root) = tab.root.as_ref() else {
        return Err(JsonRpcError::invalid_params("the target tab has no pane"));
    };
    if !tab.can_add_pane() {
        return Err(JsonRpcError::invalid_params(format!(
            "Maximum pane count reached ({MAX_PANES})"
        )));
    }
    if let Some(anchor) = anchor
        && !root.contains_leaf(anchor)
    {
        return Err(JsonRpcError::invalid_params("Surface not found"));
    }
    Ok(tab_idx)
}

pub(super) fn check_split_target(
    ws: &Workspace,
    target: &SplitTarget,
    anchor: Option<&Entity<Pane>>,
) -> Result<(), JsonRpcError> {
    match target {
        SplitTarget::Tab(tab_id) => split_target_tab_idx(ws, *tab_id, anchor).map(|_| ()),
        SplitTarget::NewTab(_) if ws.can_open_tab() => Ok(()),
        SplitTarget::NewTab(_) => Err(JsonRpcError::invalid_params(
            "Tab limit reached for this workspace",
        )),
    }
}

pub(super) fn place_split(
    ws: &mut Workspace,
    target: SplitTarget,
    anchor: Option<&Entity<Pane>>,
    direction: SplitDirection,
    new_pane: Entity<Pane>,
    new_tab_title: String,
) -> Result<(), JsonRpcError> {
    check_split_target(ws, &target, anchor)?;
    match target {
        SplitTarget::Tab(tab_id) => {
            let tab_idx = split_target_tab_idx(ws, tab_id, anchor)?;
            let root = ws
                .tab_mut(tab_idx)
                .and_then(|tab| tab.root.as_mut())
                .ok_or_else(|| JsonRpcError::invalid_params("the target tab has no pane"))?;
            match anchor {
                Some(anchor) => {
                    if !root.split_at_pane(anchor, direction, new_pane) {
                        return Err(JsonRpcError::invalid_params("Surface not found"));
                    }
                }
                None => root.split_first_leaf(direction, new_pane),
            }
            Ok(())
        }
        SplitTarget::NewTab(worktree) => {
            let opened = ws.open_tab(crate::workspace::Tab::restored(
                new_tab_title,
                paneflow_config::schema::TabTitleSource::Preset,
                Some(LayoutTree::Leaf(new_pane)),
                Some(worktree),
            ));
            if opened {
                Ok(())
            } else {
                Err(JsonRpcError::invalid_params(
                    "Tab limit reached for this workspace",
                ))
            }
        }
    }
}

struct SplitAnchor {
    workspace_id: u64,
    tab_id: u64,
    pane: Option<Entity<Pane>>,
}

struct SplitLaunch {
    direction: SplitDirection,
    direction_name: String,
    env: Option<HashMap<String, String>>,
    command: Option<String>,
    prompt: Option<String>,
    profile: TerminalSurfaceProfile,
    label: Option<String>,
    managed_worktree: Option<crate::workspace::worktree::ManagedWorktree>,
}

impl PaneFlowApp {
    fn split_anchor(&self, surface_id: Option<u64>, cx: &App) -> Result<SplitAnchor, JsonRpcError> {
        if let Some(sid) = surface_id {
            let loc = find_pane_by_surface_id(&self.workspaces, sid, cx)
                .ok_or_else(|| JsonRpcError::invalid_params("Surface not found"))?;
            let ws = &self.workspaces[loc.workspace_idx];
            return Ok(SplitAnchor {
                workspace_id: ws.id,
                tab_id: ws.tabs()[loc.tab_idx].id,
                pane: Some(loc.pane),
            });
        }
        let ws = self
            .active_workspace()
            .ok_or_else(|| JsonRpcError::invalid_params("No active workspace"))?;
        Ok(SplitAnchor {
            workspace_id: ws.id,
            tab_id: ws.active_tab().id,
            pane: None,
        })
    }

    pub(super) fn surface_split_reply(
        &mut self,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> IpcReply {
        match self.prepare_split(params, cx) {
            Ok(task) => IpcReply::Async(task),
            Err(error) => IpcReply::Ready(error.into_value()),
        }
    }

    fn prepare_split(
        &mut self,
        params: &serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Result<gpui::Task<serde_json::Value>, JsonRpcError> {
        let spec = split_spec(params)?;
        let direction = match spec.direction {
            "horizontal" => SplitDirection::Horizontal,
            "vertical" => SplitDirection::Vertical,
            _ => {
                return Err(JsonRpcError::invalid_params(
                    "Missing or invalid 'direction' parameter (use \"horizontal\" or \"vertical\")",
                ));
            }
        };
        if pane_spec_requires_orchestration(params) && !ipc_orchestration_enabled() {
            return Err(orchestration_disabled_error("surface.split"));
        }
        let managed_worktree = parse_managed_worktree(params.get("managed_worktree"));
        let anchor = self.split_anchor(spec.surface_id, cx)?;
        let ws = self
            .workspaces
            .iter()
            .find(|ws| ws.id == anchor.workspace_id)
            .ok_or_else(|| JsonRpcError::invalid_params("No active workspace"))?;
        if managed_worktree.is_none() {
            check_split_target(ws, &SplitTarget::Tab(anchor.tab_id), anchor.pane.as_ref())?;
        }
        let probe = SplitProbe {
            cwd: spec.cwd.map(str::to_owned),
            worktree: managed_worktree.as_ref().map(|mw| mw.path.clone()),
            anchor_tab: anchor.tab_id,
            tabs: ws
                .tabs()
                .iter()
                .map(|tab| (tab.id, tab.worktree.clone()))
                .collect(),
            fallback_cwd: (!ws.cwd.is_empty()).then(|| PathBuf::from(&ws.cwd)),
        };
        let launch = SplitLaunch {
            direction,
            direction_name: spec.direction.to_owned(),
            env: spec.env,
            command: spec.command.filter(|c| !c.is_empty()).map(str::to_owned),
            prompt: spec.prompt.filter(|p| !p.is_empty()).map(str::to_owned),
            profile: spec.profile,
            label: spec.label,
            managed_worktree,
        };
        Ok(cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let decision = match resolve_split(probe).await {
                Ok(decision) => decision,
                Err(error) => return error.into_value(),
            };
            this.update(cx, |app, cx| app.finish_split(anchor, decision, launch, cx))
                .unwrap_or_else(|_| app_shutting_down())
        }))
    }

    fn finish_split(
        &mut self,
        anchor: SplitAnchor,
        decision: SplitDecision,
        launch: SplitLaunch,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        let Some(ws_idx) = self
            .workspaces
            .iter()
            .position(|ws| ws.id == anchor.workspace_id)
        else {
            return JsonRpcError::invalid_params("the workspace was closed").into_value();
        };
        let anchor_pane = match &decision.target {
            SplitTarget::Tab(tab_id) if *tab_id == anchor.tab_id => anchor.pane.as_ref(),
            _ => None,
        };
        if let Err(error) =
            check_split_target(&self.workspaces[ws_idx], &decision.target, anchor_pane)
        {
            return error.into_value();
        }
        let ws_id = anchor.workspace_id;
        let new_terminal = cx.new(|cx| {
            TerminalView::with_cwd_env_and_profile(
                ws_id,
                decision.cwd.clone(),
                None,
                launch.env.clone(),
                launch.profile,
                cx,
            )
        });
        if let Some(name) = launch.label {
            new_terminal.update(cx, |view, _cx| {
                view.terminal.custom_name = Some(name);
            });
        }
        let surface_id = new_terminal.entity_id().as_u64();
        let new_pane = self.create_pane(new_terminal.clone(), ws_id, cx);
        let title = launch
            .managed_worktree
            .as_ref()
            .map(|mw| mw.branch.clone())
            .unwrap_or_default();
        if let Err(error) = place_split(
            &mut self.workspaces[ws_idx],
            decision.target,
            anchor_pane,
            launch.direction,
            new_pane,
            title,
        ) {
            return error.into_value();
        }
        if let Some(mw) = launch.managed_worktree {
            self.workspaces[ws_idx].managed_worktrees.push(mw);
        }
        if let Some(cmd) = launch.command {
            Self::schedule_launch_command(&new_terminal, cmd, launch.prompt, usize::MAX, cx);
        } else if let Some(prompt) = launch.prompt {
            Self::schedule_prompt_prefill(&new_terminal, prompt, usize::MAX, cx);
        }
        let panes = self.workspaces[ws_idx].pane_count();
        self.save_session(cx);
        cx.notify();
        serde_json::json!({
            "split": true, "direction": launch.direction_name, "panes": panes,
            "surface_id": surface_id
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(&path).expect("create fixture dir");
        path
    }

    fn canonical(path: &Path) -> PathBuf {
        canonical_dir(path).expect("fixture dir resolves")
    }

    #[test]
    fn a_split_spec_with_a_mistyped_field_is_refused() {
        for params in [
            serde_json::json!({"direction": 1}),
            serde_json::json!({"direction": "vertical", "surface_id": "1"}),
            serde_json::json!({"direction": "vertical", "env": {"A": 1}}),
            serde_json::json!({"direction": "vertical", "env": "A=1"}),
            serde_json::json!({"direction": "vertical", "profile": "shell"}),
            serde_json::json!({"direction": "vertical", "name": 7}),
            serde_json::json!({"direction": "vertical", "cwd": 7}),
        ] {
            assert_eq!(
                split_spec(&params).err().map(|error| error.code),
                Some(JsonRpcError::INVALID_PARAMS),
                "{params}"
            );
        }
    }

    #[test]
    fn an_explicit_cwd_inside_the_bound_worktree_is_honored() {
        let root = tempfile::tempdir().expect("tempdir");
        let worktree = dir(root.path(), "wt-a");
        let nested = dir(&worktree, "crates");
        let probe = SplitProbe {
            cwd: Some(nested.to_string_lossy().into_owned()),
            worktree: None,
            anchor_tab: 1,
            tabs: vec![(1, Some(worktree.clone()))],
            fallback_cwd: None,
        };

        let decision = probe.decide().expect("inside the worktree");

        assert_eq!(decision.target, SplitTarget::Tab(1));
        assert_eq!(decision.cwd, Some(canonical(&nested)));
    }

    #[test]
    fn an_explicit_cwd_outside_the_bound_worktree_is_refused() {
        let root = tempfile::tempdir().expect("tempdir");
        let worktree = dir(root.path(), "wt-a");
        let elsewhere = dir(root.path(), "main");
        let probe = SplitProbe {
            cwd: Some(elsewhere.to_string_lossy().into_owned()),
            worktree: None,
            anchor_tab: 1,
            tabs: vec![(1, Some(worktree))],
            fallback_cwd: None,
        };

        let error = probe.decide().expect_err("outside the worktree");

        assert_eq!(error.code, JsonRpcError::INVALID_PARAMS);
        assert!(
            error.message.contains("outside the worktree"),
            "{}",
            error.message
        );
    }

    #[test]
    fn without_a_cwd_the_workspace_folder_is_confined_to_the_worktree() {
        let root = tempfile::tempdir().expect("tempdir");
        let worktree = dir(root.path(), "wt-a");
        let main = dir(root.path(), "main");
        let probe = SplitProbe {
            cwd: None,
            worktree: None,
            anchor_tab: 1,
            tabs: vec![(1, Some(worktree.clone()))],
            fallback_cwd: Some(main),
        };

        assert_eq!(
            probe.decide().expect("decides").cwd,
            Some(worktree.clone()),
            "the checkout outside the worktree falls back to its root"
        );
        assert_eq!(
            probe.decide_without_probing().expect("decides").cwd,
            Some(worktree),
            "an unresolved probe falls back to the worktree root"
        );
    }

    #[test]
    fn an_unresolved_explicit_cwd_is_an_ipc_error() {
        let probe = SplitProbe {
            cwd: Some("/stuck/mount".to_owned()),
            worktree: None,
            anchor_tab: 1,
            tabs: vec![(1, None)],
            fallback_cwd: None,
        };

        let error = probe
            .decide_without_probing()
            .expect_err("cwd never resolved");

        assert_eq!(error.code, JsonRpcError::INVALID_PARAMS);
        assert!(
            error.message.contains("did not resolve"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_stuck_probe_times_out_instead_of_blocking() {
        let started = std::time::Instant::now();
        let outcome = smol::block_on(probe_off_thread(Duration::from_millis(100), || {
            std::thread::sleep(Duration::from_secs(2));
        }));
        assert!(outcome.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[gpui::test]
    fn each_flow_unit_starts_in_its_own_worktree(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;

        let root = tempfile::tempdir().expect("tempdir");
        let worktrees: Vec<PathBuf> = ["wt-a", "wt-b", "wt-c"]
            .iter()
            .map(|name| dir(root.path(), name))
            .collect();
        let cx = cx.add_empty_window();
        let new_pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| Pane::new(terminal, 1, cx))
        };
        let anchor = new_pane(cx);
        let mut ws = Workspace::restored_with_id(
            1,
            "flow",
            worktrees[0].clone(),
            vec![crate::workspace::Tab::restored(
                "wt-a",
                paneflow_config::schema::TabTitleSource::Preset,
                Some(LayoutTree::Leaf(anchor.clone())),
                Some(worktrees[0].clone()),
            )],
            0,
        );
        let anchor_tab = ws.tabs()[0].id;

        let mut cwds = Vec::new();
        for worktree in &worktrees {
            let probe = SplitProbe {
                cwd: Some(worktree.to_string_lossy().into_owned()),
                worktree: Some(worktree.clone()),
                anchor_tab,
                tabs: ws
                    .tabs()
                    .iter()
                    .map(|tab| (tab.id, tab.worktree.clone()))
                    .collect(),
                fallback_cwd: Some(worktrees[0].clone()),
            };
            let decision = probe.decide().expect("each unit resolves");
            cwds.push(decision.cwd.clone().expect("a unit always gets a cwd"));
            let anchor_pane = match &decision.target {
                SplitTarget::Tab(id) if *id == anchor_tab => Some(&anchor),
                _ => None,
            };
            let pane = new_pane(cx);
            place_split(
                &mut ws,
                decision.target,
                anchor_pane,
                SplitDirection::Vertical,
                pane,
                "unit".to_owned(),
            )
            .expect("each unit is placed");
        }

        assert_eq!(cwds.len(), 3);
        for (cwd, worktree) in cwds.iter().zip(&worktrees) {
            assert_eq!(
                canonical(cwd),
                canonical(worktree),
                "a unit ran outside its worktree"
            );
        }
        let bound: Vec<(Option<PathBuf>, usize)> = ws
            .tabs()
            .iter()
            .map(|tab| (tab.worktree.clone(), tab.pane_count()))
            .collect();
        assert_eq!(
            bound,
            vec![
                (Some(worktrees[0].clone()), 2),
                (Some(worktrees[1].clone()), 1),
                (Some(worktrees[2].clone()), 1),
            ],
            "the unit on worktree A splits beside the anchor, B and C get their own tabs"
        );
    }

    #[gpui::test]
    fn a_zoomed_tab_refuses_a_split_and_counts_its_hidden_panes(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext as _;

        let cx = cx.add_empty_window();
        let new_pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| Pane::new(terminal, 1, cx))
        };
        let (a, b) = (new_pane(cx), new_pane(cx));
        let mut tab = crate::workspace::Tab::new("zoomed", Some(LayoutTree::Leaf(a.clone())));
        tab.saved_layout =
            LayoutTree::from_panes_equal(SplitDirection::Vertical, vec![a.clone(), b]);
        let tab_id = tab.id;
        let ws = Workspace::restored_with_id(1, "ws", PathBuf::new(), vec![tab], 0);

        assert_eq!(ws.tabs()[0].pane_count(), 2);
        let error = check_split_target(&ws, &SplitTarget::Tab(tab_id), Some(&a))
            .expect_err("a zoomed tab refuses the split");
        assert_eq!(error.message, "Unzoom before splitting panes");
    }
}
