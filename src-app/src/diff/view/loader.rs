use super::*;

impl DiffView {
    pub(super) fn start_loading(&mut self, cx: &mut Context<Self>) {
        let base = self.base_ref.clone();
        let options = self.options.for_cached_rows();
        let mode = self.last_effective_mode;
        let theme = crate::theme::active_theme();
        let theme_generation = crate::theme::theme_generation();
        let col = &mut self.column;
        col.generation = col.generation.wrapping_add(1);
        col.loading_mode = None;
        col.loading_theme_generation = Some(theme_generation);
        let generation = col.generation;
        let path = col.path.clone();
        let branch = col.branch.clone();
        log::debug!("diff: start_loading base={base:?} ({branch})");
        if base.is_empty() {
            col.state = ColumnState::Failed("Select a base branch".to_string());
            cx.notify();
            return;
        }
        log::debug!("diff: ({branch}) task SPAWNED (gen={generation})");
        cx.spawn(async move |this, cx| {
            log::debug!("diff: ({branch}) task STARTED (polled)");
            let bc = branch.clone();
            let built = smol::unblock(move || {
                let fingerprint = super::super::git::column_fingerprint(&path, &base);
                let t0 = Instant::now();
                let diff = super::super::git::compute_worktree_diff(&path, &base, options);
                let file_stats = super::super::git::compute_worktree_file_stats(&path, &base);
                log::debug!(
                    "diff: ({bc}) computed {} files in {:?} (error={:?})",
                    diff.files.len(),
                    t0.elapsed(),
                    diff.error
                );
                if let Some(e) = diff.error {
                    return Built::Failed(e);
                }
                let t1 = Instant::now();
                let syntax = SYNTAX_HIGHLIGHT_ENABLED
                    .then(|| super::super::syntax::DiffSyntax::from_theme(&theme));
                let row_caches = build_file_row_caches(&diff.files, syntax.as_ref());
                let rows = build_rows_for_mode_with_caches(&diff.files, mode, &row_caches);
                let files = diff
                    .files
                    .iter()
                    .map(|f| {
                        let (added, removed) = file_stats
                            .get(&f.path)
                            .map(|stat| (stat.added, stat.removed))
                            .unwrap_or_else(|| f.line_counts());
                        FileEntry {
                            path: f.path.clone(),
                            change: f.change,
                            old_path: f.old_path.clone(),
                            added,
                            removed,
                            is_binary: f.is_binary,
                        }
                    })
                    .collect();
                log::debug!(
                    "diff: ({bc}) built {} rows for {} in {:?}",
                    match &rows {
                        BuiltModeRows::Unified { rows, .. } => rows.len(),
                        BuiltModeRows::Split { rows, .. } => rows.len(),
                    },
                    mode.label(),
                    t1.elapsed()
                );
                let cwd = path.to_string_lossy();
                let attribution = crate::agent_sessions::attribution_for_column(&cwd, &bc);
                Built::Loaded {
                    rows,
                    file_count: diff.files.len(),
                    files,
                    files_full: diff.files,
                    row_caches,
                    theme_generation,
                    fingerprint: Box::new(fingerprint),
                    attribution,
                }
            })
            .await;
            log::debug!("diff: ({branch}) off-thread done, applying on main thread");
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx| {
                    let col = &mut view.column;
                    if col.generation != generation {
                        log::debug!(
                            "diff: ({branch}) superseded - task gen={generation} != gen={}",
                            col.generation
                        );
                        return;
                    }
                    let new_state = match built {
                        Built::Failed(e) => {
                            log::warn!("diff: ({branch}) FAILED: {e}");
                            col.loading_mode = None;
                            col.loading_theme_generation = None;
                            ColumnState::Failed(e)
                        }
                        Built::Loaded {
                            rows,
                            file_count,
                            files,
                            files_full,
                            row_caches,
                            theme_generation,
                            fingerprint,
                            attribution,
                        } => {
                            log::debug!("diff: ({branch}) LOADED ({file_count} files)");
                            col.fingerprint = Some(*fingerprint);
                            col.attribution = attribution;
                            col.loading_mode = None;
                            col.loading_theme_generation = None;
                            match rows {
                                BuiltModeRows::Unified { rows, anchors } => ColumnState::Loaded {
                                    unified: Some(Rc::new(rows)),
                                    split: None,
                                    file_count,
                                    files: Rc::new(files),
                                    anchors_unified: Some(Rc::new(anchors)),
                                    anchors_split: None,
                                    files_full: Arc::new(files_full),
                                    row_caches: Arc::new(row_caches),
                                    theme_generation,
                                },
                                BuiltModeRows::Split { rows, anchors } => ColumnState::Loaded {
                                    unified: None,
                                    split: Some(Rc::new(rows)),
                                    file_count,
                                    files: Rc::new(files),
                                    anchors_unified: None,
                                    anchors_split: Some(Rc::new(anchors)),
                                    files_full: Arc::new(files_full),
                                    row_caches: Arc::new(row_caches),
                                    theme_generation,
                                },
                            }
                        }
                    };
                    col.state = new_state;
                    col.recompute_display_for(mode);
                    col.clear_display_mode(mode.other());
                    view.body_menu = None;
                    view.schedule_mode_build(mode.other(), cx);
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn ensure_mode_loaded(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        let col = &mut self.column;
        if col.has_rows_for_mode(mode) {
            if col.loading_mode == Some(mode) {
                col.loading_mode = None;
            }
            if !col.has_display_for_mode(mode) {
                col.recompute_display_for(mode);
            }
            return;
        }
        self.schedule_mode_build(mode, cx);
    }

    fn schedule_mode_build(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        let col = &mut self.column;
        if col.has_rows_for_mode(mode) || col.loading_mode.is_some() {
            return;
        }
        let (files, row_caches) = match &col.state {
            ColumnState::Loaded {
                files_full,
                row_caches,
                ..
            } => (files_full.clone(), row_caches.clone()),
            _ => return,
        };
        let generation = col.generation;
        col.loading_mode = Some(mode);
        log::debug!(
            "diff: scheduling lazy {} row build (gen={generation})",
            mode.label()
        );
        cx.spawn(async move |this, cx| {
            let rows = smol::unblock(move || {
                build_rows_for_mode_with_caches(files.as_ref(), mode, row_caches.as_ref())
            })
            .await;
            let _ = cx.update(|cx| {
                this.update(cx, |view: &mut Self, cx| {
                    let col = &mut view.column;
                    if col.generation != generation || col.loading_mode != Some(mode) {
                        return;
                    }
                    if !matches!(col.state, ColumnState::Loaded { .. }) {
                        col.loading_mode = None;
                        return;
                    }
                    col.loading_mode = None;
                    col.insert_mode_rows(rows);
                    col.recompute_display_for(mode);
                    cx.notify();
                })
            });
        })
        .detach();
    }
}
