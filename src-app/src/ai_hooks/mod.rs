pub mod extract;

use std::io::Write;

pub(crate) fn hook_diag(msg: &str) {
    let Some(path) = std::env::var_os("PANEFLOW_HOOK_LOG") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let line = format!("paneflow-app[{}]: {msg}\n", std::process::id());
    let _ = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}
