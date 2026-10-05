mod catalog;
mod fuzzy;
mod history;
mod index;
mod listing;
#[cfg(test)]
mod perf_bench;
mod render;
mod search;
pub(in crate::terminal) mod wsl;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Pixels, Subscription, UniformListScrollHandle, Window,
};

use crate::app::diff_dock::code::spawn_blocking_then;
use crate::terminal::types::ShellQuoting;
use crate::widgets::text_input::TextInput;
use catalog::{Discovery, Shared};
use index::{GLOBAL_CAP, LOCAL_CAP};
use listing::{Cancel, Listing, Query, Recent, Request, Row, Status};
use wsl::WslRoots;

pub(crate) enum PathPickerEvent {
    Picked(String),
    Dismissed { refocus: bool },
}

pub(crate) struct PathPicker {
    input: Entity<TextInput>,
    base: Option<PathBuf>,
    home: Option<PathBuf>,
    wsl: Option<Arc<WslRoots>>,
    located: bool,
    scoped: bool,
    quoting: ShellQuoting,
    anchor: Bounds<Pixels>,
    catalog: Shared,
    sealed: Arc<[PathBuf]>,
    recents: Arc<[Recent]>,
    walks_cancelled: Arc<AtomicBool>,
    navigation: Option<(PathBuf, Arc<AtomicBool>)>,
    generation: Arc<AtomicU64>,
    listing: Listing,
    query: String,
    selected: usize,
    above: bool,
    reveal_selected: bool,
    scroll: UniformListScrollHandle,
    _subscriptions: [Subscription; 2],
}

impl EventEmitter<PathPickerEvent> for PathPicker {}

impl Focusable for PathPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle.clone()
    }
}

impl PathPicker {
    pub(crate) fn new(
        cwd: Option<String>,
        quoting: ShellQuoting,
        anchor: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_catalog(cwd, quoting, anchor, catalog::machine(), window, cx)
    }

    fn with_catalog(
        cwd: Option<String>,
        quoting: ShellQuoting,
        anchor: Bounds<Pixels>,
        catalog: Shared,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new("", "", cx).with_accessible_name("Filter paths"));
        let focus = input.read(cx).focus_handle.clone();
        let subscriptions = [
            cx.observe(&input, |picker, input, cx| {
                let query = input.read(cx).value();
                if query != picker.query {
                    picker.query = query;
                    picker.refresh(cx);
                }
            }),
            cx.on_focus_out(&focus, window, |_, _, _, cx| {
                cx.emit(PathPickerEvent::Dismissed { refocus: false });
            }),
        ];
        catalog::lock(&catalog).opened();
        #[cfg(not(test))]
        release_when_idle(&catalog, cx);
        let mut picker = Self {
            input,
            base: None,
            home: None,
            wsl: None,
            located: false,
            scoped: false,
            quoting,
            anchor,
            catalog,
            sealed: Arc::from([]),
            recents: Arc::from([]),
            walks_cancelled: Arc::new(AtomicBool::new(false)),
            navigation: None,
            generation: Arc::new(AtomicU64::new(0)),
            listing: Listing::Status(Status::Indexing),
            query: String::new(),
            selected: 0,
            above: false,
            reveal_selected: false,
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        if quoting == ShellQuoting::Wsl {
            spawn_blocking_then(cx, wsl::roots, move |picker: &mut Self, roots, cx| {
                let base = cwd.and_then(|cwd| match &roots {
                    Some(roots) if cwd.starts_with('/') => Some(roots.to_windows(&cwd)),
                    _ if cwd.starts_with('/') => None,
                    _ => Some(PathBuf::from(cwd)),
                });
                let home = roots.as_ref().map(|roots| roots.home());
                picker.locate(base, home, roots, cx);
            });
        } else {
            picker.locate(cwd.map(PathBuf::from), dirs::home_dir(), None, cx);
        }
        picker
    }

