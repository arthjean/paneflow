#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod cwd;
pub mod env_expand;
pub mod pty_env;
pub mod request;
pub mod shell;
pub mod space;

pub use pty_env::PROTECTED_ENV_KEYS;
pub use request::{DEFAULT_SIZE, Launch, LaunchHome, LaunchSpec, create_request, resolve};

#[cfg(test)]
mod tests {
    #[test]
    fn the_crate_links_neither_gpui_nor_the_window() {
        let manifest = include_str!("../Cargo.toml");
        for line in manifest.lines() {
            let name = line.split(['=', '.', ' ']).next().unwrap_or_default();
            assert!(
                !name.starts_with("gpui") && name != "paneflow-app",
                "paneflow-launch must stay usable without a window: {line}"
            );
        }
    }
}
