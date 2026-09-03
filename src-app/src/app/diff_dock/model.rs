use std::collections::HashSet;
use std::rc::Rc;

use gpui::Pixels;

use super::git::DiffDockBuilt;
use crate::diff::{
    DisplayRow, FileDiff, FileRowCache, FileSpan, SplitRow, apply_collapse_split,
    apply_collapse_unified, apply_expanded_split_with_sources, apply_expanded_unified_with_sources,
    split_file_spans, split_max_line_no, split_offsets, unified_file_spans, unified_max_line_no,
    unified_offsets,
};

pub(crate) const DIFF_DOCK_PANEL_WIDTH: f32 = 880.0;

pub(crate) const DIFF_DOCK_PANEL_MIN_WIDTH: f32 = 360.0;
pub(super) const DIFF_DOCK_PANEL_MAX_WIDTH: f32 = 1400.0;

pub(crate) const MAX_DIFF_FILE_TABS: usize = 8;

#[derive(Clone)]
pub(crate) enum DiffDockTab {
    Changes,
    Terminal(gpui::Entity<crate::terminal::TerminalView>),
    File(gpui::Entity<super::code::view::CodeView>),
    PendingFile,
}

pub(super) struct DiffChrome<'a> {
    pub(super) data: &'a Option<DiffDockData>,
    pub(super) cwd: String,
    pub(super) split: bool,
    pub(super) options_open: bool,
    pub(super) layout_submenu_open: bool,
    pub(super) collapsed: &'a HashSet<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct DiffDockHScrollDrag {
    pub(super) offset_idx: usize,
    pub(super) start_mouse_x: Pixels,
    pub(super) start_offset: f32,
    pub(super) max_scroll: f32,
    pub(super) track_width: f32,
    pub(super) thumb_width: f32,
}

#[derive(Clone)]
pub(crate) struct DiffDockData {
    pub(crate) cwd: String,
    pub(super) loading: bool,
    pub(super) error: Option<String>,
    pub(super) unified_loaded: bool,
    pub(super) split_loaded: bool,
    pub(super) unified: Rc<Vec<DisplayRow>>,
    pub(super) split: Rc<Vec<SplitRow>>,
    pub(super) anchors_unified: Rc<Vec<(String, usize)>>,
    pub(super) anchors_split: Rc<Vec<(String, usize)>>,
    pub(super) files_full: Rc<Vec<FileDiff>>,
    pub(super) row_caches: Rc<Vec<FileRowCache>>,
    pub(super) disp_unified: Rc<Vec<DisplayRow>>,
    pub(super) disp_split: Rc<Vec<SplitRow>>,
    pub(super) disp_anchors_unified: Rc<Vec<(String, usize)>>,
    pub(super) disp_anchors_split: Rc<Vec<(String, usize)>>,
    pub(super) disp_unified_offsets: Rc<Vec<f32>>,
    pub(super) disp_split_offsets: Rc<Vec<f32>>,
    pub(super) disp_unified_max_no: u32,
    pub(super) disp_split_max_no: u32,
    pub(super) disp_unified_spans: Rc<Vec<FileSpan>>,
    pub(super) disp_split_spans: Rc<Vec<FileSpan>>,
    pub(super) paths: Vec<String>,
    pub(super) file_count: usize,
    pub(super) added: u32,
    pub(super) removed: u32,
    pub(super) theme_generation: u64,
    pub(super) fingerprint: u64,
}

impl DiffDockData {
    pub(super) fn loading(cwd: String) -> Self {
        Self {
            cwd,
            loading: true,
            error: None,
            unified_loaded: false,
            split_loaded: false,
            unified: Rc::new(Vec::new()),
            split: Rc::new(Vec::new()),
            anchors_unified: Rc::new(Vec::new()),
            anchors_split: Rc::new(Vec::new()),
            files_full: Rc::new(Vec::new()),
            row_caches: Rc::new(Vec::new()),
            disp_unified: Rc::new(Vec::new()),
            disp_split: Rc::new(Vec::new()),
            disp_anchors_unified: Rc::new(Vec::new()),
            disp_anchors_split: Rc::new(Vec::new()),
            disp_unified_offsets: Rc::new(vec![0.0]),
            disp_split_offsets: Rc::new(vec![0.0]),
            disp_unified_max_no: 0,
            disp_split_max_no: 0,
            disp_unified_spans: Rc::new(Vec::new()),
            disp_split_spans: Rc::new(Vec::new()),
            paths: Vec::new(),
            file_count: 0,
            added: 0,
            removed: 0,
            theme_generation: crate::theme::theme_generation(),
            fingerprint: 0,
        }
    }

