use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::host::SessionHost;

pub const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60);

pub fn spawn(host: &Arc<SessionHost>) {
    let weak = Arc::downgrade(host);
    let spawned = std::thread::Builder::new()
        .name("paneflow-host-maintenance".into())
        .spawn(move || maintenance_loop(weak));
    if let Err(error) = spawned {
        log::warn!("paneflow-host: cannot start the maintenance thread: {error}");
    }
}

fn maintenance_loop(weak: Weak<SessionHost>) {
    loop {
        std::thread::sleep(MAINTENANCE_INTERVAL);
        let Some(host) = weak.upgrade() else {
            return;
        };
        if host.is_shutting_down() {
            return;
        }
        host.run_maintenance();
    }
}
