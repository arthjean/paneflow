//! Shared signed-asset download for self-update installers.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Download `asset_url` to `dest`, verify its detached minisign sibling, then
/// promote the verified bytes into place.
///
/// The caller owns installer-specific policy through `max_bytes`, `timeout`,
/// and `label`; the trust and staging mechanics stay identical across tar.gz,
/// DMG, and MSI paths.
pub(crate) fn download_verified_asset(
    asset_url: &str,
    dest: &Path,
    max_bytes: u64,
    timeout: Duration,
    label: &str,
) -> Result<()> {
    log::info!("self-update/{label}: downloading {asset_url}");

    let partial = append_suffix(dest, ".partial")?;
    let mut response = ureq::get(asset_url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .header(
            "User-Agent",
            &format!("paneflow/{}", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .with_context(|| "Could not download update. Try again when online.".to_string())?;
    if !response.status().is_success() {
        bail!(
            "Update download returned HTTP {}. Try again later.",
            response.status()
        );
    }

    let stream_result = {
        let reader = response.body_mut().as_reader();
        let mut reader = Read::take(reader, max_bytes + 1);
        let mut file = std::fs::File::create(&partial)
            .with_context(|| format!("create {}", partial.display()))?;
        std::io::copy(&mut reader, &mut file)
            .with_context(|| format!("stream {label} to disk"))
            .and_then(|written| {
                file.sync_all()
                    .with_context(|| format!("flush {label} to disk"))?;
                Ok(written)
            })
    };
    let written = match stream_result {
        Ok(n) => n,
        Err(e) => {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
    };
    if written > max_bytes {
        let _ = std::fs::remove_file(&partial);
        bail!(
            "Update download exceeded {} MiB - aborting.",
            max_bytes / 1024 / 1024
        );
    }

    if let Err(e) = super::signature::fetch_and_verify(&partial, asset_url) {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }

    std::fs::rename(&partial, dest)
        .with_context(|| format!("rename {} -> {}", partial.display(), dest.display()))?;
    Ok(())
}

fn append_suffix(path: &Path, suffix: &str) -> Result<PathBuf> {
    let name = path
        .file_name()
        .with_context(|| format!("path has no file name: {}", path.display()))?;
    let mut name = name.to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_suffix_preserves_full_name() {
        assert_eq!(
            append_suffix(Path::new("/tmp/foo.tar.gz"), ".partial").unwrap(),
            PathBuf::from("/tmp/foo.tar.gz.partial")
        );
    }

    #[test]
    fn append_suffix_rejects_pathless_input() {
        assert!(append_suffix(Path::new("/"), ".partial").is_err());
    }
}
