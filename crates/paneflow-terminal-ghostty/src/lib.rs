#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(ghostty_native)]
mod encode;
#[cfg(all(test, ghostty_native))]
mod encode_tests;
mod error;
mod input;
#[cfg(ghostty_native)]
mod input_map;
#[cfg(ghostty_native)]
mod limits;
mod model;
#[cfg(ghostty_native)]
mod osc7;
mod search;

macro_rules! native_modules {
    ($($module:ident),+ $(,)?) => {
        $(
            #[cfg(ghostty_native)]
            mod $module;
        )+
    };
}

native_modules!(
    abi,
    abi_layout,
    batch,
    callback_ffi,
    callbacks,
    color,
    constructor,
    engine,
    grid,
    grid_ref,
    handles,
    modes,
    navigation,
    persistence,
    sgr,
    snapshot,
    snapshot_cell,
    snapshot_ffi,
    snapshot_state,
    style,
    unicode,
);

#[cfg(not(ghostty_native))]
mod stub;

#[cfg(ghostty_native)]
pub use color::{
    PALETTE_LEN, PaletteMask, contrast, default_palette, encode_color_scheme_report,
    generate_palette, luminance, parse as parse_color, parse_palette_entry, parse_x11,
    perceived_luminance, x11_names,
};
pub use error::{GhosttyError, Result};
#[cfg(ghostty_native)]
pub use grid_ref::{CellContent, CellInfo, RowInfo, SemanticContent, SemanticPrompt};
pub use input::{
    FocusEvent, Key, KeyAction, KeyInput, Modifiers, MouseAction, MouseButton, MouseInput,
};
pub use model::{
    BackendEvent, Cell, CellFlags, Color, ColorScheme, Content, Cursor, CursorShape, Hyperlink,
    Modes, Point, ProgressReport, ProgressState, Rgb, Scroll, SearchMatch, SearchResult,
    SelectionRange, TerminalAppearance, UnderlineStyle, WideCell, WindowSize,
};
#[cfg(ghostty_native)]
pub use modes::{Mode, ModeReportState, encode_mode_report};
pub use search::{
    MAX_QUERY_LEN, MAX_SEARCH_CELLS, SEARCH_CHUNK_CELLS, SearchChunk, SearchEngine, SearchLine,
};
pub use sgr::{SgrAttribute, SgrParser, SgrSeparator};
#[cfg(ghostty_native)]
pub use style::Style;
#[cfg(ghostty_native)]
pub use unicode::{GraphemeCluster, codepoint_width, grapheme_width, text_width};

#[cfg(ghostty_native)]
pub const GHOSTTY_APP_VERSION: &str = paneflow_libghostty_sys::GHOSTTY_APP_VERSION;

#[cfg(ghostty_native)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    pub source_sha: &'static str,
    pub api_version: &'static str,
    pub zig_version: &'static str,
    pub optimization: &'static str,
    pub simd: &'static str,
}

#[cfg(ghostty_native)]
pub fn build_identity() -> BuildIdentity {
    const MANIFEST: &str = include_str!("../../../native/libghostty/manifest.toml");

    fn value(key: &str) -> Option<&'static str> {
        let prefix = format!("{key} = \"");
        MANIFEST
            .lines()
            .find_map(|line| line.strip_prefix(&prefix)?.strip_suffix('"'))
    }

    BuildIdentity {
        source_sha: value("source_sha").unwrap_or("unknown"),
        api_version: paneflow_libghostty_sys::EXPECTED_API_VERSION,
        zig_version: value("zig_version").unwrap_or("unknown"),
        optimization: value("build_mode").unwrap_or("unknown"),
        simd: value("simd_profile").unwrap_or("unknown"),
    }
}

#[cfg(ghostty_native)]
pub use engine::DisplayTerminal;
#[cfg(not(ghostty_native))]
pub use stub::DisplayTerminal;

#[cfg(all(test, ghostty_native))]
mod identity_tests {
    #[test]
    fn build_identity_is_derived_from_the_pinned_manifest() {
        let identity = super::build_identity();
        assert_eq!(identity.source_sha.len(), 40);
        assert_eq!(identity.api_version, "0.1.0");
        assert_eq!(identity.zig_version, "0.16.0");
        assert_eq!(identity.optimization, "ReleaseFast");
        assert_eq!(identity.simd, "upstream-default");
    }
}