    pub(super) fn message(cwd: String, error: String) -> Self {
        let mut data = Self::loading(cwd);
        data.loading = false;
        data.error = Some(error);
        data
    }

    pub(super) fn recompute(&mut self, collapsed: &HashSet<String>, expanded: &HashSet<String>) {
        if self.unified_loaded {
            self.recompute_unified(collapsed, expanded);
        }
        if self.split_loaded {
            self.recompute_split(collapsed, expanded);
        }
    }

    pub(super) fn has_mode(&self, split: bool) -> bool {
        if split {
            self.split_loaded
        } else {
            self.unified_loaded
        }
    }

    pub(super) fn apply_built(
        &mut self,
        built: DiffDockBuilt,
        collapsed: &HashSet<String>,
        expanded: &HashSet<String>,
    ) {
        self.loading = false;
        self.error = None;
        self.paths = built.paths;
        self.file_count = built.file_count;
        self.added = built.added;
        self.removed = built.removed;
        self.theme_generation = built.theme_generation;
        self.fingerprint = built.fingerprint;
        self.files_full = Rc::new(built.files_full);
        self.row_caches = Rc::new(built.row_caches);

        self.unified = Rc::new(built.unified);
        self.anchors_unified = Rc::new(built.anchors_unified);
        self.split = Rc::new(built.split);
        self.anchors_split = Rc::new(built.anchors_split);
        self.unified_loaded = true;
        self.split_loaded = true;
        self.recompute_unified(collapsed, expanded);
        self.recompute_split(collapsed, expanded);
    }

    fn recompute_unified(&mut self, collapsed: &HashSet<String>, expanded: &HashSet<String>) {
        let (rows, anchors) = if collapsed.is_empty() {
            (
                self.unified.as_ref().clone(),
                self.anchors_unified.as_ref().clone(),
            )
        } else {
            apply_collapse_unified(&self.unified, &self.anchors_unified, collapsed)
        };
        let (rows, anchors) = if expanded.is_empty() {
            (rows, anchors)
        } else {
            apply_expanded_unified_with_sources(
                &rows,
                &anchors,
                expanded,
                self.files_full.as_ref(),
                self.row_caches.as_ref(),
            )
        };
        self.disp_unified = Rc::new(rows);
        self.disp_anchors_unified = Rc::new(anchors);
        self.disp_unified_offsets = Rc::new(unified_offsets(&self.disp_unified));
        self.disp_unified_max_no = unified_max_line_no(&self.disp_unified);
        self.disp_unified_spans = Rc::new(unified_file_spans(&self.disp_unified));
    }

    fn recompute_split(&mut self, collapsed: &HashSet<String>, expanded: &HashSet<String>) {
        let (rows, anchors) = if collapsed.is_empty() {
            (
                self.split.as_ref().clone(),
                self.anchors_split.as_ref().clone(),
            )
        } else {
            apply_collapse_split(&self.split, &self.anchors_split, collapsed)
        };
        let (rows, anchors) = if expanded.is_empty() {
            (rows, anchors)
        } else {
            apply_expanded_split_with_sources(
                &rows,
                &anchors,
                expanded,
                self.files_full.as_ref(),
                self.row_caches.as_ref(),
            )
        };
        self.disp_split = Rc::new(rows);
        self.disp_anchors_split = Rc::new(anchors);
        self.disp_split_offsets = Rc::new(split_offsets(&self.disp_split));
        self.disp_split_max_no = split_max_line_no(&self.disp_split);
        self.disp_split_spans = Rc::new(split_file_spans(&self.disp_split));
    }

    pub(super) fn paths(&self) -> Vec<String> {
        self.paths.clone()
    }

    pub(super) fn all_collapsed(&self, collapsed: &HashSet<String>) -> bool {
        !self.paths.is_empty() && self.paths.iter().all(|p| collapsed.contains(p))
    }
}
