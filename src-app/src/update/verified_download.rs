use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use minisign_verify::PublicKey;

use super::UpdateError;

const MAX_REDIRECTS: usize = 5;

const COPY_CHUNK_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
pub(crate) struct HttpTimeouts {
    pub(crate) global: Option<Duration>,
    pub(crate) connect: Option<Duration>,
    pub(crate) send_request: Option<Duration>,
    pub(crate) stall: Option<Duration>,
}

impl HttpTimeouts {
    pub(crate) const fn whole_request(limit: Duration) -> Self {
        Self {
            global: Some(limit),
            connect: None,
            send_request: None,
            stall: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AssetTransfer {
    pub(crate) http: HttpTimeouts,
    pub(crate) total: Duration,
    pub(crate) redirect_allowed: fn(&str) -> bool,
}

pub(crate) const ASSET_TRANSFER: AssetTransfer = AssetTransfer {
    http: HttpTimeouts {
        global: None,
        connect: Some(Duration::from_secs(30)),
        send_request: Some(Duration::from_secs(30)),
        stall: Some(Duration::from_secs(60)),
    },
    total: Duration::from_secs(15 * 60),
    redirect_allowed: super::checker::is_allowed_redirect_target,
};

pub(crate) fn download_verified_asset(
    asset_url: &str,
    dest: &Path,
    max_bytes: u64,
    label: &str,
) -> Result<()> {
    download_verified_asset_with(
        asset_url,
        dest,
        max_bytes,
        label,
        &ASSET_TRANSFER,
        &super::signature::embedded_public_keys(),
    )
}

pub(crate) fn download_verified_asset_with(
    asset_url: &str,
    dest: &Path,
    max_bytes: u64,
    label: &str,
    transfer: &AssetTransfer,
    keys: &[PublicKey],
) -> Result<()> {
    log::info!("self-update/{label}: downloading {asset_url}");

    let partial = append_suffix(dest, ".partial")?;
    let mut response = get_following_redirects(asset_url, transfer.http, transfer.redirect_allowed)
        .with_context(|| "Could not download update. Try again when online.".to_string())?;
    if !response.status().is_success() {
        bail!(
            "Update download returned HTTP {}. Try again later.",
            response.status()
        );
    }

    let stream_result = {
        let reader = response.body_mut().as_reader();
        std::fs::File::create(&partial)
            .with_context(|| format!("create {}", partial.display()))
            .and_then(|mut file| {
                let written = copy_bounded(reader, &mut file, max_bytes, transfer.total, label)?;
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

    if let Err(e) = super::signature::fetch_and_verify_with(
        &partial,
        asset_url,
        keys,
        transfer.redirect_allowed,
    ) {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }

    std::fs::rename(&partial, dest)
        .with_context(|| format!("rename {} -> {}", partial.display(), dest.display()))?;
    Ok(())
}

pub(crate) fn get_following_redirects(
    url: &str,
    timeouts: HttpTimeouts,
    redirect_allowed: fn(&str) -> bool,
) -> Result<ureq::http::Response<ureq::Body>> {
    let mut current = url.to_string();
    for hop in 0..=MAX_REDIRECTS {
        let response = ureq::get(&current)
            .config()
            .max_redirects(0)
            .timeout_global(timeouts.global)
            .timeout_connect(timeouts.connect)
            .timeout_send_request(timeouts.send_request)
            .timeout_recv_body(timeouts.stall)
            .build()
            .header(
                "User-Agent",
                &format!("paneflow/{}", env!("CARGO_PKG_VERSION")),
            )
            .call()?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if hop == MAX_REDIRECTS {
            bail!("gave up after {MAX_REDIRECTS} redirects from {url}");
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .with_context(|| {
                format!(
                    "HTTP {} from {current} carries no usable Location",
                    response.status()
                )
            })?;
        if !redirect_allowed(location) {
            bail!("refusing a redirect from {current} to {location}");
        }
        current = location.to_string();
    }
    unreachable!("the redirect loop returns or bails on its last hop")
}

pub(crate) fn copy_bounded(
    mut reader: impl Read,
    writer: &mut impl Write,
    max_bytes: u64,
    total: Duration,
    label: &str,
) -> Result<u64> {
    let started = Instant::now();
    let mut buf = vec![0u8; COPY_CHUNK_BYTES];
    let mut written: u64 = 0;
    loop {
        if started.elapsed() > total {
            return Err(anyhow::Error::new(UpdateError::Timeout)
                .context(format!("{label} download took longer than {total:?}")));
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => return Ok(written),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if is_stall(&e) => {
                return Err(anyhow::Error::new(UpdateError::Timeout)
                    .context(format!("{label} download stalled: {e}")));
            }
            Err(e) => return Err(e).with_context(|| format!("stream {label} to disk")),
        };
        writer
            .write_all(&buf[..n])
            .with_context(|| format!("stream {label} to disk"))?;
        written += n as u64;
        if written > max_bytes {
            return Ok(written);
        }
    }
}

fn is_stall(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::TimedOut
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<ureq::Error>())
            .is_some_and(|inner| matches!(inner, ureq::Error::Timeout(_)))
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
#[path = "verified_download_tests.rs"]
mod tests;
