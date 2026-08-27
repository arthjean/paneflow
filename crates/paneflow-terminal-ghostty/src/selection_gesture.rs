//! Pointer-driven selection: click, double-click, triple-click, drag, and
//! autoscroll, arbitrated by libghostty rather than reimplemented.
//!
//! A terminal's selection gestures carry more state than they look like they
//! do: the click counter and its timeout, which granularity each click count
//! maps to, the anchor that a drag pivots around, and whether a drag that has
//! left the viewport should scroll it. libghostty owns all of that here, so
//! Paneflow feeds it events and reads back a selection.

use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::selection::empty_selection;
use crate::{GhosttyError, Point, Result, SelectionRange};

/// What a click selects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureBehavior {
    /// Cell-granular selection.
    #[default]
    Cell,
    /// Whole words.
    Word,
    /// Whole lines.
    Line,
    /// The output of a command, using OSC 133 marks.
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

/// Which granularity each click count selects at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureBehaviors {
    /// Single click.
    pub single_click: GestureBehavior,
    /// Double click.
    pub double_click: GestureBehavior,
    /// Triple click.
    pub triple_click: GestureBehavior,
}

impl Default for GestureBehaviors {
    /// Cell, word, line: what every terminal does.
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

/// Where the rendered grid sits on the surface, so a drag can be mapped back
/// onto cells and can tell when it has left the viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureGeometry {
    /// Columns in the rendered grid. Must be non-zero.
    pub columns: u32,
    /// Width of one cell in surface pixels. Must be non-zero.
    pub cell_width: u32,
    /// Left padding before the grid, in surface pixels.
    pub padding_left: u32,
    /// Height of the rendered surface in surface pixels. Must be non-zero.
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

/// Whether an active drag wants the viewport scrolled, and which way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureAutoscroll {
    /// The pointer is inside the viewport.
    #[default]
    None,
    /// The pointer is above the viewport.
    Up,
    /// The pointer is below the viewport.
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

/// The live state of a selection gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GestureState {
    /// How many clicks the current sequence has accumulated. Zero when no
    /// gesture is active.
    pub click_count: u8,
    /// Whether the pointer has moved since the press.
    pub dragged: bool,
    /// Whether the drag is asking for the viewport to scroll.
    pub autoscroll: GestureAutoscroll,
    /// The granularity the current click count resolved to.
    pub behavior: GestureBehavior,
    /// The cell the gesture pivots around, if one is active.
    pub anchor: Option<Point>,
}

/// Optional details of a press.
#[derive(Clone, Debug, Default)]
pub struct PressOptions {
    /// Surface-space pointer position, used with `repeat_distance` to decide
    /// whether this press continues the previous click sequence.
    pub position: Option<(f64, f64)>,
    /// Monotonic event time. Without it a press is untimed and can only ever
    /// be a single click.
    pub time_ns: Option<u64>,
    /// How far the pointer may move between clicks and still count as a
    /// repeat, in surface pixels.
    pub repeat_distance: Option<f64>,
    /// How long a click sequence stays open, in nanoseconds.
    pub repeat_interval_ns: Option<u64>,
    /// Which granularity each click count selects at.
    pub behaviors: Option<GestureBehaviors>,
    /// Codepoints that separate words. Empty keeps libghostty's defaults.
    pub word_boundaries: Vec<char>,
}

/// Optional details of a drag.
#[derive(Clone, Debug, Default)]
pub struct DragOptions {
    /// Surface-space pointer position.
    pub position: Option<(f64, f64)>,
    /// Whether the drag builds a rectangular (block) selection.
    pub rectangle: bool,
    /// Codepoints that separate words. Empty keeps libghostty's defaults.
    pub word_boundaries: Vec<char>,
}

/// A gesture handle bound to the terminal that created it.
///
/// `ghostty_selection_gesture_free` needs the terminal, so this stores the
/// terminal handle alongside the gesture. It is declared before the terminal
/// in [`DisplayTerminal`], which makes the drop order correct by
/// construction.
pub(crate) struct GestureHandle {
    gesture: sys::GhosttySelectionGesture,
    terminal: sys::GhosttyTerminal,
}

