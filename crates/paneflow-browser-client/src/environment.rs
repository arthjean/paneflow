use std::ffi::{OsStr, OsString};

pub fn host_environment(
    window: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(OsString, OsString)> {
    window
        .into_iter()
        .filter(|(key, _)| !is_pane_context(key))
        .collect()
}

fn is_pane_context(key: &OsStr) -> bool {
    key.to_str()
        .is_some_and(|key| paneflow_home::PANE_CONTEXT_ENV.contains(&key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pane_marker_is_shed_and_the_rest_is_kept() {
        let mut window: Vec<(OsString, OsString)> = paneflow_home::PANE_CONTEXT_ENV
            .iter()
            .map(|key| (OsString::from(key), OsString::from("pane")))
            .collect();
        window.push(("WAYLAND_DISPLAY".into(), "wayland-0".into()));
        window.push(("PANEFLOW_HOME".into(), "/home/u/.paneflow-dev".into()));
        let kept = host_environment(window);
        assert_eq!(
            kept,
            vec![
                ("WAYLAND_DISPLAY".into(), "wayland-0".into()),
                ("PANEFLOW_HOME".into(), "/home/u/.paneflow-dev".into()),
            ]
        );
    }
}
