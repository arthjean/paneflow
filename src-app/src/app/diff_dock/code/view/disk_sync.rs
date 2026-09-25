use super::*;

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const RELOAD_DIFF_ATTEMPTS: usize = 2;

async fn reload_from_disk(
    this: &WeakEntity<CodeView>,
    cx: &mut AsyncApp,
    stamp: Option<FileStamp>,
    text: Option<String>,
    force: bool,
) -> bool {
    let present = text.is_some();
    let begun = cx.update(|cx| {
        this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
            view.begin_disk_reload(stamp, present, force, cx)
        })
    });
    let Ok(begun) = begun else {
        return false;
    };
    let (Some(mut diff), Some(text)) = (begun, text) else {
        return true;
    };
    let text = Arc::new(text);
    for attempt in 0..RELOAD_DIFF_ATTEMPTS {
        let DiskDiff { rope, revision } = diff;
        let incoming = Arc::clone(&text);
        let splices = cx
            .background_spawn(async move { edit::disk_splices(&rope, &incoming) })
            .await;
        let retry = attempt + 1 < RELOAD_DIFF_ATTEMPTS;
        let finished = cx.update(|cx| {
            this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
                view.finish_disk_reload(revision, splices, retry, force, cx)
            })
        });
        let Ok(next) = finished else {
            return false;
        };
        match next {
            Some(again) => diff = again,
            None => return true,
        }
    }
    true
}

impl CodeView {
    pub(crate) fn is_dirty(&self) -> bool {
        self.history.mark() != self.saved_mark
    }