    fn locate(
        &mut self,
        base: Option<PathBuf>,
        home: Option<PathBuf>,
        wsl: Option<Arc<WslRoots>>,
        cx: &mut Context<Self>,
    ) {
        let placeholder = listing::placeholder(base.as_deref(), home.as_deref(), wsl.as_deref());
        self.input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        let mut homes: Vec<PathBuf> = dirs::home_dir().into_iter().collect();
        if let Some(home) = home.as_ref().filter(|home| !homes.contains(home)) {
            homes.push(home.clone());
        }
        self.sealed = homes
            .iter()
            .flat_map(|home| index::platform_data_dirs(home))
            .collect();
        self.base = base;
        self.home = home;
        self.wsl = wsl;
        self.located = true;
        if let Some(base) = self.base.clone() {
            self.visit(base, self.walks_cancelled.clone(), cx);
        }
        let catalog = self.catalog.clone();
        spawn_blocking_then(
            cx,
            move || discover(&catalog, &homes),
            |picker: &mut Self, recents, cx| {
                picker.recents = recents.into();
                picker.scoped = true;
                picker.refresh(cx);
                picker.load_globals(cx);
            },
        );
    }

    fn rows(&self) -> &[Row] {
        match &self.listing {
            Listing::Rows(rows) => rows,
            Listing::Status(_) => &[],
        }
    }

    fn navigate_into(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if let Some((current, cancelled)) = &self.navigation {
            if index::same_path(current, &dir) && !cancelled.load(Ordering::Relaxed) {
                return;
            }
            cancelled.store(true, Ordering::Relaxed);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.navigation = Some((dir.clone(), cancelled.clone()));
        self.visit(dir, cancelled, cx);
    }

    fn visit(&mut self, root: PathBuf, cancelled: Arc<AtomicBool>, cx: &mut Context<Self>) {
        if !catalog::lock(&self.catalog).begin_visit(&root, &cancelled) {
            return;
        }
        let catalog = self.catalog.clone();
        let sealed = self.sealed.clone();
        spawn_blocking_then(
            cx,
            move || {
                let index = index::walk(&root, LOCAL_CAP, &sealed, &cancelled);
                catalog::lock(&catalog).finish_visit(&root, &cancelled, index);
            },
            |picker: &mut Self, (), cx| picker.refresh_in_background(cx),
        );
    }

    fn load_globals(&mut self, cx: &mut Context<Self>) {
        let store = {
            let mut catalog = catalog::lock(&self.catalog);
            if catalog.begin_disk_read() {
                catalog.store()
            } else {
                None
            }
        };
        let Some(store) = store else {
            self.walk_globals(cx);
            return;
        };
        let catalog = self.catalog.clone();
        spawn_blocking_then(
            cx,
            move || {
                let indexes = catalog::read_store(&store);
                catalog::lock(&catalog).finish_disk_read(indexes);
            },
            |picker: &mut Self, (), cx| {
                picker.refresh_in_background(cx);
                picker.walk_globals(cx);
            },
        );
    }

    fn walk_globals(&mut self, cx: &mut Context<Self>) {
        let walk = Arc::new(AtomicBool::new(false));
        let (root, store) = {
            let mut catalog = catalog::lock(&self.catalog);
            let Some(root) = catalog.next_stale(SystemTime::now(), &walk) else {
                return;
            };
            (root, catalog.store())
        };
        let catalog = self.catalog.clone();
        let sealed = self.sealed.clone();
        spawn_blocking_then(
            cx,
            move || {
                let index = index::walk(&root, GLOBAL_CAP, &sealed, &walk);
                let persisted = catalog::lock(&catalog).finish_global(&root, &walk, index);
                if let Some(store) = store {
                    catalog::write_store(&store, &persisted);
                }
            },
            |picker: &mut Self, (), cx| {
                picker.refresh_in_background(cx);
                picker.walk_globals(cx);
            },
        );
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.recompute(false, cx);
    }

    fn refresh_in_background(&mut self, cx: &mut Context<Self>) {
        if listing::parse(&self.query) != Query::Browse {
            self.recompute(true, cx);
        }
    }

    fn recompute(&mut self, keep_selection: bool, cx: &mut Context<Self>) {
        if !self.located || !self.scoped {
            return;
        }
        let mine = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let snapshot = catalog::lock(&self.catalog).snapshot(self.base.as_deref());
        let request = Request {
            query: self.query.clone(),
            base: self.base.clone(),
            home: self.home.clone(),
            wsl: self.wsl.clone(),
            local: snapshot.local,
            globals: snapshot.globals,
            visited: snapshot.visited,
            recents: self.recents.clone(),
            pending: snapshot.pending,
        };
        let cancel = Cancel::new(self.generation.clone(), mine);
        spawn_blocking_then(
            cx,
            move || listing::compute(&request, &cancel),
            move |picker: &mut Self, outcome, cx| {
                if picker.generation.load(Ordering::Relaxed) != mine {
                    return;
                }
                let Some(outcome) = outcome else {
                    return;
                };
                let kept = keep_selection
                    .then(|| {
                        picker
                            .rows()
                            .get(picker.selected)
                            .map(|row| row.path.clone())
                    })
                    .flatten();
                picker.listing = outcome.listing;
                picker.selected = kept
                    .and_then(|path| picker.rows().iter().position(|row| row.path == path))
                    .unwrap_or(0);
                picker.reveal_selected = true;
                cx.notify();
                if let Some(dir) = outcome.walk {
                    picker.navigate_into(dir, cx);
                }
            },
        );
    }

    fn step(&mut self, away_from_field: bool, cx: &mut Context<Self>) {
        let last = self.rows().len().saturating_sub(1);
        let next = if away_from_field {
            (self.selected + 1).min(last)
        } else {
            self.selected.saturating_sub(1)
        };
        if next != self.selected {
            self.selected = next;
            self.reveal_selected = true;
            cx.notify();
        }
    }

    fn complete(&mut self, cx: &mut Context<Self>) {
        let Some(completion) = self
            .rows()
            .get(self.selected)
            .map(|row| row.completion.clone())
        else {
            return;
        };
        self.input
            .update(cx, |input, cx| input.set_value(completion, cx));
    }

    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows().get(index) else {
            return;
        };
        let Some(text) = listing::insertion_text(
            &row.path,
            self.base.as_deref(),
            self.quoting,
            self.wsl.as_deref(),
        ) else {
            return;
        };
        let path = row.path.clone();
        spawn_blocking_then(cx, move || history::record(path), |_: &mut Self, (), _| {});
        cx.emit(PathPickerEvent::Picked(text));
    }

