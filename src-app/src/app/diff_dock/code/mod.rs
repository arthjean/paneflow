//! The editable-file layer of the diff dock (prd-file-editor-2026-Q3).
//!
//! EP-001 is the model half: a path becomes a [`CodeDocument`] off the render
//! thread ([`load`]), hostile files are refused or downgraded to read-only with
//! a written explanation, and [`highlight`] keeps that document colored exactly
//! like the diff colors its rows, incrementally, without a full parse per
//! keystroke. Keeping the model epic free of GPUI rendering is what makes the
//! rope, the guardrails and the highlight driver testable without a window.
//!
//! EP-004 is the editing half: [`edit`] holds the pure primitives (splice,
//! undo history, indent unit, paste sanitization) and [`save`] the atomic write
//! plus the on-disk stamp that detects a concurrent agent write. Both are
//! driven from [`view`], through GPUI actions and the native input handler.
//!
//! EP-002 is the rendering half: [`element`] paints a document through a custom
//! GPUI `Element` that only ever touches the lines on screen, and [`view`] is
//! the entity that hosts it, owns the two scroll axes and drives the recolor
//! after a theme change.
//!
//! EP-006 is the entry point: US-019 lifted the markdown lock in
//! `app/files_sidebar/row.rs`, so a click (or Enter) on any text file in the
//! Files sidebar routes through `PaneFlowApp::open_file_in_diff_dock` into
//! `PaneFlowApp::open_diff_file_tab` and constructs a [`view::CodeView`] here.

pub(crate) mod cursor;
pub(crate) mod document;
pub(crate) mod edit;
pub(crate) mod element;
pub(crate) mod highlight;
pub(crate) mod load;
pub(crate) mod save;
pub(crate) mod view;