    #[cfg(test)]
    pub(crate) fn has_conflict(&self) -> bool {
        self.disk == DiskState::Conflict
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        if doc.is_read_only() {
            self.flash_read_only(cx);
            return;
        }
        if !self.is_dirty() && self.disk == DiskState::InSync {
            return;
        }
        self.history.close_group();
        let contents = doc.to_disk_string();
        let path = self.path.clone();
        let expected = self.stamp;
        let mark = self.history.mark();
        self.saving = true;
        self.save_error = None;
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = cx
                .background_spawn(async move {
                    let current = FileStamp::read(&path);
                    let conflict = match (expected, current) {
                        (Some(expected), Some(current)) => expected.differs(&current),
                        (None, Some(_)) => true,
                        _ => false,
                    };
                    if conflict {
                        return Err(None);
                    }
                    save::save_blocking(&path, &contents).map_err(Some)
                })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    view.finish_save(outcome, mark, cx);
                });
            });
        })
        .detach();
    }

    fn finish_save(
        &mut self,
        outcome: Result<FileStamp, Option<String>>,
        mark: edit::HistoryMark,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        match outcome {
            Ok(stamp) => {
                self.stamp = Some(stamp);
                self.saved_mark = mark;
                self.disk = DiskState::InSync;
                self.save_error = None;
            }
            Err(Some(message)) => {
                self.save_error = Some(message);
            }
            Err(None) => {
                self.disk = DiskState::Conflict;
            }
        }
        cx.notify();
    }

    pub(super) fn start_watcher(&mut self, cx: &mut Context<Self>) {
        self._watcher = None;
        self._watch_bridge = None;
        let Some(parent) = self.path.parent().map(Path::to_path_buf) else {
            return;
        };
        let Some(name) = self.path.file_name().map(|name| name.to_os_string()) else {
            return;
        };
        let generation = self.slot.current();
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let watched_parent = parent.clone();
            let outcome = cx
                .background_spawn(async move { create_file_watcher(parent) })
                .await;
            let (watcher, bridge, rx) = match outcome {
                Ok(parts) => parts,
                Err(err) => {
                    log::warn!(
                        "could not watch {} for changes: {err}",
                        watched_parent.display()
                    );
                    return;
                }
            };
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if !view.slot.accept(generation) {
                        return;
                    }
                    view._watcher = Some(watcher);
                    view._watch_bridge = Some(bridge);
                    view.spawn_reload_loop(path, name, rx, cx);
                });
            });
        })
        .detach();
    }

    fn spawn_reload_loop(
        &mut self,
        path: PathBuf,
        name: std::ffi::OsString,
        mut rx: WatchEvents,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Some(first) = rx.next().await {
                if !event_is_relevant(&first, &name) {
                    continue;
                }
                let deadline = Instant::now() + RELOAD_DEBOUNCE;
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    let timer = cx.background_executor().timer(remaining);
                    match futures::future::select(rx.next(), timer).await {
                        Either::Left((Some(_), _)) => continue,
                        Either::Left((None, _)) => return,
                        Either::Right(_) => break,
                    }
                }
                let probe = path.clone();
                let (stamp, text) = cx
                    .background_spawn(async move {
                        (
                            FileStamp::read(&probe),
                            std::fs::read_to_string(&probe).ok(),
                        )
                    })
                    .await;
                if !reload_from_disk(&this, cx, stamp, text, false).await {
                    break;
                }
            }
        })
        .detach();
    }

    fn begin_disk_reload(
        &mut self,
        stamp: Option<FileStamp>,
        present: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Option<DiskDiff> {
        let Some(stamp) = stamp.filter(|_| present) else {
            if self.disk != DiskState::Deleted {
                self.disk = DiskState::Deleted;
                cx.notify();
            }
            return None;
        };
        if !force {
            if self.stamp == Some(stamp) && self.disk == DiskState::InSync {
                return None;
            }
            self.stamp = Some(stamp);
            if self.is_dirty() {
                self.disk = DiskState::Conflict;
                cx.notify();
                return None;
            }
        } else {
            self.stamp = Some(stamp);
        }
        self.disk = DiskState::InSync;
        self.state.document().map(DiskDiff::of)
    }

    fn finish_disk_reload(
        &mut self,
        revision: u64,
        splices: Vec<(Range<usize>, String)>,
        retry: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Option<DiskDiff> {
        let doc = self.state.document()?;
        if doc.revision() != revision {
            if retry {
                return Some(DiskDiff::of(doc));
            }
            self.disk = DiskState::Conflict;
            cx.notify();
            return None;
        }
        if !force && self.is_dirty() {
            self.disk = DiskState::Conflict;
            cx.notify();
            return None;
        }
        self.apply_disk_splices(&splices, cx);
        self.saved_mark = self.history.mark();
        None
    }

    fn apply_disk_splices(&mut self, ops: &[(Range<usize>, String)], cx: &mut Context<Self>) {
        if ops.is_empty() {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        let scroll_rows = self.scroll.rows();
        let after = edit::shift_selection_for_splices(self.selection, ops);
        let reason = doc.read_only_reason();
        if reason.is_some()
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_read_only(None);
        }
        let replaced = self.splice_all(ops, after, EditGroup::Atomic, cx);
        if let Some(reason) = reason
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_read_only(Some(reason));
        }
        if !replaced {
            return;
        }
        self.sync_scroll_line_count();
        self.scroll.set_rows(scroll_rows);
        self.popup = None;
        self.reset_tracker(cx);
        cx.notify();
    }

    pub(super) fn resolve_keep_mine(&mut self, cx: &mut Context<Self>) {
        self.disk = DiskState::InSync;
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let stamp = cx
                .background_spawn(async move { FileStamp::read(&path) })
                .await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    view.stamp = stamp;
                    cx.notify();
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn resolve_reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let probe = path.clone();
            let (stamp, text) = cx
                .background_spawn(async move {
                    (
                        FileStamp::read(&probe),
                        std::fs::read_to_string(&probe).ok(),
                    )
                })
                .await;
            reload_from_disk(&this, cx, stamp, text, true).await;
        })
        .detach();
    }

    pub(super) fn save_action(&mut self, _: &CeSave, _w: &mut Window, cx: &mut Context<Self>) {
        self.save(cx);
    }
}

#[cfg(test)]
impl CodeView {
    pub(super) fn disk_changed(
        &mut self,
        stamp: Option<FileStamp>,
        text: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let present = text.is_some();
        let text = text.unwrap_or_default();
        let Some(diff) = self.begin_disk_reload(stamp, present, false, cx) else {
            return;
        };
        let splices = edit::disk_splices(&diff.rope, &text);
        self.finish_disk_reload(diff.revision, splices, false, false, cx);
    }