    fn handle_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => cx.emit(PathPickerEvent::Dismissed { refocus: true }),
            "enter" => self.pick(self.selected, cx),
            "tab" => self.complete(cx),
            "up" => self.step(self.above, cx),
            "down" => self.step(!self.above, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Drop for PathPicker {
    fn drop(&mut self) {
        self.walks_cancelled.store(true, Ordering::Relaxed);
        if let Some((_, cancelled)) = &self.navigation {
            cancelled.store(true, Ordering::Relaxed);
        }
        catalog::lock(&self.catalog).closed();
    }
}

fn discover(catalog: &Shared, homes: &[PathBuf]) -> Vec<Recent> {
    let discovery = catalog::lock(catalog).discovery();
    let (roots, recents) = match discovery {
        Discovery::Machine => {
            let projects: Vec<PathBuf> = crate::app::recents::load()
                .into_iter()
                .map(|workspace| workspace.path)
                .collect();
            let extra: Vec<PathBuf> = paneflow_home::worktrees_dir().into_iter().collect();
            (
                catalog::global_roots(homes, &projects, &extra, &std::env::temp_dir()),
                history::recent(),
            )
        }
        Discovery::Fixed(roots) => (roots, Vec::new()),
    };
    catalog::lock(catalog).set_roots(roots);
    listing::existing_recents(recents)
}

#[cfg(not(test))]
fn release_when_idle(catalog: &Shared, cx: &mut Context<PathPicker>) {
    const PERIOD: std::time::Duration = std::time::Duration::from_secs(60);
    if !catalog::lock(catalog).claim_janitor() {
        return;
    }
    let catalog = catalog.clone();
    cx.background_spawn(async move {
        loop {
            smol::Timer::after(PERIOD).await;
            if catalog::lock(&catalog)
                .release_if_idle(catalog::IDLE_RELEASE, std::time::Instant::now())
            {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::MAIN_SEPARATOR;
    use std::rc::Rc;
    use std::sync::Mutex;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;
    use catalog::Catalog;

    fn open(
        cx: &mut TestAppContext,
        base: PathBuf,
        catalog: Shared,
    ) -> (Entity<PathPicker>, &mut VisualTestContext) {
        let (picker, cx) = cx.add_window_view(|window, cx| {
            PathPicker::with_catalog(
                Some(base.to_string_lossy().into_owned()),
                ShellQuoting::Posix,
                Bounds::default(),
                catalog,
                window,
                cx,
            )
        });
        picker.update_in(cx, |picker, window, cx| {
            let focus = picker.focus_handle(cx);
            window.focus(&focus, cx);
        });
        cx.run_until_parked();
        (picker, cx)
    }

    fn isolated(roots: Vec<PathBuf>, store: Option<PathBuf>) -> Shared {
        Arc::new(Mutex::new(Catalog::new(Discovery::Fixed(roots), store)))
    }

    #[gpui::test]
    fn tab_completes_the_selected_folder_and_browses_into_it(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("src")).expect("src");
        std::fs::write(dir.path().join("src").join("main.rs"), b"").expect("main.rs");
        let (picker, cx) = open(cx, dir.path().to_path_buf(), isolated(Vec::new(), None));

        cx.simulate_input("sr");
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert_eq!(picker.rows()[0].label, "src");
        });

        cx.simulate_keystrokes("tab");
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert_eq!(picker.query, format!("src{MAIN_SEPARATOR}"));
            let labels: Vec<&str> = picker.rows().iter().map(|row| row.label.as_str()).collect();
            assert_eq!(labels, [format!("src{MAIN_SEPARATOR}main.rs")]);
        });
    }

    #[gpui::test]
    fn arrows_move_the_selection_and_escape_asks_to_refocus_the_terminal(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), b"").expect("a.txt");
        std::fs::write(dir.path().join("b.txt"), b"").expect("b.txt");
        let (picker, cx) = open(cx, dir.path().to_path_buf(), isolated(Vec::new(), None));
        let dismissed = Rc::new(RefCell::new(Vec::new()));
        let sink = dismissed.clone();
        cx.update(|_, cx| {
            cx.subscribe(&picker, move |_, event: &PathPickerEvent, _| {
                if let PathPickerEvent::Dismissed { refocus } = event {
                    sink.borrow_mut().push(*refocus);
                }
            })
            .detach();
        });