impl GestureHandle {
    pub(crate) fn new(terminal: sys::GhosttyTerminal) -> Result<Self> {
        let mut gesture: sys::GhosttySelectionGesture = std::ptr::null_mut();
        // SAFETY: the null allocator selects libghostty's default and
        // `gesture` is valid writable storage.
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
        // SAFETY: both handles are live: the terminal outlives this field
        // because it is declared after it in `DisplayTerminal`.
        unsafe { sys::ghostty_selection_gesture_free(self.gesture, self.terminal) };
    }
}

/// A gesture event, owned for the length of one dispatch.
struct GestureEvent {
    raw: sys::GhosttySelectionGestureEvent,
}

impl GestureEvent {
    fn new(kind: sys::GhosttySelectionGestureEventType) -> Result<Self> {
        let mut raw: sys::GhosttySelectionGestureEvent = std::ptr::null_mut();
        // SAFETY: the null allocator selects libghostty's default and `raw`
        // is valid writable storage.
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

    /// Set one option. `value` must point at the type the option documents,
    /// or be null to clear it.
    ///
    /// # Safety
    ///
    /// `value` must match `option`'s documented input type and stay live for
    /// the call.
    unsafe fn set(
        &mut self,
        option: sys::GhosttySelectionGestureEventOption,
        value: *const c_void,
    ) -> Result<()> {
        // SAFETY: the caller guarantees the value type; the handle is live.
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
        // SAFETY: the option takes a `GhosttyCodepoints*`, the codepoints
        // outlive the call, and libghostty copies them into event storage.
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
        // SAFETY: the option takes a `GhosttySurfacePosition*` and `value`
        // outlives the call.
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
        // SAFETY: `raw` came from `selection_gesture_event_new`, is private,
        // and Drop runs exactly once.
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

    /// Apply `event`, install any resulting selection, and return it.
    ///
    /// A `None` result means the event produced no selection: a release, or a
    /// press on a cell with nothing to select. The terminal's selection is
    /// left alone in that case rather than cleared, because a release must
    /// not wipe what the press selected.
    fn dispatch(&mut self, event: &GestureEvent) -> Result<Option<SelectionRange>> {
        let gesture = self.gesture()?;
        let mut selection = empty_selection();
        // SAFETY: all three handles are live and `selection` is valid
        // writable storage with its `size` field set.
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
        // SAFETY: the option takes a `GhosttyGridRef*` and `reference`
        // outlives the call.
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,
                (&raw const reference).cast(),
            )
        }
    }