    pub(super) fn adopt_disk_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let splices = edit::disk_splices(doc.text(), text);
        self.apply_disk_splices(&splices, cx);
    }
}

fn create_file_watcher(
    parent: PathBuf,
) -> Result<(RecommendedWatcher, WatchBridge, WatchEvents), String> {
    if !parent.is_dir() {
        return Err("the parent directory no longer exists".to_string());
    }
    let (tx, rx) = mpsc::unbounded::<notify::Result<notify::Event>>();
    let bridge: WatchBridge = Arc::new(Mutex::new(Some(tx)));
    let notify_side = Arc::clone(&bridge);
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            if let Ok(guard) = notify_side.lock()
                && let Some(tx) = guard.as_ref()
            {
                let _ = tx.unbounded_send(result);
            }
        },
        notify::Config::default(),
    )
    .map_err(|err| err.to_string())?;
    watcher
        .watch(&parent, RecursiveMode::NonRecursive)
        .map_err(|err| err.to_string())?;
    Ok((watcher, bridge, rx))
}

fn event_is_relevant(result: &notify::Result<notify::Event>, target: &std::ffi::OsStr) -> bool {
    match result {
        Ok(event) => event
            .paths
            .iter()
            .any(|path| path.file_name() == Some(target)),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::super::tests::*;
    use super::*;

    #[gpui::test]
    fn an_external_reload_that_drops_lines_rebinds_the_position(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/reload.rs", &rows_of_code(500));

        view.update(cx, |view, cx| {
            view.scroll.set_rows(view.scroll.max_rows());
            cx.notify();
        });
        cx.run_until_parked();
        assert!(view.read_with(cx, |view, _| view.scroll_rows()) > 400.0);

        view.update(cx, |view, cx| {
            view.adopt_disk_text(&rows_of_code(25), cx);
        });
        cx.run_until_parked();

        let (rows, max_rows, viewport_h) = view.read_with(cx, |view, _| {
            (
                view.scroll_rows(),
                view.scroll.max_rows(),
                view.scroll.viewport_height(),
            )
        });
        let line_count = view.read_with(cx, |view, _| {
            view.document().expect("a loaded document").line_count()
        });
        assert_eq!(
            max_rows,
            line_count as f64 - f64::from(viewport_h) / f64::from(CODE_ROW_HEIGHT)
        );
        assert_eq!(rows, max_rows, "the position must stop at the new end");
    }

    #[gpui::test]
    fn saving_writes_the_file_and_settles_the_dirty_mark(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "two\n", window, cx);
            assert!(view.is_dirty());
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\ntwo\n");
        view.update_in(cx, |view, window, cx| {
            assert!(!view.is_dirty(), "a landed save clears the dot");
            assert!(view.save_error.is_none());

            view.replace_text_in_range(None, "x", window, cx);
            assert!(view.is_dirty());
            view.undo(&CeUndo, window, cx);
            assert!(
                !view.is_dirty(),
                "undoing back to the saved state clears the dot again"
            );
        });
    }

    #[gpui::test]
    fn a_save_is_refused_when_the_file_changed_underneath(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, "written by someone else\n").expect("agent write");

        view.update_in(cx, |view, window, cx| {
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "written by someone else\n",
            "the refusal happened before the write"
        );
        view.update(cx, |view, _cx| {
            assert!(view.has_conflict(), "the user is asked to choose");
            assert!(view.is_dirty(), "the in-memory edits survived");
            assert_eq!(text_of(view), "one\nmine\n");
        });
    }

    #[gpui::test]
    async fn an_external_write_reloads_through_the_watcher(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", true);
        let path = dir.path().join("main.rs");
        cx.executor().allow_parking();
        cx.run_until_parked();

        std::fs::write(&path, "one\nAGENT\ntwo\n").expect("agent write");
        for _ in 0..300 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| text_of(view) == "one\nAGENT\ntwo\n") {
                break;
            }
            smol::Timer::after(Duration::from_millis(10)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(
                text_of(view),
                "one\nAGENT\ntwo\n",
                "the watched write reached the document through the background diff"
            );
            assert!(!view.is_dirty(), "a silent reload is the new saved state");
            assert_eq!(view.disk, DiskState::InSync);
            let bridge = view._watch_bridge.take().expect("bridged watcher");
            *bridge.lock().expect("bridge lock") = None;
            view._watcher = None;
        });
    }

    #[gpui::test]
    fn a_reload_that_races_an_edit_recomputes_once_then_conflicts(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            let diff = view
                .begin_disk_reload(stamp, true, false, cx)
                .expect("a clean document starts a diff");
            let splices = edit::disk_splices(&diff.rope, "ONE!\nTWO!\n");
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "x", window, cx);

            let again = view
                .finish_disk_reload(diff.revision, splices, true, false, cx)
                .expect("a stale revision buys exactly one recomputation");
            assert_eq!(
                text_of(view),
                "xone\ntwo\n",
                "splices computed against an older revision are refused"
            );
            assert!(!view.has_conflict(), "the first miss is not a conflict yet");

            let splices = edit::disk_splices(&again.rope, "ONE!\nTWO!\n");
            view.replace_text_in_range(None, "y", window, cx);
            assert!(
                view.finish_disk_reload(again.revision, splices, false, false, cx)
                    .is_none(),
                "the second stale delivery gives up"
            );
            assert!(
                view.has_conflict(),
                "a document that keeps moving is left to the user"
            );
            assert_eq!(text_of(view), "xyone\ntwo\n", "the user edits survived");
        });
    }

    #[gpui::test]
    fn a_reload_that_settles_on_a_dirty_document_keeps_the_user_text(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            let diff = view
                .begin_disk_reload(stamp, true, false, cx)
                .expect("a clean document starts a diff");
            let splices = edit::disk_splices(&diff.rope, "ONE!\nTWO!\n");
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "x", window, cx);

            let again = view
                .finish_disk_reload(diff.revision, splices, true, false, cx)
                .expect("a stale revision buys exactly one recomputation");
            let splices = edit::disk_splices(&again.rope, "ONE!\nTWO!\n");
            assert!(
                view.finish_disk_reload(again.revision, splices, false, false, cx)
                    .is_none(),
                "the recomputed diff reaches a document that stopped moving"
            );
            assert_eq!(
                text_of(view),
                "xone\ntwo\n",
                "a document the user touched during the diff is never overwritten"
            );
            assert!(view.is_dirty(), "the unsaved edit is still unsaved");
            assert!(
                view.has_conflict(),
                "the user resolves it like any other conflict"
            );
        });
    }

    #[gpui::test]
    fn a_forced_reload_still_overwrites_the_document_the_user_edited(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "x", window, cx);
            assert!(view.is_dirty(), "the fixture starts dirty");

            let diff = view
                .begin_disk_reload(stamp, true, true, cx)
                .expect("a forced reload ignores the dirty mark");
            let splices = edit::disk_splices(&diff.rope, "ONE!\nTWO!\n");
            assert!(
                view.finish_disk_reload(diff.revision, splices, false, true, cx)
                    .is_none()
            );
            assert_eq!(
                text_of(view),
                "ONE!\nTWO!\n",
                "discarding my changes is what the user asked for"
            );
            assert!(!view.is_dirty(), "and the reload is the new saved state");
            assert!(!view.has_conflict());
        });
    }

    #[gpui::test]
    async fn a_reload_whose_tab_closed_ends_without_a_panic(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "one\ntwo\n").expect("seed");
        let stamp = FileStamp::read(&path);
        let seeded = seeded_view(path, "one\n");
        let weak = cx.update(|cx| {
            let view = cx.new(seeded);
            let weak = view.downgrade();
            drop(view);
            weak
        });
        cx.run_until_parked();
        assert!(weak.upgrade().is_none(), "the tab is gone");

        let carried = cx
            .spawn(|mut cx| async move {
                reload_from_disk(&weak, &mut cx, stamp, Some("one\ntwo\n".to_string()), false).await
            })
            .await;
        assert!(
            !carried,
            "a reload delivered to a closed tab stops its loop instead of panicking"
        );
    }

    #[gpui::test]
    fn an_external_write_reloads_a_clean_document(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "ONE!\nTWO!\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update(cx, |view, cx| {
            view.selection = CodeSelection::at(4);
            view.disk_changed(stamp, Some("ONE!\nTWO!\n".to_string()), cx);
        });

        view.update_in(cx, |view, window, cx| {
            assert_eq!(text_of(view), "ONE!\nTWO!\n");
            assert!(!view.has_conflict(), "a clean document reloads silently");
            assert!(!view.is_dirty(), "the reload is the new saved state");
            assert_eq!(
                view.cursor(),
                4,
                "the caret held: the line count is unchanged"
            );

            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                "one\ntwo\n",
                "Ctrl+Z recovers what was replaced"
            );
        });
    }

    #[gpui::test]
    fn a_multi_hunk_reload_reaches_the_highlighter_as_one_batch(cx: &mut TestAppContext) {
        let original = (0..40)
            .map(|row| format!("fn f{row:03}() {{}}\n"))
            .collect::<String>();
        let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
        lines[5] = "fn agent_a() {}".to_string();
        lines[25] = "fn agent_b() {}".to_string();
        let incoming = lines.join("\n") + "\n";
        let (dir, view, cx) = file_view(cx, &original, false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, &incoming).expect("agent write");
        let stamp = FileStamp::read(&path);

        let before = view.update(cx, |view, _cx| {
            let highlighter = view.highlighter().expect("highlighter");
            assert!(highlighter.is_enabled(), "the fixture must be colored");
            highlighter.generation()
        });

        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some(incoming.clone()), cx);
            assert_eq!(text_of(view), incoming, "both hunks landed");
            assert_eq!(
                view.highlighter().expect("highlighter").generation(),
                before + 1,
                "two hunks reach the highlighter as a single batched edit"
            );
        });
    }

    #[gpui::test]
    fn an_external_write_keeps_a_distant_caret_and_undoes_all_hunks_once(cx: &mut TestAppContext) {
        let original = (0..30)
            .map(|row| format!("line {row:03}\n"))
            .collect::<String>();
        let mut incoming_lines = original.lines().map(str::to_string).collect::<Vec<_>>();
        for (row, line) in incoming_lines.iter_mut().enumerate().take(8).skip(5) {
            *line = format!("agent changed line {row:03}");
        }
        incoming_lines[15] = "second distant hunk".to_string();
        let incoming = incoming_lines.join("\n") + "\n";
        let (dir, view, cx) = file_view_named(cx, "main.txt", &original, false);
        let path = dir.path().join("main.txt");
        std::fs::write(&path, &incoming).expect("agent write");
        let stamp = FileStamp::read(&path);
        let caret = original.find("line 025").expect("caret line") + 5;
        let expected = incoming.find("line 025").expect("shifted caret line") + 5;

        view.update(cx, |view, cx| {
            view.selection = CodeSelection::at(caret);
            view.disk_changed(stamp, Some(incoming.clone()), cx);
            assert_eq!(view.cursor(), expected);
            assert_eq!(text_of(view), incoming);
        });

        view.update_in(cx, |view, window, cx| {
            view.undo(&CeUndo, window, cx);
            assert_eq!(text_of(view), original, "every hunk shares one transaction");
            assert_eq!(view.cursor(), caret);
        });
    }

    #[gpui::test]
    fn an_identical_external_reload_pushes_no_transaction(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        view.update(cx, |view, cx| {
            let before = view.history.mark();
            view.adopt_disk_text("one\r\ntwo\r\n", cx);
            assert_eq!(view.history.mark(), before);
            assert_eq!(text_of(view), "one\ntwo\n");
        });
    }

    #[gpui::test]
    fn a_crlf_reload_preserves_the_document_line_ending(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "one\r\ntwo\r\n", false);
        view.update(cx, |view, cx| {
            view.adopt_disk_text("one\r\nTWO\r\n", cx);
            let doc = view.document().expect("document");
            assert_eq!(doc.to_disk_string(), "one\r\nTWO\r\n");
        });
    }

    #[gpui::test]
    fn a_read_only_reload_temporarily_unlocks_and_restores_the_document(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "old\n", false);
        view.update(cx, |view, cx| {
            view.state
                .document_mut()
                .expect("document")
                .set_read_only(Some(ReadOnlyReason::Permissions));
            view.adopt_disk_text("new content\n", cx);
            assert_eq!(text_of(view), "new content\n");
            assert_eq!(
                view.document().and_then(CodeDocument::read_only_reason),
                Some(ReadOnlyReason::Permissions)
            );
        });
    }

    #[gpui::test]
    fn a_whole_document_reload_remeasures_the_longest_line(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "this line starts longest\nx\n", false);
        view.update(cx, |view, cx| {
            view.adopt_disk_text("a\na much longer replacement line\n", cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert_eq!(
                view.document().expect("document").longest_line_chars(),
                "a much longer replacement line".len()
            );
        });
    }

    #[gpui::test]
    fn an_external_write_on_a_dirty_document_raises_a_conflict(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, "theirs\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some("theirs\n".to_string()), cx);
            assert!(view.has_conflict());
            assert_eq!(text_of(view), "one\nmine\n", "the buffer was not touched");
        });

        view.update(cx, |view, cx| view.resolve_keep_mine(cx));
        cx.executor().allow_parking();
        cx.run_until_parked();
        view.update_in(cx, |view, window, cx| {
            assert!(!view.has_conflict());
            view.save_action(&CeSave, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\nmine\n");
    }

    #[gpui::test]
    fn a_deleted_file_is_flagged_and_saving_recreates_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        std::fs::remove_file(&path).expect("delete");

        view.update(cx, |view, cx| {
            view.disk_changed(None, None, cx);
            view.stamp = None;
            assert_eq!(view.disk, DiskState::Deleted);
        });

        view.update_in(cx, |view, window, cx| {
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\n");
        view.update(cx, |view, _cx| assert_eq!(view.disk, DiskState::InSync));
    }

    #[gpui::test]
    fn opening_a_real_file_registers_the_conflict_watcher(cx: &mut TestAppContext) {
        let (_dir, view, cx) = file_view(cx, "one\n", true);
        cx.executor().allow_parking();
        cx.run_until_parked();
        view.update(cx, |view, _cx| {
            assert!(view._watcher.is_some(), "the parent directory is watched");
            let bridge = view
                ._watch_bridge
                .take()
                .expect("the reload task is bridged to the watcher");
            *bridge.lock().expect("bridge lock") = None;
            view._watcher = None;
        });
    }

    #[gpui::test]
    async fn only_the_latest_rapid_open_keeps_its_watcher(cx: &mut TestAppContext) {
        let first = tempfile::tempdir().expect("first tempdir");
        let second = tempfile::tempdir().expect("second tempdir");
        let first_path = first.path().join("first.rs");
        let second_path = second.path().join("second.rs");
        std::fs::write(&first_path, "first\n").expect("first fixture");
        std::fs::write(&second_path, "second\n").expect("second fixture");
        let (view, cx) = view(cx, "seed\n");
        cx.executor().allow_parking();

        view.update(cx, |view, cx| {
            view.open(first_path, cx);
            view.open(second_path.clone(), cx);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| {
                view.document()
                    .and_then(|doc| doc.line_string(0))
                    .as_deref()
                    == Some("second")
                    && view._watcher.is_some()
            }) {
                break;
            }
            smol::Timer::after(Duration::from_millis(1)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(view.path(), second_path);
            assert_eq!(
                view.document()
                    .and_then(|doc| doc.line_string(0))
                    .as_deref(),
                Some("second")
            );
            assert!(view._watcher.is_some());
            if let Some(bridge) = view._watch_bridge.take() {
                *bridge.lock().expect("bridge lock") = None;
            }
            view._watcher = None;
        });
    }

    #[test]
    fn a_removed_parent_refuses_watcher_creation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let parent = dir.path().to_path_buf();
        drop(dir);
        assert!(create_file_watcher(parent).is_err());
    }
}
