use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerBoot {
    Pending,
    Ready {
        pid: u32,
        protocol: u32,
        capabilities: Vec<String>,
        replaced: Option<String>,
    },
    Failed(String),
}

static STATE: Mutex<WorkerBoot> = Mutex::new(WorkerBoot::Pending);

pub fn state() -> WorkerBoot {
    STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn record(next: WorkerBoot) {
    *STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
}

pub fn start_in_background() {
    record(WorkerBoot::Pending);
    let Some(home) = paneflow_home::paneflow_home() else {
        record(WorkerBoot::Failed(
            "no Paneflow state home resolved; set PANEFLOW_HOME".to_string(),
        ));
        return;
    };
    let controller = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            record(WorkerBoot::Failed(format!(
                "cannot locate the Paneflow executable: {error}"
            )));
            return;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("paneflow-worker-bootstrap".into())
        .spawn(
            move || match paneflow_serve::ensure_worker_running(&home, &controller) {
                Ok(adoption) => {
                    log::info!(
                        "paneflow: worker {} {} for {} (pid {}, protocol {})",
                        adoption.identity.version,
                        if adoption.started {
                            "started"
                        } else {
                            "adopted"
                        },
                        home.display(),
                        adoption.identity.pid,
                        adoption.identity.protocol
                    );
                    record(WorkerBoot::Ready {
                        pid: adoption.identity.pid,
                        protocol: adoption.identity.protocol,
                        capabilities: adoption.identity.capabilities.clone(),
                        replaced: adoption.replaced.clone(),
                    });
                }
                Err(error) => {
                    log::warn!("paneflow: worker bootstrap failed: {error}");
                    record(WorkerBoot::Failed(error.to_string()));
                }
            },
        );
    if let Err(error) = spawned {
        record(WorkerBoot::Failed(format!(
            "cannot start the worker bootstrap thread: {error}"
        )));
    }
}

pub fn retry() {
    start_in_background();
}

pub fn banner(
    boot: &WorkerBoot,
    _reconnecting: bool,
    _stale: bool,
    _reason: Option<&str>,
) -> Option<(String, bool)> {
    if let WorkerBoot::Failed(error) = boot {
        return Some((error.clone(), true));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_names_the_exact_failure_and_offers_retry_only_then() {
        let failed = WorkerBoot::Failed("the endpoint is already in use".to_string());
        let (message, retryable) = banner(&failed, false, false, None).expect("a failure shows");
        assert_eq!(message, "the endpoint is already in use");
        assert!(retryable, "a failed bootstrap offers Retry");

        let ready = WorkerBoot::Ready {
            pid: 7,
            protocol: paneflow_serve::WORKER_PROTOCOL_VERSION,
            capabilities: Vec::new(),
            replaced: None,
        };
        assert_eq!(banner(&ready, false, false, None), None);

        assert_eq!(
            banner(&ready, true, true, Some("stream closed")),
            None,
            "an automatic reconnection stays out of the workspace list"
        );

        assert_eq!(
            banner(&ready, true, false, Some("stream closed")),
            None,
            "a connection that never bootstrapped is not a reconnection"
        );
    }

    #[test]
    fn a_failed_bootstrap_is_reported_verbatim_so_no_pane_opens_dead() {
        record(WorkerBoot::Failed("endpoint in use".to_string()));
        assert_eq!(state(), WorkerBoot::Failed("endpoint in use".to_string()));
        record(WorkerBoot::Ready {
            pid: 7,
            protocol: paneflow_serve::WORKER_PROTOCOL_VERSION,
            capabilities: vec!["agent.snapshot".to_string()],
            replaced: Some("0.15.0".to_string()),
        });
        assert!(matches!(state(), WorkerBoot::Ready { pid: 7, .. }));
        record(WorkerBoot::Pending);
    }
}