    /// Press at `point`, advancing the click counter.
    ///
    /// With a `time_ns` and a `repeat_interval_ns` in `options`, consecutive
    /// presses on the same spot become double- and triple-clicks and select
    /// at the matching granularity.
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
            // SAFETY: the option takes a `uint64_t*` that outlives the call.
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_TIME_NS,
                    (&raw const time_ns).cast(),
                )?;
            }
        }
        if let Some(distance) = options.repeat_distance {
            // SAFETY: the option takes a `double*` that outlives the call.
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_DISTANCE,
                    (&raw const distance).cast(),
                )?;
            }
        }
        if let Some(interval) = options.repeat_interval_ns {
            // SAFETY: the option takes a `uint64_t*` that outlives the call.
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REPEAT_INTERVAL_NS,
                    (&raw const interval).cast(),
                )?;
            }
        }
        if let Some(behaviors) = options.behaviors {
            let behaviors = behaviors.raw();
            // SAFETY: the option takes a `GhosttySelectionGestureBehaviors*`
            // that outlives the call.
            unsafe {
                event.set(
                    sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS,
                    (&raw const behaviors).cast(),
                )?;
            }
        }
        self.dispatch(&event)
    }

    /// Drag to `point`, extending the selection from the press anchor at the
    /// granularity the click count chose.
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
        // SAFETY: the option takes a `GhosttySelectionGestureGeometry*` that
        // outlives the call.
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,
                (&raw const geometry).cast(),
            )?;
        }
        event.set_position(options.position)?;
        event.set_word_boundaries(&options.word_boundaries)?;
        let rectangle = options.rectangle;
        // SAFETY: the option takes a `bool*` that outlives the call.
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE,
                (&raw const rectangle).cast(),
            )?;
        }
        self.dispatch(&event)
    }

    /// Release the pointer, closing the drag but keeping the click counter so
    /// a following press can become a double-click.
    ///
    /// `point` is where the release happened, or `None` when it landed
    /// outside any cell.
    pub fn gesture_release(&mut self, point: Option<Point>) -> Result<()> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_RELEASE,
        )?;
        if let Some(point) = point {
            self.set_ref(&mut event, point)?;
        }
        // A release never yields a selection; it only advances gesture state.
        self.dispatch(&event).map(|_| ())
    }

    /// Advance an autoscrolling drag by one tick.
    ///
    /// Call this while [`Self::gesture_state`] reports an autoscroll
    /// direction, after scrolling the viewport, passing the viewport
    /// coordinate the pointer now maps to.
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
        // SAFETY: the option takes a `GhosttyPointCoordinate*` that outlives
        // the call.
        unsafe {
            event.set(
                sys::GhosttySelectionGestureEventOption_GHOSTTY_SELECTION_GESTURE_EVENT_OPT_VIEWPORT,
                (&raw const coordinate).cast(),
            )?;
        }
        let geometry = geometry.raw();
        // SAFETY: the option takes a `GhosttySelectionGestureGeometry*` that
        // outlives the call.
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

    /// Deepen the current selection one granularity step, the way a
    /// force-touch or a long press does.
    pub fn gesture_deep_press(&mut self, word_boundaries: &[char]) -> Result<Option<SelectionRange>> {
        let mut event = GestureEvent::new(
            sys::GhosttySelectionGestureEventType_GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DEEP_PRESS,
        )?;
        event.set_word_boundaries(word_boundaries)?;
        self.dispatch(&event)
    }

    /// Drop the click counter and the anchor, ending any gesture in flight.
    pub fn gesture_reset(&mut self) -> Result<()> {
        let gesture = self.gesture()?;
        // SAFETY: both handles are live.
        unsafe { sys::ghostty_selection_gesture_reset(gesture, self.terminal.raw()) };
        Ok(())
    }

    /// The gesture's live state.
    pub fn gesture_state(&mut self) -> Result<GestureState> {
        let gesture = self.gesture()?;
        let mut click_count = 0u8;
        let mut dragged = false;
        let mut autoscroll =
            sys::GhosttySelectionGestureAutoscroll_GHOSTTY_SELECTION_GESTURE_AUTOSCROLL_NONE;
        let mut behavior =
            sys::GhosttySelectionGestureBehavior_GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL;
        use sys as s;
        // The anchor is read on its own because it reports
        // `GHOSTTY_NO_VALUE` when no gesture is active, which would abort the
        // whole batch.
        // SAFETY: every destination matches the output type selection.h
        // documents for its key, and all of them outlive the call.
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
        // SAFETY: the key writes a `GhosttyGridRef*` with its `size` set, and
        // both handles are live.
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

/// `ghostty_selection_gesture_get_multi` takes the terminal as a second
/// handle, so it does not fit [`get_multi`]'s shape directly. This binds the
/// terminal and hands the rest through.
///
/// # Safety
///
/// Both handles must be live and every slot must satisfy [`Slot::new`]'s
/// contract.
unsafe fn get_multi_gesture<const N: usize>(
    gesture: sys::GhosttySelectionGesture,
    terminal: sys::GhosttyTerminal,
    slots: [Slot<sys::GhosttySelectionGestureData>; N],
) -> Result<()> {
    // Thread-local rather than a parameter: the shim below is a plain `fn`
    // pointer, so the terminal has to reach it out of band. Gestures are
    // single-threaded per terminal, which this crate already enforces by
    // making `DisplayTerminal` neither `Send` nor `Sync`.
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
        // SAFETY: the caller of `get_multi_gesture` guarantees the handles
        // and the arrays; `get_multi` built them from the slots.
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
    // SAFETY: the caller guarantees the handles and slots; `shim` forwards to
    // the real entry point with the terminal it just stored.
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

        // Without a surface position the drag stops at the cell before the
        // pointer: libghostty needs the pixel offset to decide whether the
        // pointer is past the middle of its cell.
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
        // A block selection keeps the rows separate; the linear one runs
        // through the end of the first row.
        assert!(!block_text.contains("defgh"), "got {block_text:?}");
        assert!(linear_text.contains("defgh"), "got {linear_text:?}");
    }
}
