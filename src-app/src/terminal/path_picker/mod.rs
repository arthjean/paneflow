mod fuzzy;
mod history;
mod listing;
mod render;
mod wsl;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Pixels, Subscription, UniformListScrollHandle, Window,
};

use crate::app::diff_dock::code::spawn_blocking_then;
use crate::terminal::types::ShellQuoting;
use crate::widgets::text_input::TextInput;
use listing::{IndexEntry, Listing, Query, Request, Row, Status};
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
    quoting: ShellQuoting,
    anchor: Bounds<Pixels>,
    index: Option<Arc<[IndexEntry]>>,
    indexing_cancelled: Arc<AtomicBool>,
    listing: Listing,
    query: String,
    selected: usize,
    generation: u64,
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
        let mut picker = Self {
            input,
            base: None,
            home: None,
            wsl: None,
            located: false,
            quoting,
            anchor,
            index: None,
            indexing_cancelled: Arc::new(AtomicBool::new(false)),
            listing: Listing::Status(Status::Indexing),
            query: String::new(),
            selected: 0,
            generation: 0,
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
        self.base = base;
        self.home = home;
        self.wsl = wsl;
        self.located = true;
        self.start_indexing(cx);
        self.refresh(cx);
    }

    fn rows(&self) -> &[Row] {
        match &self.listing {
            Listing::Rows(rows) => rows,
            Listing::Status(_) => &[],
        }
    }

    fn start_indexing(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.base.clone() else {
            return;
        };
        let cancelled = self.indexing_cancelled.clone();
        spawn_blocking_then(
            cx,
            move || listing::build_index(&root, &cancelled),
            |picker: &mut Self, index, cx| {
                picker.index = Some(index.into());
                if matches!(listing::parse(&picker.query), Query::Search(_)) {
                    picker.refresh(cx);
                }
            },
        );
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.located {
            return;
        }
        self.generation += 1;
        let generation = self.generation;
        let request = Request {
            query: self.query.clone(),
            base: self.base.clone(),
            home: self.home.clone(),
            wsl: self.wsl.clone(),
            index: self.index.clone(),
        };
        spawn_blocking_then(
            cx,
            move || listing::compute(&request, &history::recent()),
            move |picker: &mut Self, outcome, cx| {
                if picker.generation != generation {
                    return;
                }
                picker.listing = outcome;
                picker.selected = 0;
                picker.reveal_selected = true;
                cx.notify();
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
        self.indexing_cancelled.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::MAIN_SEPARATOR;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};

    use super::*;

    fn open(
        cx: &mut TestAppContext,
        base: PathBuf,
    ) -> (Entity<PathPicker>, &mut VisualTestContext) {
        let (picker, cx) = cx.add_window_view(|window, cx| {
            PathPicker::new(
                Some(base.to_string_lossy().into_owned()),
                ShellQuoting::Posix,
                Bounds::default(),
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

    #[gpui::test]
    fn tab_completes_the_selected_folder_and_browses_into_it(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("src")).expect("src");
        std::fs::write(dir.path().join("src").join("main.rs"), b"").expect("main.rs");
        let (picker, cx) = open(cx, dir.path().to_path_buf());

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
        let (picker, cx) = open(cx, dir.path().to_path_buf());
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
}
