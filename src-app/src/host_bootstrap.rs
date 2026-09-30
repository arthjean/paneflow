const CLIENT_NAME: &str = "paneflow-desktop";

fn prune_retired_helpers(home: &std::path::Path, live_host_version: &str) {
    let keep = [env!("CARGO_PKG_VERSION"), live_host_version];
    for removed in paneflow_host::helpers::prune_versioned_helper_dirs(home, &keep) {
        log::info!(
            "paneflow: removed the retired helper directory {}",
            removed.display()
        );
    }
}

pub fn start_in_background() {
    let Some(home) = paneflow_home::paneflow_home() else {
        log::warn!("paneflow: no state home resolved; the local host was not started");
        return;
    };
    let controller = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            log::warn!(
                "paneflow: cannot locate the desktop executable ({error}); the local host was not started"
            );
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("paneflow-host-bootstrap".into())
        .spawn(
            move || match paneflow_host::ensure_host_running(&home, &controller, CLIENT_NAME) {
                Ok(adoption) => {
                    log::info!(
                        "paneflow: local host {} {} for {} (pid {}, endpoint {})",
                        adoption.identity.host_instance,
                        if adoption.started {
                            "started"
                        } else {
                            "adopted"
                        },
                        home.display(),
                        adoption.identity.pid,
                        adoption.identity.endpoint
                    );
                    prune_retired_helpers(&home, &adoption.identity.version);
                }
                Err(error) => log::warn!("paneflow: local host bootstrap failed: {error}"),
            },
        );
    if let Err(error) = spawned {
        log::warn!("paneflow: cannot start the host bootstrap thread: {error}");
    }
}
