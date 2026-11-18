use std::path::{Path, PathBuf};

pub(crate) const OPEN_WITH_DEFAULT_LABEL: &str = "Open with Default App";
pub(crate) const OPEN_WITH_LABEL: &str = "Open With";

pub(crate) fn reveal_label_for(os: &str) -> &'static str {
    match os {
        "macos" => "Reveal in Finder",
        "windows" => "Reveal in File Explorer",
        _ => "Reveal in File Manager",
    }
}

pub(crate) fn reveal_label() -> &'static str {
    reveal_label_for(std::env::consts::OS)
}

fn file_manager_name() -> &'static str {
    reveal_label().trim_start_matches("Reveal in ")
}

pub(crate) async fn open_with_default_app(path: PathBuf) -> Result<(), String> {
    smol::unblock(move || crate::external_open::open_path(&path))
        .await
        .map_err(|error| format!("Couldn't open with the default app: {error}"))
}

pub(crate) async fn open_in_editor(
    path: PathBuf,
    command: &'static str,
    label: &'static str,
) -> Result<(), String> {
    let name = crate::app::workspace_ops::editor_toast_label(label);
    let launch = smol::unblock(move || {
        let binary = crate::app::workspace_ops::resolve_editor_binary(command);
        let mut launch = std::process::Command::new(binary);
        if let Some(parent) = path.parent() {
            launch.current_dir(parent);
        }
        launch.arg(&path);
        launch
    })
    .await;
    match crate::external_open::run_workspace_command(launch).await {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "Couldn't open in {name}: launcher exited with {status}"
        )),
        Err(error) => Err(format!("Couldn't open in {name}: {error}")),
    }
}

pub(crate) async fn reveal_in_file_manager(path: PathBuf) -> Result<(), String> {
    reveal(&path)
        .await
        .map_err(|error| format!("Couldn't reveal in {}: {error}", file_manager_name()))
}

#[cfg(target_os = "macos")]
async fn reveal(path: &Path) -> std::io::Result<()> {
    let mut launch = std::process::Command::new("open");
    launch.arg("-R").arg(path);
    let status = crate::external_open::run_workspace_command(launch).await?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("open exited with {status}")))
    }
}

#[cfg(target_os = "windows")]
async fn reveal(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut launch = std::process::Command::new("explorer.exe");
    launch.raw_arg(format!("/select,\"{}\"", path.display()));
    crate::external_open::run_workspace_command(launch)
        .await
        .map(|_| ())
}

#[cfg(target_os = "linux")]
async fn reveal(path: &Path) -> std::io::Result<()> {
    match show_items(path).await {
        Ok(()) => Ok(()),
        Err(error) => {
            log::info!("FileManager1.ShowItems unavailable ({error}); opening the parent folder");
            let parent = path.parent().unwrap_or(path);
            crate::app::workspace_ops::open_workspace_folder("xdg-open", parent).await
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
async fn reveal(path: &Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(path).to_path_buf();
    smol::unblock(move || crate::external_open::open_path(&parent)).await
}

#[cfg(target_os = "linux")]
const SHOW_ITEMS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(target_os = "linux")]
async fn show_items(path: &Path) -> Result<(), String> {
    let uri = file_uri(path);
    let call = async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| error.to_string())?;
        connection
            .call_method(
                Some("org.freedesktop.FileManager1"),
                "/org/freedesktop/FileManager1",
                Some("org.freedesktop.FileManager1"),
                "ShowItems",
                &(vec![uri.as_str()], ""),
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    };
    smol::future::or(call, async {
        smol::Timer::after(SHOW_ITEMS_TIMEOUT).await;
        Err("the file manager did not answer".to_string())
    })
    .await
}

#[cfg(target_os = "linux")]
pub(crate) fn file_uri(path: &Path) -> String {
    use std::fmt::Write;
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(char::from(*byte))
            }
            _ => {
                let _ = write!(uri, "%{byte:02X}");
            }
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reveal_label_names_the_file_manager_of_each_platform() {
        assert_eq!(reveal_label_for("macos"), "Reveal in Finder");
        assert_eq!(reveal_label_for("windows"), "Reveal in File Explorer");
        assert_eq!(reveal_label_for("linux"), "Reveal in File Manager");
        assert_eq!(reveal_label_for("freebsd"), "Reveal in File Manager");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn file_uris_escape_everything_but_unreserved_bytes() {
        assert_eq!(
            file_uri(Path::new("/home/a b/ré#1.rs")),
            "file:///home/a%20b/r%C3%A9%231.rs"
        );
    }

    #[test]
    fn a_missing_editor_is_reported_with_its_name_and_error() {
        let missing = std::env::temp_dir().join("paneflow-no-such-dir").join("x");
        let outcome = smol::block_on(open_in_editor(
            missing,
            "paneflow-no-such-editor",
            "Open in Nowhere",
        ));
        let message = outcome.expect_err("an editor that is not installed fails");
        assert!(
            message.starts_with("Couldn't open in Nowhere: "),
            "{message}"
        );
    }
}
