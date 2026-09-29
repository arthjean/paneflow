use super::*;

const RELOAD_DEBOUNCE: Duration = Duration::from_millis(200);
const RELOAD_DIFF_ATTEMPTS: usize = 2;

pub(crate) enum SaveStart {
    Clean,
    Deferred,
    Started(gpui::Task<Result<(), String>>),
    Refused(String),
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn disk_generation(this: &WeakEntity<CodeView>, cx: &mut AsyncApp) -> Option<u64> {
    cx.update(|cx| this.read_with(cx, |view: &CodeView, _| view.disk_generation))
        .ok()
}

pub(super) struct DiskRead {
    text: String,
    stamp: FileStamp,
    facts: DiskFacts,
}

pub(super) fn read_for_reload(path: &Path) -> Result<DiskRead, CodeLoadError> {
    let disk = super::super::load::read_disk_text(path)?;
    let longest = disk
        .text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    Ok(DiskRead {
        facts: DiskFacts {
            read_only: super::super::load::read_only_reason_for(longest, disk.read_only),
            line_ending: LineEnding::detect(&disk.text),
        },
        stamp: disk.stamp,
        text: disk.text,
    })
}

async fn probe_disk(
    this: &WeakEntity<CodeView>,
    cx: &mut AsyncApp,
    path: &Path,
    force: bool,
) -> bool {
    let Some(generation) = disk_generation(this, cx) else {
        return false;
    };
    let probe = path.to_path_buf();
    let read = cx
        .background_spawn(async move { read_for_reload(&probe) })
        .await;
    reload_from_disk(this, cx, generation, read, force).await
}

async fn reload_from_disk(
    this: &WeakEntity<CodeView>,
    cx: &mut AsyncApp,
    generation: u64,
    read: Result<DiskRead, CodeLoadError>,
    force: bool,
) -> bool {
    let read = match read {
        Ok(read) => Some(read),
        Err(CodeLoadError::NotFound) => None,
        Err(error) => {
            return cx
                .update(|cx| {
                    this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
                        if view.disk_generation == generation {
                            view.disk_unreadable(error, force, cx);
                        }
                    })
                })
                .is_ok();
        }
    };
    let stamp = read.as_ref().map(|read| read.stamp);
    let present = read.is_some();
    let begun = cx.update(|cx| {
        this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
            if view.disk_generation != generation {
                return None;
            }
            if present && matches!(view.state, CodeLoadState::Failed(_)) {
                view.start_load(cx);
                return None;
            }
            view.begin_disk_reload(stamp, present, force, cx)
        })
    });
    let Ok(begun) = begun else {
        return false;
    };
    let (Some(mut diff), Some(read)) = (begun, read) else {
        return true;
    };
    let facts = read.facts;
    let text = Arc::new(read.text);
    for attempt in 0..RELOAD_DIFF_ATTEMPTS {
        let DiskDiff { rope, mut meta } = diff;
        meta.facts = Some(facts);
        let incoming = Arc::clone(&text);
        let splices = cx
            .background_spawn(async move { edit::disk_splices(&rope, &incoming) })
            .await;
        let retry = attempt + 1 < RELOAD_DIFF_ATTEMPTS;
        let finished = cx.update(|cx| {
            this.update(cx, |view: &mut CodeView, cx: &mut Context<CodeView>| {
                view.finish_disk_reload(meta, splices, retry, force, cx)
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
        if let SaveStart::Started(task) = self.start_save(true, cx) {
            task.detach();
        }
    }

    pub(crate) fn save_for_close(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Task<Result<(), String>> {
        match self.start_save(false, cx) {
            SaveStart::Clean => gpui::Task::ready(Ok(())),
            SaveStart::Started(task) => task,
            SaveStart::Deferred => gpui::Task::ready(Err(format!(
                "{} is reloading from disk.",
                file_name_of(&self.path)
            ))),
            SaveStart::Refused(reason) => gpui::Task::ready(Err(reason)),
        }
    }

    pub(crate) fn discard_unsaved(&mut self) {
        self.saved_mark = self.history.mark();
    }

    pub(crate) fn start_save(&mut self, defer: bool, cx: &mut Context<Self>) -> SaveStart {
        let name = file_name_of(&self.path);
        if self.saving {
            return SaveStart::Refused(format!("{name} is already being saved."));
        }
        let Some(doc) = self.state.document() else {
            return SaveStart::Clean;
        };
        if doc.is_read_only() {
            self.flash_read_only(cx);
            return SaveStart::Refused(format!("{name} is read-only."));
        }
        if self.disk == DiskState::Conflict {
            cx.notify();
            return SaveStart::Refused(format!(
                "{name} changed on disk. Choose Keep mine or Reload."
            ));
        }
        if !self.is_dirty() && self.disk == DiskState::InSync {
            return SaveStart::Clean;
        }
        if self.reloading {
            if defer {
                self.save_after_reload = true;
                return SaveStart::Deferred;
            }
            return SaveStart::Refused(format!("{name} is reloading from disk."));
        }
        self.history.close_group();
        let contents = doc.to_disk_string();
        let path = self.path.clone();
        let expected = self.buffer_stamp;
        let mark = self.history.mark();
        self.saving = true;
        self.save_error = None;
        cx.notify();
        SaveStart::Started(
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let outcome = cx
                    .background_spawn(
                        async move { save::save_blocking(&path, &contents, expected) },
                    )
                    .await;
                cx.update(|cx| {
                    this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                        view.finish_save(outcome, mark, cx)
                    })
                })
                .unwrap_or_else(|_| Err(format!("{name} was closed before it was saved.")))
            }),
        )
    }

    fn finish_save(
        &mut self,
        outcome: Result<FileStamp, save::SaveFailure>,
        mark: edit::HistoryMark,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.saving = false;
        cx.notify();
        match outcome {
            Ok(stamp) => {
                self.buffer_stamp = Some(stamp);
                self.disk_stamp = Some(stamp);
                self.disk_generation = self.disk_generation.wrapping_add(1);
                self.saved_mark = mark;
                self.disk = DiskState::InSync;
                self.save_error = None;
                Ok(())
            }
            Err(save::SaveFailure::Write(message)) => {
                self.save_error = Some(message.clone());
                Err(format!("{}: {message}", file_name_of(&self.path)))
            }
            Err(save::SaveFailure::ChangedOnDisk(observed)) => {
                self.disk_stamp = observed.or(self.disk_stamp);
                self.disk = DiskState::Conflict;
                Err(format!(
                    "{} changed on disk. Choose Keep mine or Reload.",
                    file_name_of(&self.path)
                ))
            }
        }
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
                    view.spawn_reload_loop(path.clone(), name, rx, cx);
                    cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                        probe_disk(&this, cx, &path, false).await;
                    })
                    .detach();
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
                if !probe_disk(&this, cx, &path, false).await {
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
            self.disk_stamp = None;
            if self.disk != DiskState::Deleted {
                self.disk = DiskState::Deleted;
                cx.notify();
            }
            return None;
        };
        let seen = self.disk_stamp == Some(stamp);
        self.disk_stamp = Some(stamp);
        if !force {
            if self.buffer_stamp == Some(stamp) && self.disk == DiskState::InSync {
                return None;
            }
            if seen && self.disk == DiskState::Conflict {
                return None;
            }
            if self.is_dirty() {
                self.disk = DiskState::Conflict;
                cx.notify();
                return None;
            }
        }
        self.disk = DiskState::InSync;
        let diff = self.state.document().map(|doc| DiskDiff::of(doc, stamp))?;
        self.reloading = true;
        Some(diff)
    }

    fn finish_disk_reload(
        &mut self,
        meta: DiskMeta,
        splices: Vec<(Range<usize>, String)>,
        retry: bool,
        force: bool,
        cx: &mut Context<Self>,
    ) -> Option<DiskDiff> {
        let Some(doc) = self.state.document() else {
            self.end_disk_reload(cx);
            return None;
        };
        if doc.revision() != meta.revision {
            if retry {
                return Some(DiskDiff::of(doc, meta.stamp));
            }
            self.disk = DiskState::Conflict;
            self.end_disk_reload(cx);
            return None;
        }
        if !force && self.is_dirty() {
            self.disk = DiskState::Conflict;
            self.end_disk_reload(cx);
            return None;
        }
        self.apply_disk_splices(&splices, cx);
        if let Some(facts) = meta.facts
            && let Some(doc) = self.state.document_mut()
        {
            doc.set_line_ending(facts.line_ending);
            doc.set_read_only(facts.read_only);
        }
        self.saved_mark = self.history.mark();
        self.buffer_stamp = Some(meta.stamp);
        self.end_disk_reload(cx);
        None
    }

    fn disk_unreadable(&mut self, error: CodeLoadError, force: bool, cx: &mut Context<Self>) {
        if !force && self.is_dirty() {
            if self.disk != DiskState::Conflict {
                self.disk = DiskState::Conflict;
                cx.notify();
            }
            return;
        }
        self.state = CodeLoadState::Failed(error);
        self.history.clear();
        self.saved_mark = edit::HistoryMark::default();
        self.popup = None;
        self.buffer_stamp = None;
        self.disk_stamp = None;
        self.disk = DiskState::InSync;
        self.reloading = false;
        self.save_after_reload = false;
        cx.notify();
    }

    fn end_disk_reload(&mut self, cx: &mut Context<Self>) {
        self.reloading = false;
        cx.notify();
        if std::mem::take(&mut self.save_after_reload) {
            self.save(cx);
        }
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
        self.buffer_stamp = self.disk_stamp;
        self.disk = DiskState::InSync;
        cx.notify();
    }

    pub(super) fn resolve_reload(&mut self, cx: &mut Context<Self>) {
        let path = self.path.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            probe_disk(&this, cx, &path, true).await;
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
        self.finish_disk_reload(diff.meta, splices, false, false, cx);
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
    use gpui::{Entity, TestAppContext, VisualTestContext};

    use super::super::tests::*;
    use super::*;

    fn read_of(stamp: Option<FileStamp>, text: &str) -> Result<DiskRead, CodeLoadError> {
        Ok(DiskRead {
            text: text.to_string(),
            stamp: stamp.expect("a stamped fixture"),
            facts: DiskFacts {
                read_only: None,
                line_ending: LineEnding::detect(text),
            },
        })
    }

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
    fn saving_during_a_conflict_writes_nothing_until_keep_mine(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, "the agent's version\n").expect("agent write");
        let stamp = FileStamp::read(&path);
        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some("the agent's version\n".to_string()), cx);
            assert!(view.has_conflict());
        });

        view.update_in(cx, |view, window, cx| view.save_action(&CeSave, window, cx));
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "the agent's version\n",
            "Ctrl+S under the banner never overwrites the agent"
        );
        view.update(cx, |view, _cx| {
            assert!(view.has_conflict(), "the banner stays up");
            assert!(view.is_dirty());
        });
    }

    #[gpui::test]
    fn a_save_during_a_reload_waits_and_never_writes_over_the_new_disk_text(
        cx: &mut TestAppContext,
    ) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "agent rewrote it\n").expect("agent write");
        let stamp = FileStamp::read(&path);

        view.update_in(cx, |view, window, cx| {
            let diff = view
                .begin_disk_reload(stamp, true, false, cx)
                .expect("a clean document starts a reload");
            view.selection = CodeSelection::at(0);
            view.replace_text_in_range(None, "typed ", window, cx);
            view.save_action(&CeSave, window, cx);
            assert!(!view.saving, "the save waits for the reload to land");

            let splices = edit::disk_splices(&diff.rope, "agent rewrote it\n");
            let again = view
                .finish_disk_reload(diff.meta, splices, true, false, cx)
                .expect("the edit forces one recomputation");
            let splices = edit::disk_splices(&again.rope, "agent rewrote it\n");
            assert!(
                view.finish_disk_reload(again.meta, splices, false, false, cx)
                    .is_none()
            );
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "agent rewrote it\n",
            "the deferred save was compared with the disk text, not written over it"
        );
        view.update(cx, |view, _cx| {
            assert!(view.has_conflict());
            assert_eq!(text_of(view), "typed one\n");
        });
    }

    #[gpui::test]
    async fn a_probe_started_before_a_save_is_ignored_after_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        let stale_stamp = FileStamp::read(&path);
        let generation = view.update(cx, |view, _cx| view.disk_generation);

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "saved\n", window, cx);
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "one\nsaved\n"
        );

        let weak = view.downgrade();
        let carried = cx
            .spawn(|mut cx| async move {
                reload_from_disk(
                    &weak,
                    &mut cx,
                    generation,
                    read_of(stale_stamp, "one\n"),
                    false,
                )
                .await
            })
            .await;

        assert!(carried);
        view.update(cx, |view, _cx| {
            assert_eq!(
                text_of(view),
                "one\nsaved\n",
                "the pre-save read never reverts the saved text"
            );
            assert!(!view.has_conflict());
            assert!(!view.is_dirty());
        });
    }

    #[gpui::test]
    async fn a_close_time_save_reports_a_conflict_and_keeps_the_file_dirty(
        cx: &mut TestAppContext,
    ) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, "an agent got there first\n").expect("agent write");
        cx.executor().allow_parking();

        let save = view.update(cx, |view, cx| view.save_for_close(cx));
        let outcome = save.await;

        assert!(
            outcome
                .as_ref()
                .is_err_and(|message| message.contains("changed on disk")),
            "{outcome:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "an agent got there first\n"
        );
        view.update(cx, |view, cx| {
            assert!(view.is_dirty(), "the file stays open and modified");
            let again = view.save_for_close(cx);
            assert!(view.has_conflict());
            drop(again);
        });

        view.update(cx, |view, _cx| view.discard_unsaved());
        view.update(cx, |view, _cx| {
            assert!(!view.is_dirty(), "Don't Save lets the view go");
        });
    }

    #[gpui::test]
    async fn a_close_time_save_of_a_clean_or_saved_file_succeeds(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        cx.executor().allow_parking();

        let clean = view.update(cx, |view, cx| view.save_for_close(cx));
        assert_eq!(clean.await, Ok(()));

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "two\n", window, cx);
        });
        let save = view.update(cx, |view, cx| view.save_for_close(cx));
        assert_eq!(save.await, Ok(()));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\ntwo\n");
        view.update(cx, |view, _cx| assert!(!view.is_dirty()));
    }

    async fn probe(view: &Entity<CodeView>, cx: &mut VisualTestContext, path: &Path) {
        let weak = view.downgrade();
        let path = path.to_path_buf();
        cx.executor().allow_parking();
        cx.spawn(|mut cx| async move { probe_disk(&weak, &mut cx, &path, false).await })
            .await;
        cx.run_until_parked();
    }

    fn failure_of(view: &CodeView) -> Option<CodeLoadError> {
        match &view.state {
            CodeLoadState::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }

    #[gpui::test]
    async fn a_reload_applies_the_open_guards_instead_of_splicing(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");

        std::fs::write(&path, b"bin\0ary\n").expect("binary write");
        probe(&view, cx, &path).await;
        view.update(cx, |view, _cx| {
            assert_eq!(failure_of(view), Some(CodeLoadError::Binary));
            assert_ne!(view.disk, DiskState::Deleted);
        });

        std::fs::write(&path, [b'o', b'k', 0xff, 0xfe, b'\n']).expect("invalid utf-8");
        probe(&view, cx, &path).await;
        view.update(cx, |view, _cx| {
            assert_eq!(
                failure_of(view),
                Some(CodeLoadError::NotUtf8),
                "invalid UTF-8 reads as unreadable, not as a deleted file"
            );
            assert_ne!(view.disk, DiskState::Deleted);
        });

        let huge = vec![b'x'; 11 * 1024 * 1024];
        std::fs::write(&path, &huge).expect("grow to 11 MiB");
        probe(&view, cx, &path).await;
        view.update(cx, |view, _cx| {
            assert!(
                matches!(failure_of(view), Some(CodeLoadError::TooLarge { .. })),
                "the view switches to the too-large state"
            );
        });

        std::fs::write(&path, "back to text\n").expect("readable again");
        probe(&view, cx, &path).await;
        for _ in 0..200 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| view.document().is_some()) {
                break;
            }
            smol::Timer::after(Duration::from_millis(5)).await;
        }
        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "back to text\n", "a readable file reopens");
        });
    }

    #[gpui::test]
    async fn a_reload_marks_a_giant_line_read_only(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "short\n", false);
        let path = dir.path().join("main.rs");
        let giant = format!("{}\n", "y".repeat(10_001));
        std::fs::write(&path, &giant).expect("giant line");

        probe(&view, cx, &path).await;

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), giant);
            assert!(matches!(
                view.document().and_then(CodeDocument::read_only_reason),
                Some(ReadOnlyReason::GiantLine { .. })
            ));
        });
    }

    #[gpui::test]
    async fn an_unreadable_rewrite_of_a_dirty_buffer_is_a_conflict(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "mine\n", window, cx);
        });
        std::fs::write(&path, b"\0\0\0").expect("binary write");

        probe(&view, cx, &path).await;

        view.update(cx, |view, _cx| {
            assert!(view.has_conflict());
            assert_eq!(text_of(view), "one\nmine\n", "the edits are kept");
        });
    }

    #[gpui::test]
    async fn a_clean_buffer_follows_a_switch_to_crlf_and_saves_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "one\r\ntwo\r\n").expect("crlf rewrite");

        probe(&view, cx, &path).await;

        view.update_in(cx, |view, window, cx| {
            assert_eq!(
                view.document().map(CodeDocument::line_ending),
                Some(LineEnding::Crlf)
            );
            view.selection = CodeSelection::at(8);
            view.replace_text_in_range(None, "three\n", window, cx);
            view.save_action(&CeSave, window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "one\r\ntwo\r\nthree\r\n"
        );
    }

    #[gpui::test]
    async fn a_write_before_the_watcher_starts_is_still_reloaded(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "written while the watcher was starting\n").expect("early write");
        cx.executor().allow_parking();

        view.update(cx, |view, cx| view.start_watcher(cx));
        for _ in 0..300 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| {
                text_of(view) == "written while the watcher was starting\n"
            }) {
                break;
            }
            smol::Timer::after(Duration::from_millis(10)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "written while the watcher was starting\n");
            if let Some(bridge) = view._watch_bridge.take() {
                *bridge.lock().expect("bridge lock") = None;
            }
            view._watcher = None;
        });
    }

    #[gpui::test]
    fn a_write_after_my_save_is_caught_by_the_next_save(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(4);
            view.replace_text_in_range(None, "first\n", window, cx);
            view.save_action(&CeSave, window, cx);
        });
        cx.executor().allow_parking();
        cx.run_until_parked();

        std::fs::write(&path, "another process wrote a longer file\n").expect("external");
        view.update_in(cx, |view, window, cx| {
            view.replace_text_in_range(None, "second\n", window, cx);
            view.save_action(&CeSave, window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "another process wrote a longer file\n"
        );
        view.update(cx, |view, _cx| assert!(view.has_conflict()));
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
                .finish_disk_reload(diff.meta, splices, true, false, cx)
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
                view.finish_disk_reload(again.meta, splices, false, false, cx)
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
                .finish_disk_reload(diff.meta, splices, true, false, cx)
                .expect("a stale revision buys exactly one recomputation");
            let splices = edit::disk_splices(&again.rope, "ONE!\nTWO!\n");
            assert!(
                view.finish_disk_reload(again.meta, splices, false, false, cx)
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
                view.finish_disk_reload(diff.meta, splices, false, true, cx)
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
                reload_from_disk(&weak, &mut cx, 0, read_of(stamp, "one\ntwo\n"), false).await
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
    fn keep_mine_never_overrides_a_write_the_banner_did_not_show(cx: &mut TestAppContext) {
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
            view.resolve_keep_mine(cx);
        });
        std::fs::write(&path, "a later agent write\n").expect("second agent write");

        view.update_in(cx, |view, window, cx| view.save_action(&CeSave, window, cx));
        cx.executor().allow_parking();
        cx.run_until_parked();

        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "a later agent write\n",
            "Keep mine only overrides the version the banner showed"
        );
        view.update(cx, |view, _cx| assert!(view.has_conflict()));
    }

    #[gpui::test]
    fn a_deleted_file_is_flagged_and_saving_recreates_it(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\n", false);
        let path = dir.path().join("main.rs");
        std::fs::remove_file(&path).expect("delete");

        view.update(cx, |view, cx| {
            view.disk_changed(None, None, cx);
            view.buffer_stamp = None;
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
