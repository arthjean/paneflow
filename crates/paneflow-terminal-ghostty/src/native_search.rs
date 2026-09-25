use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::{OwnedHandle, check};
use crate::{GhosttyError, MAX_QUERY_LEN, NativeSearchSnapshot, Result, SearchMatch};

pub(crate) struct NativeSearch {
    handle: OwnedHandle<sys::GhosttySearch>,
    query: String,
    initial_selection_pending: bool,
    rail_dirty: bool,
    feed_dirty: bool,
    rail_total: usize,
    rail_offsets: Arc<[usize]>,
    viewport: Option<(usize, usize)>,
}

impl NativeSearch {
    fn new(terminal: sys::GhosttyTerminal, query: &str) -> Result<Self> {
        validate_query(query)?;
        let mut raw = std::ptr::null_mut();
        check("search_new", unsafe {
            sys::ghostty_search_new(std::ptr::null(), &mut raw, terminal)
        })?;
        if raw.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "search_new returned a null handle".into(),
            ));
        }
        let mut search = Self {
            handle: unsafe { OwnedHandle::from_raw(raw, sys::ghostty_search_free) },
            query: String::new(),
            initial_selection_pending: true,
            rail_dirty: true,
            feed_dirty: true,
            rail_total: 0,
            rail_offsets: Arc::default(),
            viewport: None,
        };
        search.set_query(query)?;
        Ok(search)
    }

    fn set_query(&mut self, query: &str) -> Result<()> {
        validate_query(query)?;
        let needle = sys::GhosttyString {
            ptr: query.as_ptr(),
            len: query.len(),
        };
        check("search_set_needle", unsafe {
            sys::ghostty_search_set(
                self.handle.raw(),
                sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_NEEDLE,
                (&raw const needle).cast(),
            )
        })?;
        if !self.query.eq_ignore_ascii_case(query) {
            self.initial_selection_pending = true;
            self.rail_dirty = true;
            self.feed_dirty = true;
            self.query = query.to_owned();
        }
        Ok(())
    }

    fn status(&self) -> Result<sys::GhosttySearchStatus> {
        let mut status = sys::GhosttySearchStatus_GHOSTTY_SEARCH_STATUS_FEED_REQUIRED;
        check("search_status", unsafe {
            sys::ghostty_search_get(
                self.handle.raw(),
                sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_STATUS,
                (&raw mut status).cast(),
            )
        })?;
        Ok(status)
    }

    fn select(&mut self, option: sys::GhosttySearchOption) -> Result<()> {
        let result =
            unsafe { sys::ghostty_search_set(self.handle.raw(), option, std::ptr::null()) };
        if result != sys::GhosttyResult_GHOSTTY_NO_VALUE {
            check("search_select", result)?;
            self.initial_selection_pending = false;
        }
        Ok(())
    }

    fn step(&mut self, viewport: (usize, usize)) -> Result<()> {
        let started = Instant::now();
        if self.feed_dirty || self.viewport != Some(viewport) {
            check("search_feed", unsafe {
                sys::ghostty_search_feed(self.handle.raw())
            })?;
            self.feed_dirty = false;
        }
        let mut status = self.status()?;
        while status != sys::GhosttySearchStatus_GHOSTTY_SEARCH_STATUS_COMPLETE
            && started.elapsed() < Duration::from_millis(2)
        {
            match status {
                sys::GhosttySearchStatus_GHOSTTY_SEARCH_STATUS_RUNNING => {
                    check("search_tick", unsafe {
                        sys::ghostty_search_tick(self.handle.raw(), &mut status)
                    })?;
                }
                sys::GhosttySearchStatus_GHOSTTY_SEARCH_STATUS_FEED_REQUIRED => {
                    check("search_feed", unsafe {
                        sys::ghostty_search_feed(self.handle.raw())
                    })?;
                    status = self.status()?;
                }
                other => {
                    return Err(GhosttyError::AbiMismatch(format!(
                        "unknown search status {other}"
                    )));
                }
            }
        }
        if self.initial_selection_pending {
            self.select(sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_NEXT)?;
            check("search_feed_selected_viewport", unsafe {
                sys::ghostty_search_feed(self.handle.raw())
            })?;
        }
        Ok(())
    }

    fn selections(&self, data: sys::GhosttySearchData) -> Result<Vec<sys::GhosttySelection>> {
        let mut buffer = sys::GhosttySelectionBuffer {
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        let result = unsafe {
            sys::ghostty_search_get(self.handle.raw(), data, (&raw mut buffer).cast())
        };
        if result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            check("search_selections_size", result)?;
        }
        let mut selections = vec![crate::selection::empty_selection(); buffer.len];
        if selections.is_empty() {
            return Ok(selections);
        }
        buffer.ptr = selections.as_mut_ptr();
        buffer.cap = selections.len();
        check("search_selections", unsafe {
            sys::ghostty_search_get(self.handle.raw(), data, (&raw mut buffer).cast())
        })?;
        if buffer.len > selections.len() {
            return Err(GhosttyError::AbiMismatch(
                "search selections exceed buffer capacity".into(),
            ));
        }
        selections.truncate(buffer.len);
        Ok(selections)
    }

    fn snapshot(&mut self, terminal: &DisplayTerminal, refresh_rail: bool) -> Result<NativeSearchSnapshot> {
        let mut count = 0usize;
        check("search_total_matches", unsafe {
            sys::ghostty_search_get(
                self.handle.raw(),
                sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,
                (&raw mut count).cast(),
            )
        })?;
        let mut selected = 0usize;
        let result = unsafe {
            sys::ghostty_search_get(
                self.handle.raw(),
                sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_SELECTED_INDEX,
                (&raw mut selected).cast(),
            )
        };
        let selected = if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            None
        } else {
            check("search_selected_index", result)?;
            Some(selected)
        };
        let viewport = terminal.scrollbar_position()?;
        self.viewport = Some(viewport);
        let (_, display_offset) = viewport;
        let first_row = -(display_offset as i64);
        let last_row = first_row + i64::from(terminal.callbacks.size().rows) - 1;
        let mut viewport_matches = self
            .selections(sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_VIEWPORT_MATCHES)?
            .iter()
            .map(|selection| match_from_selection(terminal, selection))
            .collect::<Result<Vec<_>>>()?;
        viewport_matches.retain(|found| {
            i64::from(found.start.line) <= last_row && i64::from(found.end.line) >= first_row
        });
        let selected_match = if selected.is_some() {
            let mut selection = crate::selection::empty_selection();
            check("search_selected_match", unsafe {
                sys::ghostty_search_get(
                    self.handle.raw(),
                    sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_SELECTED_MATCH,
                    (&raw mut selection).cast(),
                )
            })?;
            Some(match_from_selection(terminal, &selection)?)
        } else {
            None
        };
        if count == 0 {
            self.rail_offsets = Arc::default();
            self.rail_total = 0;
            self.rail_dirty = false;
        }
        if refresh_rail && (self.rail_dirty || self.rail_total != count) {
            let selections = self.selections(sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_MATCHES)?;
            let mut page_lines = HashMap::new();
            let bottom = i64::from(terminal.callbacks.size().rows.saturating_sub(1));
            let mut offsets = Vec::with_capacity(selections.len());
            for selection in &selections {
                let reference = &selection.start;
                let page_line = match page_lines.entry(reference.node) {
                    std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        let point = terminal.point_from_grid_ref(reference)?;
                        *entry.insert(i64::from(point.line) - i64::from(reference.y))
                    }
                };
                let line = page_line + i64::from(reference.y);
                offsets.push(usize::try_from((bottom - line).max(0)).map_err(|_| {
                    GhosttyError::AbiMismatch("search rail offset overflow".into())
                })?);
            }
            offsets.sort_unstable();
            offsets.dedup();
            self.rail_offsets = offsets.into();
            self.rail_total = count;
            self.rail_dirty = false;
        }
        let selected = selected.map(|index| top_down_index(count, index));
        Ok(NativeSearchSnapshot {
            total_matches: count,
            viewport_matches,
            selected_match,
            rail_offsets: self.rail_offsets.clone(),
            rail_pending: self.rail_dirty || self.rail_total != count,
            selected,
            complete: self.status()? == sys::GhosttySearchStatus_GHOSTTY_SEARCH_STATUS_COMPLETE,
        })
    }
}

