use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::snapshot_ffi::ghostty_point;
use crate::{GhosttyError, Point, Result};

pub struct TrackedRef {
    raw: sys::GhosttyTrackedGridRef,
}

impl TrackedRef {
    #[must_use]
    pub fn is_live(&self) -> bool {
        unsafe { sys::ghostty_tracked_grid_ref_has_value(self.raw) }
    }

    pub fn screen_point(&self) -> Result<Option<(u32, u16)>> {
        let mut coordinate = sys::GhosttyPointCoordinate { x: 0, y: 0 };
        let result = unsafe {
            sys::ghostty_tracked_grid_ref_point(
                self.raw,
                sys::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
                &mut coordinate,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("tracked_grid_ref_point", result)?;
        Ok(Some((coordinate.y, coordinate.x)))
    }

    pub(crate) fn snapshot(&self) -> Result<Option<sys::GhosttyGridRef>> {
        let mut reference: sys::GhosttyGridRef = unsafe { std::mem::zeroed() };
        reference.size = std::mem::size_of::<sys::GhosttyGridRef>();
        let result = unsafe { sys::ghostty_tracked_grid_ref_snapshot(self.raw, &mut reference) };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("tracked_grid_ref_snapshot", result)?;
        Ok(Some(reference))
    }
}

impl Drop for TrackedRef {
    fn drop(&mut self) {
        unsafe { sys::ghostty_tracked_grid_ref_free(self.raw) };
    }
}

impl DisplayTerminal {
    pub fn track(&self, point: Point) -> Result<TrackedRef> {
        let point = self.screen_point(point)?;
        let mut raw: sys::GhosttyTrackedGridRef = std::ptr::null_mut();
        let result =
            unsafe { sys::ghostty_terminal_grid_ref_track(self.terminal.raw(), point, &mut raw) };
        check("terminal_grid_ref_track", result)?;
        if raw.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "terminal_grid_ref_track returned a null handle".into(),
            ));
        }
        Ok(TrackedRef { raw })
    }

    pub fn retrack(&self, reference: &TrackedRef, point: Point) -> Result<()> {
        let point = self.screen_point(point)?;
        let result =
            unsafe { sys::ghostty_tracked_grid_ref_set(reference.raw, self.terminal.raw(), point) };
        check("tracked_grid_ref_set", result)
    }

    pub fn tracked_point(&self, reference: &TrackedRef) -> Result<Option<Point>> {
        let Some(snapshot) = reference.snapshot()? else {
            return Ok(None);
        };
        self.point_from_grid_ref(&snapshot).map(Some)
    }

    fn screen_point(&self, point: Point) -> Result<sys::GhosttyPoint> {
        let scrollback = i64::try_from(self.scrollback_rows()?)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i64".into()))?;
        let screen_y = i64::from(point.line)
            .checked_add(scrollback)
            .ok_or_else(|| GhosttyError::AbiMismatch("tracked point overflow".into()))?;
        if screen_y < 0 {
            return Err(GhosttyError::Ffi {
                operation: "tracked_point_out_of_bounds",
                code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
        ghostty_point(
            sys::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
            usize::try_from(screen_y)
                .map_err(|_| GhosttyError::AbiMismatch("negative tracked point".into()))?,
            point.column,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    fn terminal(cols: usize, rows: usize, scrollback: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, scrollback, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    #[test]
    fn a_tracked_reference_follows_its_row_as_output_scrolls() {
        let mut terminal = terminal(20, 3, 100);
        terminal.feed(b"first\r\n").expect("output must parse");

        let anchor = terminal.track(Point::new(0, 2)).expect("track");
        assert!(anchor.is_live());
        assert_eq!(
            terminal.tracked_point(&anchor).expect("point"),
            Some(Point::new(0, 2))
        );
        let screen = anchor.screen_point().expect("screen point");

        for index in 0..5 {
            terminal
                .feed(format!("line {index}\r\n").as_bytes())
                .expect("output must parse");
        }

        assert_eq!(anchor.screen_point().expect("screen point"), screen);
        let moved = terminal
            .tracked_point(&anchor)
            .expect("point")
            .expect("still live");
        assert!(moved.line < 0, "expected a scrollback row, got {moved:?}");
        assert_eq!(moved.column, 2);
    }

    #[test]
    fn a_reference_survives_heavy_output_at_a_stable_screen_row() {
        let mut terminal = terminal(20, 2, 2);
        terminal.feed(b"anchored\r\n").expect("output must parse");
        let anchor = terminal.track(Point::new(0, 0)).expect("track");
        let screen = anchor.screen_point().expect("screen point");

        for index in 0..50 {
            terminal
                .feed(format!("line {index}\r\n").as_bytes())
                .expect("output must parse");
        }

        assert!(anchor.is_live());
        assert_eq!(anchor.screen_point().expect("screen point"), screen);
        assert_eq!(
            terminal
                .tracked_point(&anchor)
                .expect("point")
                .expect("still live")
                .line,
            -50
        );
    }

    #[test]
    fn clearing_the_scrollback_drops_the_reference() {
        let mut terminal = terminal(20, 3, 100);
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
            .expect("output must parse");
        let anchor = terminal.track(Point::new(-2, 0)).expect("track");
        assert!(anchor.is_live());

        terminal
            .clear_screen_and_scrollback()
            .expect("clear must succeed");

        assert!(
            !anchor.is_live()
                || terminal.tracked_point(&anchor).expect("point").is_none()
                || anchor.screen_point().expect("screen point") == Some((0, 0)),
            "a cleared scrollback must not leave the anchor pointing at live content"
        );
    }

    #[test]
    fn a_reference_can_be_moved_to_a_new_point() {
        let mut terminal = terminal(20, 3, 100);
        terminal.feed(b"one\r\ntwo").expect("output must parse");

        let anchor = terminal.track(Point::new(0, 0)).expect("track");
        terminal
            .retrack(&anchor, Point::new(1, 2))
            .expect("retrack");
        assert_eq!(
            terminal.tracked_point(&anchor).expect("point"),
            Some(Point::new(1, 2))
        );
    }

    #[test]
    fn a_reference_outlives_its_terminal() {
        let anchor = {
            let mut terminal = terminal(20, 3, 100);
            terminal.feed(b"text").expect("output must parse");
            terminal.track(Point::new(0, 1)).expect("track")
        };
        assert!(!anchor.is_live());
    }
}
