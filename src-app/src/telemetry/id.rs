use crate::runtime_paths;

pub fn telemetry_id() -> String {
    telemetry_id_with_first_run().0
}

pub fn telemetry_id_with_first_run() -> (String, bool) {
    match runtime_paths::data_dir() {
        Some(dir) => paneflow_telemetry::id::telemetry_id_at(&dir),
        None => (
            paneflow_telemetry::id::ephemeral_id("no data_local_dir resolved"),
            false,
        ),
    }
}
