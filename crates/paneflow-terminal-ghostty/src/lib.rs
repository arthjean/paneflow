#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod encode;
#[cfg(all(
    test,
    feature = "native",
    any(target_os = "linux", target_os = "windows")
))]
mod encode_tests;
mod error;
mod input;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod input_map;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod limits;
mod model;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod osc52;

#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod abi;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod abi_layout;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod callback_ffi;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod callbacks;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod color_query;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod constructor;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod engine;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod grid;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod handles;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod navigation;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod persistence;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod search;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod snapshot;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod snapshot_cell;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod snapshot_ffi;
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
mod snapshot_state;
#[cfg(not(all(feature = "native", any(target_os = "linux", target_os = "windows"))))]
mod stub;

pub use error::{GhosttyError, Result};
pub use input::{
    FocusEvent, Key, KeyAction, KeyInput, Modifiers, MouseAction, MouseButton, MouseInput,
};
pub use model::{
    BackendEvent, Cell, CellFlags, Color, Content, Cursor, CursorShape, Hyperlink, Modes, Point,
    Rgb, Scroll, SearchMatch, SearchResult, SelectionRange, UnderlineStyle, WideCell, WindowSize,
};
#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
pub const GHOSTTY_APP_VERSION: &str = paneflow_libghostty_sys::GHOSTTY_APP_VERSION;

#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildIdentity {
    pub source_sha: &'static str,
    pub api_version: &'static str,
    pub zig_version: &'static str,
    pub optimization: &'static str,
    pub simd: &'static str,
}

#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
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

#[cfg(all(feature = "native", any(target_os = "linux", target_os = "windows")))]
pub use engine::DisplayTerminal;
#[cfg(not(all(feature = "native", any(target_os = "linux", target_os = "windows"))))]
pub use stub::DisplayTerminal;

#[cfg(all(
    test,
    feature = "native",
    any(target_os = "linux", target_os = "windows")
))]
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
