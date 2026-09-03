use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::selection::empty_selection;
use crate::{GhosttyError, Point, Result, SelectionRange};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureBehavior {
    #[default]
    Cell,
    Word,
    Line,
    Output,
}

impl GestureBehavior {
    fn raw(self) -> sys::GhosttySelectionGestureBehavior {
        use sys as s;
        match self {
            Self::Cell => s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL,
            Self::Word => s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD,
            Self::Line => s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE,
            Self::Output => {
                s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT
            }
        }
    }

    fn from_raw(value: sys::GhosttySelectionGestureBehavior) -> Result<Self> {
        use sys as s;
        match value {
            s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL => {
                Ok(Self::Cell)
            }
            s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD => {
                Ok(Self::Word)
            }
            s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE => {
                Ok(Self::Line)
            }
            s::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_OUTPUT => {
                Ok(Self::Output)
            }
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown Ghostty selection gesture behavior {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureBehaviors {
    pub single_click: GestureBehavior,
    pub double_click: GestureBehavior,
    pub triple_click: GestureBehavior,
}

impl Default for GestureBehaviors {
    fn default() -> Self {
        Self {
            single_click: GestureBehavior::Cell,
            double_click: GestureBehavior::Word,
            triple_click: GestureBehavior::Line,
        }
    }
}

impl GestureBehaviors {
    fn raw(self) -> sys::GhosttySelectionGestureBehaviors {
        sys::GhosttySelectionGestureBehaviors {
            single_click: self.single_click.raw(),
            double_click: self.double_click.raw(),
            triple_click: self.triple_click.raw(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureGeometry {
    pub columns: u32,
    pub cell_width: u32,
    pub padding_left: u32,
    pub screen_height: u32,
}

impl GestureGeometry {
    fn raw(self) -> sys::GhosttySelectionGestureGeometry {
        sys::GhosttySelectionGestureGeometry {
            columns: self.columns,
            cell_width: self.cell_width,
            padding_left: self.padding_left,
            screen_height: self.screen_height,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureAutoscroll {
    #[default]
    None,
    Up,
    Down,
}

impl GestureAutoscroll {
    fn from_raw(value: sys::GhosttySelectionGestureAutoscroll) -> Result<Self> {
        use sys as s;
        match value {
            s::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE => {
                Ok(Self::None)
            }
            s::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_UP => {
                Ok(Self::Up)
            }
            s::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_DOWN => {
                Ok(Self::Down)
            }
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown Ghostty selection gesture autoscroll {other}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureState {
    pub click_count: u8,
    pub dragged: bool,
    pub autoscroll: GestureAutoscroll,
    pub behavior: GestureBehavior,
    pub anchor: Option<Point>,
}

#[derive(Clone, Debug, Default)]
pub struct PressOptions {
    pub position: Option<(f64, f64)>,
    pub time_ns: Option<u64>,
    pub repeat_distance: Option<f64>,
    pub repeat_interval_ns: Option<u64>,
    pub behaviors: Option<GestureBehaviors>,
    pub word_boundaries: Vec<char>,
}

#[derive(Clone, Debug, Default)]
pub struct DragOptions {
    pub position: Option<(f64, f64)>,
    pub rectangle: bool,
    pub word_boundaries: Vec<char>,
}

pub(crate) struct GestureHandle {
    gesture: sys::GhosttySelectionGesture,
    terminal: sys::GhosttyTerminal,
}

impl GestureHandle {
    pub(crate) fn new(terminal: sys::GhosttyTerminal) -> Result<Self> {
        let mut gesture: sys::GhosttySelectionGesture = std::ptr::null_mut();
        let result = unsafe { sys::ghostty_selection_gesture_new(std::ptr::null(), &mut gesture) };
        check("selection_gesture_new", result)?;
        if gesture.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "selection_gesture_new returned a null handle".into(),
            ));
        }
        Ok(Self { gesture, terminal })
    }

    fn raw(&self) -> sys::GhosttySelectionGesture {
        self.gesture
    }
}

impl Drop for GestureHandle {
    fn drop(&mut self) {
        unsafe { sys::ghostty_selection_gesture_free(self.gesture, self.terminal) };
    }
}

struct GestureEvent {
    raw: sys::GhosttySelectionGestureEvent,
}

impl GestureEvent {
    fn new(kind: sys::GhosttySelectionGestureEventType) -> Result<Self> {
        let mut raw: sys::GhosttySelectionGestureEvent = std::ptr::null_mut();
        let result =
            unsafe { sys::ghostty_selection_gesture_event_new(std::ptr::null(), &mut raw, kind) };
        check("selection_gesture_event_new", result)?;
        if raw.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "selection_gesture_event_new returned a null handle".into(),
            ));
        }
        Ok(Self { raw })
    }

    unsafe fn set(
        &mut self,
        option: sys::GhosttySelectionGestureEventOption,
        value: *const c_void,
    ) -> Result<()> {
        let result = unsafe { sys::ghostty_selection_gesture_event_set(self.raw, option, value) };
        check("selection_gesture_event_set", result)
    }

    fn set_word_boundaries(&mut self, boundaries: &[char]) -> Result<()> {
        if boundaries.is_empty() {
            return Ok(());
        }
        let codepoints: Vec<u32> = boundaries.iter().copied().map(u32::from).collect();
        let value = sys::GhosttyCodepoints {
            ptr: codepoints.as_ptr(),
            len: codepoints.len(),
        };
        unsafe {
            self.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_WORD_BOUNDARY_CODEPOINTS,
                (&raw const value).cast(),
            )
        }
    }

    fn set_position(&mut self, position: Option<(f64, f64)>) -> Result<()> {
        let Some((x, y)) = position else {
            return Ok(());
        };
        let value = sys::GhosttySurfacePosition { x, y };
        unsafe {
            self.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,
                (&raw const value).cast(),
            )
        }
    }
}

impl Drop for GestureEvent {
    fn drop(&mut self) {
        unsafe { sys::ghostty_selection_gesture_event_free(self.raw) };
    }
}

impl DisplayTerminal {
    fn gesture(&mut self) -> Result<sys::GhosttySelectionGesture> {
        let gesture = match self.gesture.as_ref() {
            Some(gesture) => gesture,
            None => self
                .gesture
                .insert(GestureHandle::new(self.terminal.raw())?),
        };
        Ok(gesture.raw())
    }

    fn dispatch(&mut self, event: &GestureEvent) -> Result<Option<SelectionRange>> {
        let gesture = self.gesture()?;
        let mut selection = empty_selection();
        let result = unsafe {
            sys::ghostty_selection_gesture_event(
                gesture,
                self.terminal.raw(),
                event.raw,
                &mut selection,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("selection_gesture_event", result)?;
        self.install_selection(&selection)?;
        self.snapshot_cache.invalidate();
        let range = self.selection_range_of(&selection)?;
        Ok(Some(range))
    }

    fn set_ref(&self, event: &mut GestureEvent, point: Point) -> Result<()> {
        let reference = self.grid_ref(point)?;
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                (&raw const reference).cast(),
            )
        }
    }

    pub fn gesture_press(
        &mut self,
        point: Point,
        options: &PressOptions,
    ) -> Result<Option<SelectionRange>> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS,
        )?;
        self.set_ref(&mut event, point)?;
        event.set_position(options.position)?;
        event.set_word_boundaries(&options.word_boundaries)?;
        if let Some(time_ns) = options.time_ns {
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS,
                    (&raw const time_ns).cast(),
                )?;
            }
        }
        if let Some(distance) = options.repeat_distance {
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE,
                    (&raw const distance).cast(),
                )?;
            }
        }
        if let Some(interval) = options.repeat_interval_ns {
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS,
                    (&raw const interval).cast(),
                )?;
            }
        }
        if let Some(behaviors) = options.behaviors {
            let behaviors = behaviors.raw();
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS,
                    (&raw const behaviors).cast(),
                )?;
            }
        }
        self.dispatch(&event)
    }

    pub fn gesture_drag(
        &mut self,
        point: Point,
        geometry: GestureGeometry,
        options: &DragOptions,
    ) -> Result<Option<SelectionRange>> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG,
        )?;
        self.set_ref(&mut event, point)?;
        let geometry = geometry.raw();
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                (&raw const geometry).cast(),
            )?;
        }
        event.set_position(options.position)?;
        event.set_word_boundaries(&options.word_boundaries)?;
        let rectangle = options.rectangle;
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE,
                (&raw const rectangle).cast(),
            )?;
        }
        self.dispatch(&event)
    }

    pub fn gesture_release(&mut self, point: Option<Point>) -> Result<()> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE,
        )?;
        if let Some(point) = point {
            self.set_ref(&mut event, point)?;
        }
        self.dispatch(&event).map(|_| ())
    }

    pub fn gesture_autoscroll_tick(
        &mut self,
        viewport: Point,
        geometry: GestureGeometry,
        options: &DragOptions,
    ) -> Result<Option<SelectionRange>> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_AUTOSCROLL_TICK,
        )?;
        let coordinate = sys::GhosttyPointCoordinate {
            x: u16::try_from(viewport.column).map_err(|_| GhosttyError::InvalidDimensions {
                cols: viewport.column,
                rows: 0,
                max: u16::MAX,
            })?,
            y: u32::try_from(viewport.line.max(0)).map_err(|_| GhosttyError::LimitExceeded {
                resource: "viewport row",
                limit: u32::MAX as usize,
            })?,
        };
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT,
                (&raw const coordinate).cast(),
            )?;
        }
        let geometry = geometry.raw();
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                (&raw const geometry).cast(),
            )?;
        }
        event.set_position(options.position)?;
        event.set_word_boundaries(&options.word_boundaries)?;
        self.dispatch(&event)
    }

    pub fn gesture_deep_press(&mut self, word_boundaries: &[char]) -> Result<Option<SelectionRange>> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DEEP_PRESS,
        )?;
        event.set_word_boundaries(word_boundaries)?;
        self.dispatch(&event)
    }

    pub fn gesture_reset(&mut self) -> Result<()> {
        let gesture = self.gesture()?;
        unsafe { sys::ghostty_selection_gesture_reset(gesture, self.terminal.raw()) };
        Ok(())
    }

    pub fn gesture_state(&mut self) -> Result<GestureState> {
        let gesture = self.gesture()?;
        let mut click_count = 0u8;
        let mut dragged = false;
        let mut autoscroll =
            sys::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE;
        let mut behavior =
            sys::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL;
        use sys as s;
        unsafe {
            get_multi_gesture(
                gesture,
                self.terminal.raw(),
                [
                    Slot::new(
                        s::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_CLICK_COUNT,
                        &mut click_count,
                    ),
                    Slot::new(
                        s::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_DRAGGED,
                        &mut dragged,
                    ),
                    Slot::new(
                        s::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_AUTOSCROLL,
                        &mut autoscroll,
                    ),
                    Slot::new(
                        s::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_BEHAVIOR,
                        &mut behavior,
                    ),
                ],
            )?;
        }

        let mut anchor: sys::GhosttyGridRef = unsafe { std::mem::zeroed() };
        anchor.size = std::mem::size_of::<sys::GhosttyGridRef>();
        let result = unsafe {
            sys::ghostty_selection_gesture_get(
                gesture,
                self.terminal.raw(),
                s::GhosttySelectionGestureData_GHOSTTY_SELECTION_GESTURE_DATA_ANCHOR,
                (&raw mut anchor).cast(),
            )
        };
        let anchor = if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            None
        } else {
            check("selection_gesture_get_anchor", result)?;
            Some(self.point_from_grid_ref(&anchor)?)
        };

        Ok(GestureState {
            click_count,
            dragged,
            autoscroll: GestureAutoscroll::from_raw(autoscroll)?,
            behavior: GestureBehavior::from_raw(behavior)?,
            anchor,
        })
    }
}

