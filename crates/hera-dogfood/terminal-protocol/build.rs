use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=PANEFLOW_HERA_TERMINAL_ROOT");

    if env::var_os("CARGO_FEATURE_HERA_DOGFOOD").is_none() {
        return;
    }

    let hera_root = hera_root();
    require_file(&hera_root, "crates/terminal-protocol/src/lib.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    let generated = format!(
        r#"
#[path = "{protocol}"]
mod terminal_protocol_impl;
pub use terminal_protocol_impl::*;
"#,
        protocol = rust_path(&hera_root, "crates/terminal-protocol/src/lib.rs"),
    );

    let path = out_dir.join("hera_protocol.rs");
    if let Err(error) = fs::write(&path, generated) {
        fail(&format!("failed to write {}: {error}", path.display()));
    }
}

fn hera_root() -> PathBuf {
    env::var_os("PANEFLOW_HERA_TERMINAL_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default())
                .join("../../../..")
                .join("hera-terminal")
        })
}

fn require_file(root: &Path, relative: &str) {
    let path = root.join(relative);
    if !path.is_file() {
        fail(&format!(
            "hera-dogfood requires local Hera source at {}. Set PANEFLOW_HERA_TERMINAL_ROOT or keep Hera checked out next to Paneflow.",
            root.display()
        ));
    }
}

fn rust_path(root: &Path, relative: &str) -> String {
    root.join(relative).to_string_lossy().replace('\\', "/")
}

fn fail(message: &str) -> ! {
    eprintln!("terminal-protocol dogfood build error: {message}");
    std::process::exit(1);
}
