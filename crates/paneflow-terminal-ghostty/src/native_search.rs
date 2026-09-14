use std::time::{Duration, Instant};

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::{OwnedHandle, check};
use crate::{GhosttyError, MAX_QUERY_LEN, NativeSearchSnapshot, Result, SearchMatch, SearchResult};

pub(crate) struct NativeSearch {
    handle: OwnedHandle<sys::GhosttySearch>,
    query: String,
    initial_selection_pending: bool,
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

    fn select(&mut self, previous: bool) -> Result<()> {
        let option = if previous {
            sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_PREV
        } else {
            sys::GhosttySearchOption_GHOSTTY_SEARCH_OPT_SELECT_NEXT
        };
        let result =
            unsafe { sys::ghostty_search_set(self.handle.raw(), option, std::ptr::null()) };
        if result != sys::GhosttyResult_GHOSTTY_NO_VALUE {
            check("search_select", result)?;
            self.initial_selection_pending = false;
        }
        Ok(())
    }

    fn step(&mut self) -> Result<()> {
        let started = Instant::now();
        check("search_feed", unsafe {
            sys::ghostty_search_feed(self.handle.raw())
        })?;
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
            self.select(false)?;
        }
        Ok(())
    }

    fn snapshot(&self, terminal: &DisplayTerminal) -> Result<NativeSearchSnapshot> {
        let mut count = 0usize;
        check("search_total_matches", unsafe {
            sys::ghostty_search_get(
                self.handle.raw(),
                sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,
                (&raw mut count).cast(),
            )
        })?;
        let mut selections = vec![crate::selection::empty_selection(); count];
        let mut buffer = sys::GhosttySelectionBuffer {
            ptr: selections.as_mut_ptr(),
            cap: selections.len(),
            len: 0,
        };
        check("search_matches", unsafe {
            sys::ghostty_search_get(
                self.handle.raw(),
                sys::GhosttySearchData_GHOSTTY_SEARCH_DATA_MATCHES,
                (&raw mut buffer).cast(),
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
        let selections = selections.get(..buffer.len).ok_or_else(|| {
            GhosttyError::AbiMismatch("search_matches returned a length beyond capacity".into())
        })?;
        let matches = selections
            .iter()
            .map(|selection| {
                Ok(SearchMatch {
                    start: terminal.point_from_grid_ref(&selection.start)?,
                    end: terminal.point_from_grid_ref(&selection.end)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(NativeSearchSnapshot {
            matches,
            selected,
            complete: self.status()? == sys::GhosttySearchStatus_GHOSTTY_SEARCH_STATUS_COMPLETE,
        })
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

    pub fn search_step(&mut self) -> Result<Option<NativeSearchSnapshot>> {
        let Some(search) = &mut self.search else {
            return Ok(None);
        };
        search.step()?;
        self.snapshot_cache.invalidate();
        self.search
            .as_ref()
            .map(|search| search.snapshot(self))
            .transpose()
    }

    pub fn search_select(&mut self, previous: bool) -> Result<()> {
        if let Some(search) = &mut self.search {
            search.select(previous)?;
            self.snapshot_cache.invalidate();
        }
        Ok(())
    }

    pub(crate) fn native_search_once(&self, query: &str) -> Result<SearchResult> {
        if query.is_empty() {
            return Ok(SearchResult::default());
        }
        let search = NativeSearch::new(self.terminal.raw(), query)?;
        check("search_run", unsafe {
            sys::ghostty_search_run(search.handle.raw())
        })?;
        let mut snapshot = search.snapshot(self)?;
        snapshot.matches.reverse();
        Ok(SearchResult {
            matches: snapshot.matches,
            regex_error: None,
            truncated: false,
        })
    }
}
