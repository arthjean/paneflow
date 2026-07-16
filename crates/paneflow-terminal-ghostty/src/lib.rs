#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(target_os = "linux", feature = "native"))]
mod encode;
#[cfg(all(test, target_os = "linux", feature = "native"))]
mod encode_tests;
mod error;
mod input;
#[cfg(all(target_os = "linux", feature = "native"))]
mod input_map;
#[cfg(all(target_os = "linux", feature = "native"))]
mod limits;
mod model;
#[cfg(all(target_os = "linux", feature = "native"))]
mod osc52;

#[cfg(all(target_os = "linux", feature = "native"))]
mod abi;
#[cfg(all(target_os = "linux", feature = "native"))]
mod abi_layout;
#[cfg(all(target_os = "linux", feature = "native"))]
mod callback_ffi;
#[cfg(all(target_os = "linux", feature = "native"))]
mod callbacks;
#[cfg(all(target_os = "linux", feature = "native"))]
mod color_query;
#[cfg(all(target_os = "linux", feature = "native"))]
mod constructor;
#[cfg(all(target_os = "linux", feature = "native"))]
mod engine;
#[cfg(all(target_os = "linux", feature = "native"))]
mod grid;
#[cfg(all(target_os = "linux", feature = "native"))]
mod handles;
#[cfg(all(target_os = "linux", feature = "native"))]
mod navigation;
#[cfg(all(target_os = "linux", feature = "native"))]
mod persistence;
#[cfg(all(target_os = "linux", feature = "native"))]
mod search;
#[cfg(all(target_os = "linux", feature = "native"))]
mod snapshot;
#[cfg(all(target_os = "linux", feature = "native"))]
mod snapshot_cell;
#[cfg(all(target_os = "linux", feature = "native"))]
mod snapshot_ffi;
#[cfg(all(target_os = "linux", feature = "native"))]
mod snapshot_state;
#[cfg(not(all(target_os = "linux", feature = "native")))]
mod stub;

pub use error::{GhosttyError, Result};
pub use input::{
    FocusEvent, Key, KeyAction, KeyInput, Modifiers, MouseAction, MouseButton, MouseInput,
};
pub use model::{
    BackendEvent, Cell, CellFlags, Color, Content, Cursor, CursorShape, Hyperlink, Modes, Point,
    Rgb, Scroll, SearchMatch, SearchResult, SelectionRange, UnderlineStyle, WideCell, WindowSize,
};
#[cfg(all(target_os = "linux", feature = "native"))]
pub const GHOSTTY_APP_VERSION: &str = paneflow_libghostty_sys::GHOSTTY_APP_VERSION;

#[cfg(all(target_os = "linux", feature = "native"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    pub source_sha: &'static str,
    pub api_version: &'static str,
    pub zig_version: &'static str,
    pub optimization: &'static str,
    pub simd: &'static str,
}

#[cfg(all(target_os = "linux", feature = "native"))]
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

#[cfg(all(target_os = "linux", feature = "native"))]
pub use engine::DisplayTerminal;
#[cfg(not(all(target_os = "linux", feature = "native")))]
pub use stub::DisplayTerminal;

#[cfg(all(test, target_os = "linux", feature = "native"))]
mod identity_tests {
    #[test]
    fn build_identity_is_derived_from_the_pinned_manifest() {
        let identity = super::build_identity();
        assert_eq!(identity.source_sha.len(), 40);
        assert_eq!(identity.api_version, "0.1.0");
        assert_eq!(identity.zig_version, "0.15.2");
        assert_eq!(identity.optimization, "ReleaseFast");
        assert_eq!(identity.simd, "upstream-default");
    }
}