unsafe fn get_multi_gesture<const N: usize>(
    gesture: sys::GhosttySelectionGesture,
    terminal: sys::GhosttyTerminal,
    slots: [Slot<sys::GhosttySelectionGestureData>; N],
) -> Result<()> {
    thread_local! {
        static TERMINAL: std::cell::Cell<sys::GhosttyTerminal> =
            const { std::cell::Cell::new(std::ptr::null_mut()) };
    }

    unsafe extern "C" fn shim(
        gesture: sys::GhosttySelectionGesture,
        count: usize,
        keys: *const sys::GhosttySelectionGestureData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> sys::GhosttyResult {
        let terminal = TERMINAL.with(std::cell::Cell::get);
        unsafe {
            sys::ghostty_selection_gesture_get_multi(
                gesture,
                terminal,
                count,
                keys,
                values,
                out_written,
            )
        }
    }

    TERMINAL.with(|slot| slot.set(terminal));
    let result = unsafe { get_multi("selection_gesture_get_multi", gesture, shim, slots) };
    TERMINAL.with(|slot| slot.set(std::ptr::null_mut()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    const SECOND: u64 = 1_000_000_000;

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    fn geometry(columns: u32) -> GestureGeometry {
        GestureGeometry {
            columns,
            cell_width: 8,
            padding_left: 0,
            screen_height: 64,
        }
    }

    fn timed_press(time_ns: u64) -> PressOptions {
        PressOptions {
            position: Some((0.0, 0.0)),
            time_ns: Some(time_ns),
            repeat_distance: Some(4.0),
            repeat_interval_ns: Some(SECOND / 2),
            behaviors: None,
            word_boundaries: Vec::new(),
        }
    }

    #[test]
    fn a_double_click_selects_the_word_under_the_pointer() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"alpha beta").expect("output must parse");

        terminal
            .gesture_press(Point::new(0, 7), &timed_press(0))
            .expect("first press");
        terminal.gesture_release(Some(Point::new(0, 7))).expect("release");
        terminal
            .gesture_press(Point::new(0, 7), &timed_press(SECOND / 4))
            .expect("second press");

        assert_eq!(terminal.gesture_state().expect("state").click_count, 2);
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert_eq!(text.trim(), "beta");
    }

    #[test]
    fn a_triple_click_selects_the_line() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"alpha beta").expect("output must parse");

        for (index, time) in [0, SECOND / 4, SECOND / 2].into_iter().enumerate() {
            assert!(
                terminal
                    .gesture_press(Point::new(0, 2), &timed_press(time))
                    .is_ok(),
                "press {index} must succeed"
            );
            terminal
                .gesture_release(Some(Point::new(0, 2)))
                .expect("release");
        }

        assert_eq!(terminal.gesture_state().expect("state").click_count, 3);
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert_eq!(text.trim(), "alpha beta");
    }

    #[test]
    fn presses_outside_the_repeat_interval_start_a_new_sequence() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"alpha beta").expect("output must parse");

        terminal
            .gesture_press(Point::new(0, 2), &timed_press(0))
            .expect("first press");
        terminal.gesture_release(Some(Point::new(0, 2))).expect("release");
        terminal
            .gesture_press(Point::new(0, 2), &timed_press(SECOND * 5))
            .expect("late press");

        assert_eq!(terminal.gesture_state().expect("state").click_count, 1);
    }

    #[test]
    fn dragging_extends_the_selection_from_the_press_anchor() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"hello world").expect("output must parse");

        terminal
            .gesture_press(Point::new(0, 0), &PressOptions::default())
            .expect("press");
        let state = terminal.gesture_state().expect("state");
        assert_eq!(state.anchor, Some(Point::new(0, 0)));
        assert!(!state.dragged);

        let range = terminal
            .gesture_drag(Point::new(0, 4), geometry(20), &DragOptions::default())
            .expect("drag")
            .expect("a drag selects");
        assert_eq!(range.start, Point::new(0, 0));
        assert!(terminal.gesture_state().expect("state").dragged);

        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert_eq!(text, "hell");

        terminal
            .gesture_drag(
                Point::new(0, 4),
                geometry(20),
                &DragOptions {
                    position: Some((4.0 * 8.0 + 7.0, 0.0)),
                    ..DragOptions::default()
                },
            )
            .expect("drag past the cell midpoint")
            .expect("a drag selects");
        assert_eq!(
            terminal
                .selection_text()
                .expect("selection text")
                .expect("a selection exists"),
            "hello"
        );
    }

    #[test]
    fn a_custom_behavior_table_changes_what_one_click_selects() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"alpha beta").expect("output must parse");

        let options = PressOptions {
            behaviors: Some(GestureBehaviors {
                single_click: GestureBehavior::Word,
                double_click: GestureBehavior::Line,
                triple_click: GestureBehavior::Line,
            }),
            ..PressOptions::default()
        };
        terminal
            .gesture_press(Point::new(0, 7), &options)
            .expect("press");

        assert_eq!(
            terminal.gesture_state().expect("state").behavior,
            GestureBehavior::Word
        );
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert_eq!(text.trim(), "beta");
    }

    #[test]
    fn resetting_clears_the_click_count_and_anchor() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"hello").expect("output must parse");

        terminal
            .gesture_press(Point::new(0, 1), &timed_press(0))
            .expect("press");
        assert!(terminal.gesture_state().expect("state").click_count > 0);

        terminal.gesture_reset().expect("reset");
        let state = terminal.gesture_state().expect("state");
        assert_eq!(state.click_count, 0);
        assert_eq!(state.anchor, None);
        assert_eq!(state.autoscroll, GestureAutoscroll::None);
    }

    #[test]
    fn a_drag_held_below_the_grid_autoscrolls_and_keeps_extending() {
        let mut terminal = terminal(20, 3);
        terminal
            .feed(b"a\r\nb\r\nc\r\nd\r\ne\r\nf\r\ng\r\nh\r\ni\r\nj")
            .expect("output must parse");
        terminal.scroll(crate::Scroll::Top);

        let geometry = GestureGeometry {
            columns: 20,
            cell_width: 8,
            padding_left: 0,
            screen_height: 48,
        };
        let held_below = DragOptions {
            position: Some((8.0, 60.0)),
            ..DragOptions::default()
        };
        terminal
            .gesture_press(Point::new(-7, 0), &PressOptions::default())
            .expect("press");
        terminal
            .gesture_drag(Point::new(-5, 1), geometry, &held_below)
            .expect("drag");

        assert_eq!(
            terminal.gesture_state().expect("state").autoscroll,
            GestureAutoscroll::Down
        );
        assert_eq!(
            terminal
                .selection_text()
                .expect("selection text")
                .expect("a selection exists"),
            "a\nb\nc"
        );

        terminal.scroll(crate::Scroll::Delta(-1));
        terminal
            .gesture_autoscroll_tick(Point::new(2, 1), geometry, &held_below)
            .expect("autoscroll tick")
            .expect("the tick extends the selection");
        let extended = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");

        assert!(
            extended.starts_with("a\nb\nc\nd"),
            "the tick must hold the anchor and reach further down: {extended:?}"
        );
    }

    #[test]
    fn a_rectangular_drag_is_not_the_same_as_a_linear_one() {
        let mut terminal = terminal(20, 4);
        terminal
            .feed(b"abcdefgh\r\nijklmnop")
            .expect("output must parse");

        terminal
            .gesture_press(Point::new(0, 1), &PressOptions::default())
            .expect("press");
        let linear = terminal
            .gesture_drag(Point::new(1, 3), geometry(20), &DragOptions::default())
            .expect("linear drag")
            .expect("a drag selects");
        assert!(!linear.rectangle);
        let linear_text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");

        terminal.gesture_reset().expect("reset");
        terminal
            .gesture_press(Point::new(0, 1), &PressOptions::default())
            .expect("press");
        let block = terminal
            .gesture_drag(
                Point::new(1, 3),
                geometry(20),
                &DragOptions {
                    rectangle: true,
                    ..DragOptions::default()
                },
            )
            .expect("block drag")
            .expect("a drag selects");
        assert!(block.rectangle);
        let block_text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");

        assert_ne!(linear_text, block_text);
        assert!(block_text.contains("bc"), "got {block_text:?}");
        assert!(block_text.contains("jk"), "got {block_text:?}");
        assert!(!block_text.contains("defgh"), "got {block_text:?}");
        assert!(linear_text.contains("defgh"), "got {linear_text:?}");
    }
}
