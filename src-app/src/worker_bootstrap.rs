use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerBoot {
    Pending,
    Ready,
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
                    record(WorkerBoot::Ready);
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

pub fn banner(boot: &WorkerBoot) -> Option<String> {
    match boot {
        WorkerBoot::Failed(error) => Some(error.clone()),
        WorkerBoot::Pending | WorkerBoot::Ready => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_names_the_exact_failure_and_only_a_failure() {
        let failed = WorkerBoot::Failed("the endpoint is already in use".to_string());
        assert_eq!(
            banner(&failed).as_deref(),
            Some("the endpoint is already in use")
        );
        assert_eq!(banner(&WorkerBoot::Ready), None);
        assert_eq!(banner(&WorkerBoot::Pending), None);
    }

    #[test]
    fn a_failed_bootstrap_is_reported_verbatim_so_no_pane_opens_dead() {
        record(WorkerBoot::Failed("endpoint in use".to_string()));
        assert_eq!(state(), WorkerBoot::Failed("endpoint in use".to_string()));
        record(WorkerBoot::Ready);
        assert_eq!(state(), WorkerBoot::Ready);
        record(WorkerBoot::Pending);
    }
}
