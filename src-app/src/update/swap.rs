use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) fn staging_dirs(live_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = live_dir
        .parent()
        .context("the install directory has no parent - refusing to swap at filesystem root")?;
    let name = live_dir
        .file_name()
        .context("the install directory has no file name - refusing to swap")?;
    let name = name.to_string_lossy();
    Ok((
        parent.join(format!("{name}.old")),
        parent.join(format!("{name}.new")),
    ))
}

pub(crate) fn recover_and_clean_staging(
    live_dir: &Path,
    old_dir: &Path,
    label: &str,
) -> Result<()> {
    if !old_dir.exists() {
        return Ok(());
    }
    if !live_dir.exists() {
        std::fs::rename(old_dir, live_dir).with_context(|| {
            format!(
                "recover live install {} from {}",
                live_dir.display(),
                old_dir.display()
            )
        })?;
        log::warn!(
            "self-update/{label}: recovered live install from a crashed prior update ({})",
            live_dir.display()
        );
        return Ok(());
    }
    if let Err(e) = std::fs::remove_dir_all(old_dir) {
        log::warn!(
            "self-update/{label}: could not remove stale {}: {e}",
            old_dir.display()
        );
    }
    Ok(())
}