fn match_from_selection(
    terminal: &DisplayTerminal,
    selection: &sys::GhosttySelection,
) -> Result<SearchMatch> {
    Ok(SearchMatch {
        start: terminal.point_from_grid_ref(&selection.start)?,
        end: terminal.point_from_grid_ref(&selection.end)?,
    })
}

fn top_down_index(len: usize, newest_first_index: usize) -> usize {
    len.saturating_sub(1).saturating_sub(newest_first_index)
}

fn top_down_selection_option(previous: bool) -> sys::GhosttySearchOption {
    if previous {
        sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_NEXT
    } else {
        sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_PREV
    }
}

fn validate_query(query: &str) -> Result<()> {
    if query.len() > MAX_QUERY_LEN {
        return Err(GhosttyError::LimitExceeded {
            resource: "search query",
            limit: MAX_QUERY_LEN,
        });
    }
    Ok(())
}

impl DisplayTerminal {
    pub(crate) fn invalidate_search_rail(&mut self) {
        if let Some(search) = &mut self.search {
            search.rail_dirty = true;
            search.feed_dirty = true;
        }
    }

    pub fn set_search_query(&mut self, query: &str) -> Result<()> {
        if query.is_empty() {
            self.clear_search();
            return Ok(());
        }
        if let Some(search) = &mut self.search {
            search.set_query(query)
        } else {
            self.search = Some(NativeSearch::new(self.terminal.raw(), query)?);
            Ok(())
        }
    }

    pub fn clear_search(&mut self) {
        self.search = None;
    }

    pub fn search_step_with_rail_refresh(&mut self, refresh_rail: bool) -> Result<Option<NativeSearchSnapshot>> {
        let viewport = self.scrollbar_position()?;
        let Some(mut search) = self.search.take() else {
            return Ok(None);
        };
        let result = search.step(viewport).and_then(|()| search.snapshot(self, refresh_rail));
        self.search = Some(search);
        result.map(Some)
    }

    pub fn search_select(&mut self, previous: bool) -> Result<()> {
        if let Some(search) = &mut self.search {
            search.select(top_down_selection_option(previous))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_newest_first_index_maps_to_top_down_order() {
        assert_eq!(top_down_index(3, 0), 2);
        assert_eq!(top_down_index(3, 2), 0);
        assert_eq!(top_down_index(0, 0), 0);
    }

    #[test]
    fn previous_walks_toward_older_matches_and_next_toward_newer() {
        assert_eq!(
            top_down_selection_option(true),
            sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_NEXT
        );
        assert_eq!(
            top_down_selection_option(false),
            sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_PREV
        );
    }
}
