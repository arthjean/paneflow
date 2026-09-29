pub mod extract;

pub(crate) fn hook_diag(msg: &str) {
    let Some(path) = std::env::var_os(paneflow_ipc_client::hook_log::HOOK_LOG_ENV) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let line = format!("paneflow-app[{}]: {msg}\n", std::process::id());
    let _ = paneflow_ipc_client::hook_log::append(std::path::Path::new(&path), &line);
}