        cx.simulate_keystrokes("down");
        picker.read_with(cx, |picker, _| assert_eq!(picker.selected, 1));
        cx.simulate_keystrokes("down");
        picker.read_with(cx, |picker, _| assert_eq!(picker.selected, 1));
        cx.simulate_keystrokes("up");
        picker.read_with(cx, |picker, _| assert_eq!(picker.selected, 0));

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert_eq!(*dismissed.borrow(), [true]);
    }

    #[gpui::test]
    fn a_search_reaches_a_global_root_and_its_index_is_stored(cx: &mut TestAppContext) {
        let workdir = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let cache = tempfile::tempdir().expect("tempdir");
        std::fs::write(workdir.path().join("main.rs"), b"").expect("main.rs");
        std::fs::create_dir(elsewhere.path().join("notes")).expect("notes");
        std::fs::write(elsewhere.path().join("notes").join("todo.md"), b"").expect("todo.md");
        let store = cache.path().join("path-index.bin");
        let catalog = isolated(vec![elsewhere.path().to_path_buf()], Some(store.clone()));
        let (picker, cx) = open(cx, workdir.path().to_path_buf(), catalog);

        cx.simulate_input("todo");
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert_eq!(
                picker.rows()[0].path,
                elsewhere.path().join("notes").join("todo.md")
            );
        });
        let stored = catalog::read_store(&store);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].root(), elsewhere.path());
    }

    #[gpui::test]
    fn a_navigated_folder_is_searched_below_once_it_is_walked(cx: &mut TestAppContext) {
        let workdir = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let deep = elsewhere.path().join("a").join("b");
        std::fs::create_dir_all(&deep).expect("deep");
        std::fs::write(deep.join("needle.txt"), b"").expect("needle");
        let (picker, cx) = open(cx, workdir.path().to_path_buf(), isolated(Vec::new(), None));

        let typed = format!("{}{MAIN_SEPARATOR}need", elsewhere.path().display());
        cx.simulate_input(&typed);
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert_eq!(picker.rows().len(), 1);
            assert_eq!(picker.rows()[0].path, deep.join("needle.txt"));
        });
    }
}
